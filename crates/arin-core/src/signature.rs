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
//!
//! # Measuring how far, not only whether
//!
//! Noticing movement is enough to throw annotations away, which is all 0.1 did. Following
//! the content instead needs a distance, so a signature also carries two one-dimensional
//! profiles: one value per horizontal band, one per vertical band. [`Signature::shift_from`]
//! slides each profile against its predecessor and takes the offset that lines them up.
//!
//! Averaging is right here and wrong above, which looks contradictory until you notice
//! the two are measuring different things. Averaging *along* a row keeps the vertical
//! pattern that a vertical scroll moves: a row through a line of text is darker than the
//! gap above it, whatever the glyphs are. Averaging over a two-dimensional *cell* is what
//! destroyed the signal, because it collapsed the axis being measured.
//!
//! An offset is only returned when it is worth acting on. [`shift_along`] decides what
//! one axis can say, [`reconcile`] decides what the two of them say together, and every
//! rule in both answers the same question: is a shift genuinely the explanation for what
//! changed, or merely the least bad one?

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

/// Most bands a profile is cut into, per axis.
///
/// Bands are what bound the accuracy of a measured shift: on a 1117 point display this
/// resolves to about two logical points, well inside an orb. Capturing downscaled, which
/// is what the scroll watcher does, usually produces fewer rows than this and every row
/// becomes its own band.
const PROFILE_BANDS: usize = 512;

/// Samples averaged into each band.
const PROFILE_SAMPLES: usize = 64;

/// A profile flatter than this carries no usable structure.
///
/// Not a failure: an axis that looks the same everywhere cannot show movement along
/// itself, and content that uniform does not visibly move either. Zero is the honest
/// answer rather than an admission of defeat.
const FEATURELESS: f64 = 2.0;

/// How well two profiles must line up before a shift is believed, out of 255.
///
/// This is the whole partial-scroll defence. A window scrolling inside a screen that is
/// otherwise still leaves every static band disagreeing with the winning offset, which
/// lifts the residual well clear of the noise floor and the shift is refused. A starting
/// number rather than a measured one: it wants a real corpus of scrolls behind it.
const PROFILE_MATCH: f64 = 6.0;

/// Bands either side of the winner that count as the same answer.
const NEIGHBOURHOOD: i32 = 2;

/// A shift is only believed when the nearest distant rival is this much worse.
///
/// Evenly spaced text lines make a profile periodic, so sliding by exactly one line pitch
/// scores almost as well as the truth. Two plausible answers means no answer.
const DECISIVE: f64 = 2.0;

/// Absolute margin on the decisiveness test, so a near-zero residual cannot make any
/// rival look like a rival.
const DECISIVE_MARGIN: f64 = 2.0;

/// How far a movement must carry before it is worth redrawing for, in logical points.
const NEGLIGIBLE: f64 = 1.0;

/// How far content moved between two frames, in logical points.
///
/// Positive is down and to the right, matching the protocol's axes. Add it to an anchor
/// to follow the content.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shift {
    /// Horizontal movement in logical points.
    pub dx: f64,
    /// Vertical movement in logical points.
    pub dy: f64,
}

impl Shift {
    /// Whether this is too small to be worth moving anything for.
    pub fn is_negligible(&self) -> bool {
        self.dx.abs() < NEGLIGIBLE && self.dy.abs() < NEGLIGIBLE
    }
}

