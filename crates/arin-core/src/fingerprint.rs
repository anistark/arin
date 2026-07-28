//! What was under an annotation when it was drawn, so a moved one can be checked.
//!
//! [`crate::signature`] measures one movement for a whole display. That is the right
//! granularity for a scroll and the wrong one for everything a scroll is not: a page that
//! moves under a static toolbar has one honest answer for most of the screen and a
//! different one for the rest, and the display-wide estimate cannot tell which side of
//! that line any given mark falls on.
//!
//! So each annotation also records what its own region looked like. After the daemon
//! follows a movement, it looks again at where the mark now sits. Content that travelled
//! with the scroll still matches. A mark dragged off the thing it was pointing at does
//! not, and goes.
//!
//! This is the reserved `content_hash` on the anchor, filled in at last.
//!
//! # Why the bar is set so low
//!
//! The overlay is in the frame. A capture taken while a mark is on screen contains that
//! mark, so a fingerprint recorded before it was drawn and one measured after are
//! comparing partly different pixels, and the difference is the daemon's own ink. A text
//! box covers its whole anchor and an orb a good part of one.
//!
//! Rather than pretend that away, [`Fingerprint::matches`] asks only that a *minority* of
//! samples disagree. That is enough for the failure being guarded against, which is not
//! subtle: a mark left behind by a partial scroll is looking at unrelated content and
//! disagrees almost everywhere. Distinguishing a button from the same button one line
//! lower is not something this is being asked to do.
//!
//! # Why not a hash
//!
//! Same reason as [`crate::signature`]. Compositor rounding moves individual bytes, so an
//! exact digest reports a mismatch on a screen that did not change. The stored value is a
//! small grid of brightnesses compared with a tolerance, and `content_hash` is the name
//! the field was reserved under rather than a claim about what is in it.

use crate::traits::Frame;
use arin_protocol::LogicalRect;

/// Samples per side of the region. Sixteen values, one per cell of a 4x4 grid.
///
/// Small on purpose. It travels on the wire inside an anchor, and it only has to tell
/// content apart from unrelated content.
const GRID: usize = 4;

/// How far one sample may drift and still count as the same content, out of 255.
///
/// Looser than the scroll tolerance. This compares two different captures of a region
/// that has moved across the screen, where subpixel positioning genuinely differs, rather
/// than two captures of a still one.
const TOLERANCE: i16 = 24;

/// What share of samples must still agree for the content to be the same.
///
/// Deliberately a minority-rules bar. See the module docs: the daemon's own mark is in
/// the later capture and can account for a large share of the region on its own.
const AGREEMENT: f64 = 0.5;

/// A coarse record of the content under an annotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    samples: [u8; GRID * GRID],
}

impl Fingerprint {
    /// Sample what a frame shows inside a region.
    ///
    /// `None` when the frame carries nothing to sample, which is the headless case and
    /// the not-yet-permitted one. An annotation without a fingerprint is followed on the
    /// display-wide estimate alone.
    pub fn of(frame: &Frame, rect: LogicalRect) -> Option<Self> {
        let (width, height) = (frame.width as usize, frame.height as usize);
        let [logical_width, logical_height] = frame.logical_size;
        if width == 0 || height == 0 || logical_width <= 0.0 || logical_height <= 0.0 {
            return None;
        }
        if !rect.is_valid() {
            return None;
        }

        let mut samples = [0u8; GRID * GRID];
        for row in 0..GRID {
            for col in 0..GRID {
                // The middle of each cell, for the same reason the signature grid does it:
                // a sample on a boundary moves onto the neighbouring cell for free.
                let at_x = rect.x + rect.width * (col as f64 * 2.0 + 1.0) / (GRID as f64 * 2.0);
                let at_y = rect.y + rect.height * (row as f64 * 2.0 + 1.0) / (GRID as f64 * 2.0);

                // Positions off the edge are clamped rather than dropped, so a mark half
                // off the screen still records the half that is on it.
                let x = ((at_x / logical_width) * width as f64).round();
                let y = ((at_y / logical_height) * height as f64).round();
                if !x.is_finite() || !y.is_finite() {
                    return None;
                }
                let x = x.clamp(0.0, width as f64 - 1.0) as usize;
                let y = y.clamp(0.0, height as f64 - 1.0) as usize;

                let idx = (y * width + x) * 4;
                if let Some(px) = frame.pixels.get(idx..idx + 4) {
                    samples[row * GRID + col] = luminance(px);
                }
            }
        }
        Some(Self { samples })
    }

    /// Read one back off an anchor.
    ///
    /// Anything that is not the expected length of hex is `None`. The field is reserved
    /// on the wire and a client may put whatever it likes there, so this refuses to guess
    /// rather than comparing against nonsense.
    pub fn parse(encoded: &str) -> Option<Self> {
        if encoded.len() != GRID * GRID * 2 {
            return None;
        }
        let mut samples = [0u8; GRID * GRID];
        for (slot, pair) in samples.iter_mut().zip(encoded.as_bytes().chunks_exact(2)) {
            let pair = std::str::from_utf8(pair).ok()?;
            *slot = u8::from_str_radix(pair, 16).ok()?;
        }
        Some(Self { samples })
    }

    /// Render for the anchor's `content_hash`.
    pub fn encode(&self) -> String {
        use std::fmt::Write as _;
        self.samples.iter().fold(String::new(), |mut out, sample| {
            // Writing to a String cannot fail.
            let _ = write!(out, "{sample:02x}");
            out
        })
    }

