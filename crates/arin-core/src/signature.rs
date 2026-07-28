//! Coarse summaries of a captured frame, for telling scrolling apart from noise.
//!
//! # Why not a hash
//!
//! The obvious way to notice a display changing is to hash the pixels and compare. It
//! does not survive contact with a compositor. Downscaling, dithering, and subpixel
//! antialiasing all move individual bytes by one or two, so an exact hash reports that
//! everything changed when nothing did. Worse, it cannot distinguish an annotation
//! appearing from the page moving underneath it, and treating the former as the latter
//! makes the daemon clear the very mark it was asked to draw.
//!
//! # What replaces it
//!
//! A grid of sampled brightnesses, compared two ways:
//!
//! - A sample counts as changed only if it moved by more than [`SAMPLE_TOLERANCE`],
//!   which absorbs compositor rounding.
//! - The frame counts as moved only if more than [`MOVED_FRACTION`] of samples changed.
//!
//! The second one is what separates the two cases, because it measures *how much of the
//! screen* changed rather than whether anything did. Scrolling moves nearly everything
//! in the window being scrolled. An annotation is small and local: an orb covers well
//! under one percent of a display, and even a generous highlight only a few percent.
//!
//! # Why sample rather than average
//!
//! An earlier version averaged brightness over each cell. It rejected annotations
//! nicely and then failed to notice a terminal scrolling past, because a block of text
//! has roughly the same average brightness whichever glyphs are in it. Averaging is what
//! destroys the signal: the thing that changes when text scrolls is the pattern, not the
//! overall brightness. Point samples keep that pattern, and the tolerance plus the
//! fraction threshold supply the robustness the averaging was there for.

use crate::traits::Frame;

/// Samples per side. 96 gives 9216 points, fine enough to catch a line of text moving
/// and cheap enough to run twice a second for the life of a session.
const GRID: usize = 96;

/// How far one sample may drift and still count as unchanged, out of 255.
///
/// Covers compositor rounding. Measured drift on a still screen is zero, so this is
/// insurance for hardware that dithers rather than a load bearing number.
const SAMPLE_TOLERANCE: i16 = 6;

/// What fraction of samples must change before the content counts as having moved.
///
/// Sits between the two things being told apart: an annotation covers a few percent of a
/// display at most, while scrolling changes most of whatever is being scrolled.
const MOVED_FRACTION: f64 = 0.08;

/// A coarse summary of what a display looked like.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    width: u32,
    height: u32,
    /// Sampled brightness, row major.
    samples: Vec<u8>,
}

impl Signature {
    /// Summarise a frame.
    pub fn of(frame: &Frame) -> Self {
        let (width, height) = (frame.width as usize, frame.height as usize);
        let mut samples = vec![0u8; GRID * GRID];

        if width == 0 || height == 0 {
            return Self {
                width: frame.width,
                height: frame.height,
                samples,
            };
        }

        for row in 0..GRID {
            // Sample the middle of each band rather than its edge, so a one pixel shift
            // does not move every sample onto a boundary at once.
            let y = (row * 2 + 1) * height / (GRID * 2);
            for col in 0..GRID {
                let x = (col * 2 + 1) * width / (GRID * 2);
                let idx = (y.min(height - 1) * width + x.min(width - 1)) * 4;
                // A frame shorter than its dimensions is a capture bug, not a reason to
                // panic on a timer.
                if let Some(px) = frame.pixels.get(idx..idx + 4) {
                    samples[row * GRID + col] = luminance(px);
                }
            }
        }

        Self {
            width: frame.width,
            height: frame.height,
            samples,
        }
    }

    /// The fraction of samples that moved more than the tolerance.
    ///
    /// Returns 1.0 for frames of different sizes, since there is nothing sensible to
    /// compare and a display that changed resolution has certainly changed.
    pub fn drift(&self, other: &Self) -> f64 {
        if self.width != other.width || self.height != other.height {
            return 1.0;
        }
        let changed = self
            .samples
            .iter()
            .zip(&other.samples)
            .filter(|(a, b)| (i16::from(**a) - i16::from(**b)).abs() > SAMPLE_TOLERANCE)
            .count();
        changed as f64 / self.samples.len() as f64
    }

    /// Whether the content moved, rather than merely differing a little.
    pub fn moved_from(&self, other: &Self) -> bool {
        self.drift(other) > MOVED_FRACTION
    }
}