/// A coarse summary of what a display looked like.
#[derive(Debug, Clone, PartialEq)]
pub struct Signature {
    width: u32,
    height: u32,
    /// Display size in logical points, for reporting a shift in the protocol's units.
    logical_size: [f64; 2],
    /// Sampled brightness, row major.
    samples: Vec<u8>,
    /// Mean brightness per horizontal band, top to bottom. Moves with a vertical scroll.
    rows: Vec<u8>,
    /// Mean brightness per vertical band, left to right. Moves with a horizontal scroll.
    columns: Vec<u8>,
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
                logical_size: frame.logical_size,
                samples,
                rows: Vec::new(),
                columns: Vec::new(),
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
            logical_size: frame.logical_size,
            samples,
            rows: profile(frame, Axis::Vertical),
            columns: profile(frame, Axis::Horizontal),
        }
    }

    /// How far the content moved since `previous`, if that can be established.
    ///
    /// `None` means the change has no single shift that explains it: a partial scroll, a
    /// window appearing, a page replaced outright. The caller's fallback is what 0.1 did
    /// unconditionally, which is to invalidate.
    pub fn shift_from(&self, previous: &Self) -> Option<Shift> {
        if self.width != previous.width || self.height != previous.height {
            return None;
        }
        let down = shift_along(&previous.rows, &self.rows);
        let across = shift_along(&previous.columns, &self.columns);
        let (bands_down, bands_across) = reconcile(down, across)?;
        Some(Shift {
            dx: to_logical(bands_across, self.columns.len(), self.logical_size[0]),
            dy: to_logical(bands_down, self.rows.len(), self.logical_size[1]),
        })
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

/// Which axis a profile measures along.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    /// One value per horizontal band. Moves when content scrolls up or down.
    Vertical,
    /// One value per vertical band. Moves when content scrolls left or right.
    Horizontal,
}

/// Mean brightness of each band along an axis.
///
/// Samples the middle of each band rather than its edge, so a one pixel shift does not
/// move every sample onto a boundary at once. Same reasoning as the two dimensional grid
/// above, for the same reason.
fn profile(frame: &Frame, axis: Axis) -> Vec<u8> {
    let (width, height) = (frame.width as usize, frame.height as usize);
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let (along, across) = match axis {
        Axis::Vertical => (height, width),
        Axis::Horizontal => (width, height),
    };

    let bands = along.min(PROFILE_BANDS);
    let mut values = vec![0u8; bands];
    for (band, value) in values.iter_mut().enumerate() {
        let major = ((band * 2 + 1) * along / (bands * 2)).min(along - 1);
        let mut total = 0u32;
        let mut taken = 0u32;
        for sample in 0..PROFILE_SAMPLES {
            let minor = ((sample * 2 + 1) * across / (PROFILE_SAMPLES * 2)).min(across - 1);
            let (x, y) = match axis {
                Axis::Vertical => (minor, major),
                Axis::Horizontal => (major, minor),
            };
            let idx = (y * width + x) * 4;
            // A frame shorter than its dimensions is a capture bug, not a reason to panic
            // on a timer.
            if let Some(px) = frame.pixels.get(idx..idx + 4) {
                total += u32::from(luminance(px));
                taken += 1;
            }
        }
        if taken > 0 {
            *value = (total / taken) as u8;
        }
    }
    values
}

/// What one axis was able to say about the movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Estimate {
    /// A decisive offset, in bands.
    Moved(i32),
    /// The profile carries no structure, so nothing along this axis can be seen to move.
    ///
    /// Distinct from measuring zero. Content this uniform would not visibly move if it
    /// did, so the axis abstains rather than voting.
    Blind,
    /// The profile has structure, and no offset accounts for how it changed.
    Unexplained,
}