    /// Whether this is plausibly the same content as `other`.
    pub fn matches(&self, other: &Self) -> bool {
        let agreed = self
            .samples
            .iter()
            .zip(&other.samples)
            .filter(|(a, b)| (i16::from(**a) - i16::from(**b)).abs() <= TOLERANCE)
            .count();
        agreed as f64 / self.samples.len() as f64 >= AGREEMENT
    }
}

/// Perceived brightness of one BGRA pixel.
fn luminance(bgra: &[u8]) -> u8 {
    let b = u32::from(bgra[0]);
    let g = u32::from(bgra[1]);
    let r = u32::from(bgra[2]);
    ((r * 77 + g * 150 + b * 29) >> 8) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use arin_protocol::DisplayId;
    use std::sync::Arc;

    const WIDTH: u32 = 512;
    const HEIGHT: u32 = 320;

    fn frame(pixels: Vec<u8>) -> Frame {
        Frame {
            display: DisplayId(1),
            scale: 1.0,
            logical_size: [f64::from(WIDTH), f64::from(HEIGHT)],
            width: WIDTH,
            height: HEIGHT,
            pixels: Arc::from(pixels),
        }
    }

    /// Content with a strong vertical pattern, offset by whole rows.
    fn page(offset: i32) -> Vec<u8> {
        let mut pixels = vec![0u8; WIDTH as usize * HEIGHT as usize * 4];
        for y in 0..HEIGHT as usize {
            let row = y as i32 + offset;
            for x in 0..WIDTH as usize {
                // Varies along both axes, so a region is distinguishable from its
                // neighbours in either direction.
                let v = (((row * 7) ^ (x as i32 * 3)) & 0xFF) as u8;
                let idx = (y * WIDTH as usize + x) * 4;
                pixels[idx] = v;
                pixels[idx + 1] = v;
                pixels[idx + 2] = v;
                pixels[idx + 3] = 255;
            }
        }
        pixels
    }

    #[test]
    fn the_same_region_of_a_still_screen_matches() {
        let rect = LogicalRect::new(100.0, 80.0, 120.0, 60.0);
        let a = Fingerprint::of(&frame(page(0)), rect).unwrap();
        let b = Fingerprint::of(&frame(page(0)), rect).unwrap();
        assert!(a.matches(&b));
    }

    /// The case the whole thing exists for: content scrolled, the mark followed it, and
    /// the region it landed on is the region it started from.
    #[test]
    fn content_followed_across_a_scroll_matches() {
        let before = Fingerprint::of(&frame(page(0)), LogicalRect::new(100.0, 80.0, 120.0, 60.0));
        // The page moved up by 40 points, and the anchor moved with it.
        let after = Fingerprint::of(&frame(page(40)), LogicalRect::new(100.0, 40.0, 120.0, 60.0));
        assert!(before.unwrap().matches(&after.unwrap()));
    }

    /// A mark that did not move with the content is looking at something else.
    #[test]
    fn a_mark_left_behind_does_not_match() {
        let rect = LogicalRect::new(100.0, 80.0, 120.0, 60.0);
        let before = Fingerprint::of(&frame(page(0)), rect).unwrap();
        let after = Fingerprint::of(&frame(page(40)), rect).unwrap();
        assert!(
            !before.matches(&after),
            "a stale anchor over changed content must not pass verification"
        );
    }

    #[test]
    fn a_round_trip_through_the_wire_survives() {
        let rect = LogicalRect::new(100.0, 80.0, 120.0, 60.0);
        let original = Fingerprint::of(&frame(page(0)), rect).unwrap();
        let encoded = original.encode();
        assert_eq!(encoded.len(), GRID * GRID * 2);
        assert_eq!(Fingerprint::parse(&encoded), Some(original));
    }

    #[test]
    fn nonsense_in_the_reserved_field_is_refused() {
        assert_eq!(Fingerprint::parse(""), None);
        assert_eq!(Fingerprint::parse("nope"), None);
        // Right length, not hex.
        assert_eq!(Fingerprint::parse(&"zz".repeat(GRID * GRID)), None);
    }

    #[test]
    fn a_frame_with_nothing_in_it_has_no_fingerprint() {
        let empty = Frame {
            display: DisplayId(1),
            scale: 1.0,
            logical_size: [0.0, 0.0],
            width: 0,
            height: 0,
            pixels: Vec::new().into(),
        };
        assert_eq!(
            Fingerprint::of(&empty, LogicalRect::new(0.0, 0.0, 10.0, 10.0)),
            None
        );
    }

    #[test]
    fn a_zero_area_region_has_no_fingerprint() {
        assert_eq!(
            Fingerprint::of(&frame(page(0)), LogicalRect::new(10.0, 10.0, 0.0, 10.0)),
            None
        );
    }

    /// The daemon's own mark lands in the later capture and not the earlier one. The bar
    /// is set to survive that, which is the whole reason it is set where it is.
    #[test]
    fn the_daemons_own_ink_does_not_fail_verification() {
        let rect = LogicalRect::new(100.0, 80.0, 120.0, 60.0);
        let clean = Fingerprint::of(&frame(page(0)), rect).unwrap();

        let mut painted = page(0);
        // An orb-sized blob over the middle of the region, in a colour nothing else uses.
        for y in 95..125 {
            for x in 140..180 {
                let idx = (y * WIDTH as usize + x) * 4;
                painted[idx] = 32;
                painted[idx + 1] = 176;
                painted[idx + 2] = 255;
                painted[idx + 3] = 255;
            }
        }
        let marked = Fingerprint::of(&frame(painted), rect).unwrap();
        assert!(
            clean.matches(&marked),
            "a mark covering part of its own anchor must not read as different content"
        );
    }
}