/// Perceived brightness of one BGRA pixel.
///
/// Integer weights rather than floats: this runs over every pixel of every capture, and
/// the answer only has to be consistent, not colorimetrically exact.
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

    /// A frame with vertical texture, so that shifting it is detectable.
    fn textured(width: u32, height: u32) -> Vec<u8> {
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        for y in 0..height as usize {
            for x in 0..width as usize {
                let idx = (y * width as usize + x) * 4;
                // Stripes across y, so a vertical shift changes brightness everywhere.
                let v = if (y / 8) % 2 == 0 { 40 } else { 200 };
                pixels[idx] = v;
                pixels[idx + 1] = v;
                pixels[idx + 2] = v;
                pixels[idx + 3] = 255;
            }
        }
        pixels
    }

    fn frame(pixels: Vec<u8>, width: u32, height: u32) -> Frame {
        Frame {
            display: DisplayId(1),
            scale: 1.0,
            logical_size: [f64::from(width), f64::from(height)],
            width,
            height,
            pixels: Arc::from(pixels),
        }
    }

    /// Paint an opaque block, standing in for an annotation.
    fn block(pixels: &mut [u8], width: u32, rect: (usize, usize, usize, usize)) {
        let (x0, y0, w, h) = rect;
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                let idx = (y * width as usize + x) * 4;
                if idx + 4 <= pixels.len() {
                    pixels[idx] = 255;
                    pixels[idx + 1] = 176;
                    pixels[idx + 2] = 32;
                    pixels[idx + 3] = 255;
                }
            }
        }
    }

    #[test]
    fn an_unchanged_display_has_not_moved() {
        let a = Signature::of(&frame(textured(512, 320), 512, 320));
        let b = Signature::of(&frame(textured(512, 320), 512, 320));
        assert_eq!(a.drift(&b), 0.0);
        assert!(!a.moved_from(&b));
    }

    #[test]
    fn compositor_noise_is_not_movement() {
        let base = textured(512, 320);
        let mut noisy = base.clone();
        // Every byte off by a little, the way a rescaled composite comes back.
        for (i, byte) in noisy.iter_mut().enumerate() {
            if i % 4 != 3 {
                *byte = byte.saturating_add(if i % 2 == 0 { 3 } else { 2 });
            }
        }
        let a = Signature::of(&frame(base, 512, 320));
        let b = Signature::of(&frame(noisy, 512, 320));
        assert!(
            !a.moved_from(&b),
            "small per pixel drift must not read as a scroll, drift was {}",
            a.drift(&b)
        );
    }

    #[test]
    fn an_annotation_appearing_is_not_movement() {
        let base = textured(512, 320);
        let mut drawn = base.clone();
        // Roughly an orb: 72 logical points on a downscaled frame.
        block(&mut drawn, 512, (200, 120, 40, 40));

        let a = Signature::of(&frame(base, 512, 320));
        let b = Signature::of(&frame(drawn, 512, 320));
        assert!(
            !a.moved_from(&b),
            "a mark the daemon drew must not read as a scroll, drift was {}",
            a.drift(&b)
        );
    }

    #[test]
    fn several_annotations_are_still_not_movement() {
        let base = textured(512, 320);
        let mut drawn = base.clone();
        block(&mut drawn, 512, (40, 40, 40, 40));
        block(&mut drawn, 512, (300, 90, 90, 45));
        block(&mut drawn, 512, (120, 220, 70, 35));

        let a = Signature::of(&frame(base, 512, 320));
        let b = Signature::of(&frame(drawn, 512, 320));
        assert!(
            !a.moved_from(&b),
            "a session's worth of marks must not read as a scroll, drift was {}",
            a.drift(&b)
        );
    }

    #[test]
    fn scrolling_is_movement() {
        let base = textured(512, 320);
        // Shift the content up by half a stripe, which is what scrolling looks like.
        let mut scrolled = vec![0u8; base.len()];
        let row = 512 * 4;
        let shift = 4 * row;
        scrolled[..base.len() - shift].copy_from_slice(&base[shift..]);

        let a = Signature::of(&frame(base, 512, 320));
        let b = Signature::of(&frame(scrolled, 512, 320));
        assert!(
            a.moved_from(&b),
            "content shifting must read as a scroll, drift was {}",
            a.drift(&b)
        );
    }

    /// Text-like content: uniform density, so the average brightness of any region is
    /// about the same no matter which glyphs are in it.
    fn texty(width: u32, height: u32, offset: usize) -> Vec<u8> {
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        for y in 0..height as usize {
            for x in 0..width as usize {
                let idx = (y * width as usize + x) * 4;
                let src = y + offset;
                // Deterministic pseudo glyphs. Ink covers a similar share of every line,
                // which is what makes averaging blind to it.
                let ink = ((x * 7 + src * 13) ^ (x / 3 + src * 5)) % 5 == 0;
                let v = if ink { 230 } else { 20 };
                pixels[idx] = v;
                pixels[idx + 1] = v;
                pixels[idx + 2] = v;
                pixels[idx + 3] = 255;
            }
        }
        pixels
    }

    #[test]
    fn scrolling_text_is_movement() {
        // The case an averaging summary got wrong: a wall of text scrolls, the pattern
        // moves, and the brightness of every region stays put.
        let a = Signature::of(&frame(texty(512, 320, 0), 512, 320));
        let b = Signature::of(&frame(texty(512, 320, 9), 512, 320));
        assert!(
            a.moved_from(&b),
            "text scrolling must read as a scroll, drift was {}",
            a.drift(&b)
        );
    }

    #[test]
    fn an_annotation_over_text_is_still_not_movement() {
        let base = texty(512, 320, 0);
        let mut drawn = base.clone();
        block(&mut drawn, 512, (200, 120, 40, 40));
        let a = Signature::of(&frame(base, 512, 320));
        let b = Signature::of(&frame(drawn, 512, 320));
        assert!(
            !a.moved_from(&b),
            "a mark over text must not read as a scroll, drift was {}",
            a.drift(&b)
        );
    }

    #[test]
    fn a_resized_display_counts_as_moved() {
        let a = Signature::of(&frame(textured(512, 320), 512, 320));
        let b = Signature::of(&frame(textured(640, 400), 640, 400));
        assert_eq!(a.drift(&b), 1.0);
        assert!(a.moved_from(&b));
    }

    #[test]
    fn a_truncated_frame_does_not_panic() {
        let short = frame(vec![0; 16], 512, 320);
        let _ = Signature::of(&short);
    }
}