/// Turn two per-axis readings into one movement, or decline to.
///
/// The asymmetry here is the interesting part. A vertical scroll *should* leave the
/// horizontal profile unexplained: each vertical band is an average down the screen, and
/// scrolling pulls new content into it, so the profile changes without having shifted.
/// Refusing to follow a scroll because the other axis changed would refuse every real
/// scroll, which is what a first attempt at this did.
///
/// So one axis naming a genuine movement is allowed to account for the other axis being
/// unexplainable. What is never allowed is *nothing* accounting for it: an axis with
/// structure that changed, next to an axis that either measured no movement or could not
/// look, is a content change rather than a scroll and the marks go.
///
/// The gap this leaves is a diagonal scroll whose horizontal component is not decisive on
/// its own. That is followed vertically and not horizontally, so the mark ends up sideways
/// of its target. Per-annotation verification in [`crate::fingerprint`] is what catches it.
fn reconcile(down: Estimate, across: Estimate) -> Option<(i32, i32)> {
    use Estimate::{Blind, Moved, Unexplained};
    match (down, across) {
        (Moved(dy), Moved(dx)) => Some((dy, dx)),
        (Moved(dy), Blind) => Some((dy, 0)),
        (Blind, Moved(dx)) => Some((0, dx)),
        // Nothing to see on either axis. Nothing moved that anyone could point at.
        (Blind, Blind) => Some((0, 0)),
        // One axis moved and the other cannot be read as a shift: the movement explains
        // the change. A stationary axis explains nothing, so that is a content change.
        (Moved(0), Unexplained) | (Unexplained, Moved(0)) => None,
        (Moved(dy), Unexplained) => Some((dy, 0)),
        (Unexplained, Moved(dx)) => Some((0, dx)),
        (Unexplained, _) | (_, Unexplained) => None,
    }
}

/// The offset, in bands, that best explains `current` as `previous` shifted.
///
/// Two things have to hold before an offset is believed, and each rules out a way of
/// being confidently wrong:
///
/// 1. The profiles must actually line up at the winning offset. A region of the screen
///    that did not move disagrees with any non-zero shift, which is what stops a window
///    scrolling inside a still screen from dragging every other mark along with it.
/// 2. No distant offset may score nearly as well. Lines of text are evenly spaced, so
///    sliding by one line pitch looks almost as good as the truth, and two plausible
///    answers are worth no answer at all.
///
/// Failing either yields [`Estimate::Unexplained`] rather than a guess. A mark that
/// vanishes is a mark the client can see is gone. A mark confidently pointing at the
/// wrong thing is worse, and the client has no way to know.
fn shift_along(previous: &[u8], current: &[u8]) -> Estimate {
    let bands = previous.len();
    if bands == 0 || bands != current.len() {
        return Estimate::Unexplained;
    }
    // Nothing to line up against, and nothing that would visibly move if it did.
    if spread(previous) < FEATURELESS || spread(current) < FEATURELESS {
        return Estimate::Blind;
    }

    // Half the axis. Beyond that the overlap is too small to mean anything, and a jump
    // that far is a new page rather than a scroll.
    let reach = (bands / 2) as i32;
    let scores: Vec<(i32, f64)> = (-reach..=reach)
        .filter_map(|offset| mismatch(previous, current, offset).map(|score| (offset, score)))
        .collect();

    let &(best, residual) = scores
        .iter()
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .expect("an offset of zero always overlaps");

    if residual > PROFILE_MATCH {
        return Estimate::Unexplained;
    }

    let rival = scores
        .iter()
        .filter(|(offset, _)| (offset - best).abs() > NEIGHBOURHOOD)
        .map(|(_, score)| *score)
        .fold(f64::INFINITY, f64::min);
    if rival < residual * DECISIVE + DECISIVE_MARGIN {
        return Estimate::Unexplained;
    }

    Estimate::Moved(best)
}

/// Mean absolute difference between `current` and `previous` slid by `offset` bands.
///
/// `None` when the two overlap over less than half the axis, which is what keeps a large
/// offset from winning on a handful of agreeable bands.
fn mismatch(previous: &[u8], current: &[u8], offset: i32) -> Option<f64> {
    let bands = previous.len() as i32;
    let first = offset.max(0);
    let last = (bands + offset).min(bands);
    let overlap = last - first;
    if overlap * 2 < bands {
        return None;
    }

    let total: u32 = (first..last)
        .map(|i| {
            let here = i16::from(current[i as usize]);
            let there = i16::from(previous[(i - offset) as usize]);
            (here - there).unsigned_abs() as u32
        })
        .sum();
    Some(f64::from(total) / f64::from(overlap))
}

/// Mean absolute deviation of a profile, as a stand-in for how much structure it has.
fn spread(values: &[u8]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mean = values.iter().map(|v| f64::from(*v)).sum::<f64>() / values.len() as f64;
    values
        .iter()
        .map(|v| (f64::from(*v) - mean).abs())
        .sum::<f64>()
        / values.len() as f64
}

/// Convert a band offset into the logical points the protocol speaks.
fn to_logical(bands: i32, total: usize, extent: f64) -> f64 {
    if total == 0 || !extent.is_finite() {
        return 0.0;
    }
    f64::from(bands) * extent / total as f64
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

    /// Brightness for a band, aperiodic over any plausible display.
    ///
    /// An earlier version of this multiplied and shifted, which produced an arithmetic
    /// progression modulo 256: a sawtooth with a period of sixteen. The decisiveness
    /// test correctly refused to name a shift on it, which is the right answer to the
    /// wrong question. A proper bit mixer is what makes the pattern actually unrepeating.
    fn noise(n: i32) -> u8 {
        let mut x = (n as u32) ^ 0x9E37_79B9;
        x ^= x >> 16;
        x = x.wrapping_mul(0x7FEB_352D);
        x ^= x >> 15;
        x = x.wrapping_mul(0x846C_A68B);
        x ^= x >> 16;
        (x & 0xFF) as u8
    }

    /// Content with a vertical pattern that never repeats, which is what an offset can
    /// actually be measured against. `textured` above is deliberately periodic and is
    /// used to prove the opposite case.
    fn landscape(width: u32, height: u32, offset: i32) -> Vec<u8> {
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        for y in 0..height as usize {
            let v = noise(y as i32 + offset);
            for x in 0..width as usize {
                let idx = (y * width as usize + x) * 4;
                pixels[idx] = v;
                pixels[idx + 1] = v;
                pixels[idx + 2] = v;
                pixels[idx + 3] = 255;
            }
        }
        pixels
    }

    /// The same pattern running the other way, for the horizontal case.
    fn upright(width: u32, height: u32, offset: i32) -> Vec<u8> {
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        for x in 0..width as usize {
            let v = noise(x as i32 + offset);
            for y in 0..height as usize {
                let idx = (y * width as usize + x) * 4;
                pixels[idx] = v;
                pixels[idx + 1] = v;
                pixels[idx + 2] = v;
                pixels[idx + 3] = 255;
            }
        }
        pixels
    }

    #[test]
    fn a_still_screen_has_not_shifted() {
        let a = Signature::of(&frame(landscape(512, 320, 0), 512, 320));
        let b = Signature::of(&frame(landscape(512, 320, 0), 512, 320));
        let shift = b.shift_from(&a).expect("a still screen is explainable");
        assert!(
            shift.is_negligible(),
            "a still screen must not read as movement, got {shift:?}"
        );
    }

    /// The measurement the whole feature rests on: content that moved up by a known
    /// amount reports that amount, with the sign the protocol's axes use.
    #[test]
    fn a_vertical_scroll_reports_how_far() {
        // The frame's logical size equals its pixel size here, so a row is a point.
        let before = Signature::of(&frame(landscape(512, 320, 0), 512, 320));
        let after = Signature::of(&frame(landscape(512, 320, 12), 512, 320));

        let shift = after
            .shift_from(&before)
            .expect("a clean scroll is measurable");
        // Row y now shows what row y+12 used to, so the content moved twelve points up.
        assert!(
            (shift.dy + 12.0).abs() <= 1.0,
            "expected about -12 points of vertical movement, got {}",
            shift.dy
        );
        assert!(
            shift.dx.abs() <= 1.0,
            "nothing moved sideways, got {}",
            shift.dx
        );
    }

    #[test]
    fn a_horizontal_scroll_reports_how_far() {
        let before = Signature::of(&frame(upright(512, 320, 0), 512, 320));
        let after = Signature::of(&frame(upright(512, 320, 20), 512, 320));

        let shift = after
            .shift_from(&before)
            .expect("a clean scroll is measurable");
        assert!(
            (shift.dx + 20.0).abs() <= 1.0,
            "expected about -20 points of horizontal movement, got {}",
            shift.dx
        );
        assert!(
            shift.dy.abs() <= 1.0,
            "nothing moved down, got {}",
            shift.dy
        );
    }

    /// The case that decides whether this is safe to ship. Part of the screen scrolled
    /// and part did not, so there is no one offset that follows the content, and marks on
    /// the still part would be dragged somewhere arbitrary by any answer at all.
    #[test]
    fn a_partial_scroll_has_no_answer() {
        let still = landscape(512, 320, 0);
        let mut mixed = landscape(512, 320, 24);
        // Put the top third back the way it was: a toolbar that did not move.
        let split = (320 / 3) * 512 * 4;
        mixed[..split].copy_from_slice(&still[..split]);

        let before = Signature::of(&frame(still, 512, 320));
        let after = Signature::of(&frame(mixed, 512, 320));
        assert_eq!(
            after.shift_from(&before),
            None,
            "a screen that moved in one place and not another must refuse to name a shift"
        );
    }

    /// Evenly spaced content offers several equally good answers, and several answers is
    /// no answer. `textured` is stripes on a fixed pitch, which is the worst case.
    #[test]
    fn periodic_content_refuses_to_guess() {
        let base = textured(512, 320);
        let mut scrolled = vec![0u8; base.len()];
        let shift = 4 * 512 * 4;
        scrolled[..base.len() - shift].copy_from_slice(&base[shift..]);

        let before = Signature::of(&frame(base, 512, 320));
        let after = Signature::of(&frame(scrolled, 512, 320));
        assert_eq!(
            after.shift_from(&before),
            None,
            "stripes on a fixed pitch line up at many offsets, so none of them is the answer"
        );
    }

    #[test]
    fn a_mark_appearing_does_not_read_as_a_shift() {
        let base = landscape(512, 320, 0);
        let mut drawn = base.clone();
        block(&mut drawn, 512, (200, 120, 40, 40));

        let before = Signature::of(&frame(base, 512, 320));
        let after = Signature::of(&frame(drawn, 512, 320));
        match after.shift_from(&before) {
            None => {}
            Some(shift) => assert!(
                shift.is_negligible(),
                "the daemon's own mark must not look like the page moving, got {shift:?}"
            ),
        }
    }

    /// A wall of text is what this actually runs against, so measure one. The pattern
    /// here has uniform ink density per line, which is the case that defeated an earlier
    /// averaging summary, and it still has to yield a distance rather than just a yes.
    #[test]
    fn scrolling_text_reports_how_far() {
        let before = Signature::of(&frame(texty(512, 320, 0), 512, 320));
        let after = Signature::of(&frame(texty(512, 320, 9), 512, 320));

        let shift = after
            .shift_from(&before)
            .expect("text scrolling by a known amount is measurable");
        assert!(
            (shift.dy + 9.0).abs() <= 1.0,
            "expected about -9 points of vertical movement, got {}",
            shift.dy
        );
    }

    #[test]
    fn a_resized_display_has_no_shift() {
        let a = Signature::of(&frame(landscape(512, 320, 0), 512, 320));
        let b = Signature::of(&frame(landscape(640, 400, 0), 640, 400));
        assert_eq!(b.shift_from(&a), None);
    }

    #[test]
    fn a_blank_screen_reports_no_movement() {
        // Nothing to line up against, but nothing that could visibly move either.
        let a = Signature::of(&frame(vec![20; 512 * 320 * 4], 512, 320));
        let b = Signature::of(&frame(vec![20; 512 * 320 * 4], 512, 320));
        assert_eq!(b.shift_from(&a), Some(Shift { dx: 0.0, dy: 0.0 }));
    }
}
