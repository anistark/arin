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
//! A block of text has roughly the same average brightness whichever glyphs are in it, so
//! averaging each cell cannot see a terminal scrolling past. What changes when text
//! scrolls is the pattern, not the overall brightness. Point samples keep that pattern,
//! and the tolerance plus the fraction threshold supply the robustness.
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

use crate::traits::{Frame, luminance};
use arin_protocol::LogicalRect;

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

/// How many strips a region is cut into across the axis being measured.
///
/// One mean per band throws away *where* in the band the ink was, and that is most of what
/// tells one line of text from the next. Measured against the recorded corpus, a single
/// profile per axis found the right offset on nine scrolls of eleven and only six survived
/// the checks around it. Eight strips found all eleven, and reported no movement on all
/// sixty-three still pairs.
///
/// Nearly free, because the strips divide the same pixels rather than adding any: the
/// reading cost is unchanged and only the comparison is multiplied, which is arithmetic
/// over a few hundred numbers.
const STRIPS: usize = 8;

/// Most samples to average into each band of a region's profile.
///
/// High enough that a band is a stable average rather than a noisy one, capped so that a
/// full resolution capture of a large region does not read every pixel of it twice a
/// second. A downscaled capture falls well under this and is read in full.
const BAND_SAMPLES: usize = 512;

/// A profile flatter than this carries no usable structure.
///
/// Not a failure: an axis that looks the same everywhere cannot show movement along
/// itself, and content that uniform does not visibly move either. Zero is the honest
/// answer rather than an admission of defeat.
const FEATURELESS: f64 = 2.0;

/// Bands either side of the winner that count as the same answer.
const NEIGHBOURHOOD: i32 = 2;

/// How much better the winning offset must be than the nearest offset that is not it.
///
/// Evenly spaced content is the case this exists for. Stripes on a fixed pitch, or lines of
/// code at a uniform leading, line up exactly as well every whole number of periods away,
/// so the winner ties with rivals far from it and picking one is a coin toss that can land
/// a mark a hundred points out.
///
/// A ratio, and a narrow one, because the corpus says there is not much room: real scrolls
/// scored between 1.04 and 1.76 on this, while an exact tie scores 1.00 by construction.
/// This sits between the two, which rejects content that genuinely aliases without
/// rejecting any measured scroll. Narrow enough to be worth revisiting when the corpus
/// grows.
const DECISIVE: f64 = 1.02;

/// How much better a shift must explain the change than not shifting at all.
///
/// A floor, not a discriminator. What separates a right answer from a wrong one is the
/// two scorers agreeing: against the recorded corpus every offset they agreed on was
/// correct, with this ratio ranging from 1.04 to 2.85. Setting it at 1.3 threw away a
/// correct answer scoring 1.04.
///
/// So it sits just above the value a still region produces, which is exactly 1.0, and
/// catches nothing but a shift that explains no more than staying put. The real work is
/// done by [`shift_along`] requiring two scorers that fail differently to arrive at the
/// same place.
const WORTH_MOVING: f64 = 1.02;

/// How far a movement must carry before it is worth redrawing for, in logical points.
const NEGLIGIBLE: f64 = 1.0;

/// How far from its old place to look for a mark's content, in logical points.
///
/// The template stays tight around the mark, so that it measures that mark's own window
/// rather than the desktop behind it. Reach is a separate question, and tying the two
/// together was a mistake: slid against itself, a region can only show half its own height
/// of movement, so a 540 point neighbourhood could not follow a scroll past 270 points and
/// ordinary flicks are bigger than that. Live, the winning offset was repeatedly the last
/// one in range. Searching a wider window in the later frame buys reach without loosening
/// what is being measured.
const SEARCH: f64 = 400.0;

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

/// How far the content inside one region of the screen moved between two frames.
///
/// This is what a mark actually needs to know, and measuring it across the whole display
/// was the mistake. On a real desktop a scroll happens inside a window: the menu bar, the
/// dock, the desktop and every other window stay exactly where they were. Correlated over
/// the whole screen that comes out as *nothing moved*, because globally nothing did, and
/// the one window that did is outvoted by everything that did not.
///
/// Measured on a 1512 by 982 display, scrolling a text window: the display-wide profiles
/// put their best offset at zero with a residual of 4.6, comfortably inside the tolerance,
/// while a fifth of the screen's samples had changed. The correct answer for the window
/// was tens of points and the correct answer for the screen was zero. Both are true, and
/// only one of them is any use to a mark sitting in that window.
///
/// The region is expected to be generous around the mark rather than tight to it. See
/// [`crate::daemon`] for how it is sized, and why the mark's own ink inside it is left to
/// be outvoted rather than masked out.
pub fn shift_within(before: &Frame, after: &Frame, region: LogicalRect) -> Option<Shift> {
    if before.width != after.width || before.height != after.height {
        tracing::debug!(
            was = format_args!("{}x{}", before.width, before.height),
            now = format_args!("{}x{}", after.width, after.height),
            "the capture changed size, so there is nothing to compare"
        );
        return None;
    }
    let Some(area) = to_pixels(after, region) else {
        tracing::debug!(?region, "the region is not on the frame");
        return None;
    };

    // How far to hunt, in this frame's pixels. Captures arrive downscaled, so this is a
    // much smaller number than it looks: on a 1512 point display captured 512 wide, four
    // hundred points is a hundred and thirty five pixels.
    let [logical_width, logical_height] = after.logical_size;
    let grow = |logical: f64, pixels: u32| {
        if logical <= 0.0 {
            return 0;
        }
        ((SEARCH / logical) * f64::from(pixels)).round().max(0.0) as usize
    };
    let grow_x = grow(logical_width, after.width);
    let grow_y = grow(logical_height, after.height);

    // Each axis widens only along itself, because the strips have to line up: strip three
    // of the template and strip three of the window must be the same columns of the
    // screen, or they are correlating unrelated slices of the display against each other.
    let down_window = Area {
        y0: area.y0.saturating_sub(grow_y),
        y1: (area.y1 + grow_y).min(after.height as usize),
        ..area
    };
    let across_window = Area {
        x0: area.x0.saturating_sub(grow_x),
        x1: (area.x1 + grow_x).min(after.width as usize),
        ..area
    };

    let down = shift_along(
        "down",
        &region_strips(before, area, Axis::Vertical),
        &region_strips(after, down_window, Axis::Vertical),
        (area.y0 - down_window.y0) as i32,
    );
    let across = shift_along(
        "across",
        &region_strips(before, area, Axis::Horizontal),
        &region_strips(after, across_window, Axis::Horizontal),
        (area.x0 - across_window.x0) as i32,
    );
    let (bands_down, bands_across) = reconcile(down, across)?;

    let Area { x0, y0, x1, y1 } = area;
    Some(Shift {
        dx: to_logical(bands_across, (x1 - x0).min(PROFILE_BANDS), region.width),
        dy: to_logical(bands_down, (y1 - y0).min(PROFILE_BANDS), region.height),
    })
}

/// A rectangle of a frame, in that frame's own pixels.
#[derive(Debug, Clone, Copy)]
struct Area {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

/// Where a logical rectangle lands in a frame's pixels.
///
/// Through the frame's own dimensions rather than its `scale`, for the reason the whole
/// crate keeps repeating: a downscaled capture reports its own pixels per point, and a
/// resolver may have shrunk it again. The frame is the only thing that knows how big the
/// frame is.
fn to_pixels(frame: &Frame, rect: LogicalRect) -> Option<Area> {
    let (width, height) = (frame.width as usize, frame.height as usize);
    let [logical_width, logical_height] = frame.logical_size;
    if width == 0 || height == 0 || logical_width <= 0.0 || logical_height <= 0.0 {
        return None;
    }
    if !rect.is_valid() {
        return None;
    }

    let scale_x = |v: f64| (v / logical_width) * width as f64;
    let scale_y = |v: f64| (v / logical_height) * height as f64;

    let x0 = scale_x(rect.x).floor().clamp(0.0, (width - 1) as f64) as usize;
    let y0 = scale_y(rect.y).floor().clamp(0.0, (height - 1) as f64) as usize;
    let x1 = scale_x(rect.x + rect.width).ceil().clamp(1.0, width as f64) as usize;
    let y1 = scale_y(rect.y + rect.height)
        .ceil()
        .clamp(1.0, height as f64) as usize;

    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(Area { x0, y0, x1, y1 })
}

/// Mean brightness per band, over one region of a frame.
fn region_profile(frame: &Frame, area: Area, axis: Axis) -> Vec<u8> {
    let width = frame.width as usize;
    let (along, across) = match axis {
        Axis::Vertical => (area.y1 - area.y0, area.x1 - area.x0),
        Axis::Horizontal => (area.x1 - area.x0, area.y1 - area.y0),
    };
    if along == 0 || across == 0 {
        return Vec::new();
    }

    let bands = along.min(PROFILE_BANDS);
    // Across the band, take everything there is rather than a fixed handful of samples.
    // Sixty-four was enough for a whole-frame summary and is not enough here: a band
    // averaged from sixty-four points is noisy, and two scorers reading the same noisy
    // profile disagree about where it lines up.
    let stride = across.div_ceil(BAND_SAMPLES).max(1);
    let mut values = vec![0u8; bands];
    for (band, value) in values.iter_mut().enumerate() {
        let major = ((band * 2 + 1) * along / (bands * 2)).min(along - 1);
        let mut total = 0u32;
        let mut taken = 0u32;
        let mut minor = 0;
        while minor < across {
            let (x, y) = match axis {
                Axis::Vertical => (area.x0 + minor, area.y0 + major),
                Axis::Horizontal => (area.x0 + major, area.y0 + minor),
            };
            let idx = (y * width + x) * 4;
            if let Some(px) = frame.pixels.get(idx..idx + 4) {
                total += u32::from(luminance(px));
                taken += 1;
            }
            minor += stride;
        }
        // A band that read no pixels keeps its zero, rather than dividing by none of them.
        if let Some(mean) = total.checked_div(taken) {
            *value = mean as u8;
        }
    }
    values
}

/// One profile per strip of a region.
fn region_strips(frame: &Frame, area: Area, axis: Axis) -> Vec<Vec<u8>> {
    (0..STRIPS)
        .map(|strip| {
            let narrowed = match axis {
                // Measuring down the region, so the strips divide it across.
                Axis::Vertical => {
                    let span = (area.x1 - area.x0).max(1);
                    let x0 = area.x0 + strip * span / STRIPS;
                    let x1 = (area.x0 + (strip + 1) * span / STRIPS)
                        .max(x0 + 1)
                        .min(area.x1);
                    Area { x0, x1, ..area }
                }
                Axis::Horizontal => {
                    let span = (area.y1 - area.y0).max(1);
                    let y0 = area.y0 + strip * span / STRIPS;
                    let y1 = (area.y0 + (strip + 1) * span / STRIPS)
                        .max(y0 + 1)
                        .min(area.y1);
                    Area { y0, y1, ..area }
                }
            };
            region_profile(frame, narrowed, axis)
        })
        .collect()
}

/// Mean absolute difference with the template laid down at `at`, over every strip.
fn strip_mismatch(template: &[Vec<u8>], window: &[Vec<u8>], at: i32) -> Option<f64> {
    if template.len() != window.len() || template.is_empty() {
        return None;
    }
    let mut total = 0.0;
    for (strip, into) in template.iter().zip(window) {
        total += mismatch(strip, into, at)?;
    }
    Some(total / template.len() as f64)
}

/// Which axis a profile measures along.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    /// One value per horizontal band. Moves when content scrolls up or down.
    Vertical,
    /// One value per vertical band. Moves when content scrolls left or right.
    Horizontal,
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
/// horizontal profile unexplained: each vertical band is an average down the region, and
/// scrolling pulls new content into it, so the profile changes without having shifted.
/// Refusing to follow a scroll because the other axis changed would refuse every real
/// scroll, which is what a first attempt at this did.
///
/// So one axis naming a genuine movement is allowed to account for the other axis being
/// unexplainable. What is never allowed is *nothing* accounting for it: an axis with
/// structure that changed, next to an axis that either measured no movement or could not
/// look, is a content change rather than a scroll and the mark goes.
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

/// The offset, in bands, that best explains where a template's content went.
///
/// Two things have to hold before an offset is believed, and each rules out a way of
/// being confidently wrong:
///
/// 1. The template must actually line up where it landed. A region of the screen that did
///    not move disagrees with any non-zero shift, which is what stops a window scrolling
///    inside a still screen from dragging every other mark along with it.
/// 2. No distant position may score nearly as well. Lines of text are evenly spaced, so
///    sliding by one line pitch looks almost as good as the truth, and two plausible
///    answers are worth no answer at all.
///
/// Failing either yields [`Estimate::Unexplained`] rather than a guess. A mark that
/// vanishes is a mark the client can see is gone. A mark confidently pointing at the
/// wrong thing is worse, and the client has no way to know.
///
/// `base` is where the template started, so that an answer can be reported relative to
/// where the mark already is rather than to the edge of the window being searched.
fn shift_along(axis: &str, template: &[Vec<u8>], window: &[Vec<u8>], base: i32) -> Estimate {
    let span = template.first().map_or(0, Vec::len);
    let reach = window.first().map_or(0, Vec::len);
    if span == 0 || reach < span || template.len() != window.len() {
        return Estimate::Unexplained;
    }

    let textured =
        |strips: &[Vec<u8>]| strips.iter().map(|strip| spread(strip)).fold(0.0, f64::max);
    // Both sides have to have something in them. A featureless region cannot be seen to
    // move, and a featureless one that gains a single feature must not read as a shift.
    if textured(template).min(textured(window)) < FEATURELESS {
        return Estimate::Blind;
    }

    // Where the template may sit in the window: anywhere that keeps half of it covered.
    let first = -(span as i32 / 2);
    let last = reach as i32 - span as i32 + span as i32 / 2;
    let Some((at, residual)) = (first..=last)
        .filter_map(|at| strip_mismatch(template, window, at).map(|s| (at, s)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
    else {
        return Estimate::Unexplained;
    };

    // An answer sitting on the end of the range is the search running out of room rather
    // than a measurement. Live, this was the common failure: the winning offset was
    // exactly the last one tried, and the mark was flung several hundred points off the
    // content it was pointing at.
    if at == first || at == last {
        tracing::debug!(
            axis,
            at,
            residual,
            "the search reached the end of its range"
        );
        return Estimate::Unexplained;
    }

    let best = at - base;
    if best.abs() <= NEIGHBOURHOOD {
        return Estimate::Moved(0);
    }

    let rival = (first..=last)
        .filter(|other| (other - at).abs() > NEIGHBOURHOOD)
        .filter_map(|other| strip_mismatch(template, window, other))
        .fold(f64::INFINITY, f64::min);
    // Not `<`. On content that repeats exactly, every alias scores zero, and a strict
    // comparison would let the first of them through as though it had won.
    if rival <= residual * DECISIVE {
        tracing::debug!(
            axis,
            best,
            residual,
            rival,
            "several offsets explain it equally"
        );
        return Estimate::Unexplained;
    }

    let staying = strip_mismatch(template, window, base).unwrap_or(f64::INFINITY);
    let gain = if residual > f64::EPSILON {
        staying / residual
    } else {
        f64::INFINITY
    };
    tracing::debug!(axis, best, residual, staying, gain, "correlated a region");

    if gain < WORTH_MOVING {
        return Estimate::Unexplained;
    }
    Estimate::Moved(best)
}

/// How badly `template` disagrees with `window` when laid down starting at index `at`.
///
/// Only the overlapping part is compared, and at least half the template has to be
/// covered: a sliver that happens to agree says nothing about where the rest went.
fn mismatch(template: &[u8], window: &[u8], at: i32) -> Option<f64> {
    let mut total = 0u32;
    let mut counted = 0usize;
    for (i, value) in template.iter().enumerate() {
        let Ok(j) = usize::try_from(at + i as i32) else {
            continue;
        };
        if let Some(other) = window.get(j) {
            total += u32::from(value.abs_diff(*other));
            counted += 1;
        }
    }
    if counted * 2 < template.len() {
        return None;
    }
    Some(f64::from(total) / counted as f64)
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use arin_protocol::DisplayId;
    use std::sync::Arc;

    /// A frame with vertical texture, so that shifting it is detectable.
    pub(crate) fn textured(width: u32, height: u32) -> Vec<u8> {
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

    pub(crate) fn frame(pixels: Vec<u8>, width: u32, height: u32) -> Frame {
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

    /// A page split in two: the top band never moves, the rest scrolls. This is what a
    /// real screen looks like, and what the display-wide measurement could not read.
    pub(crate) fn split_page(frozen_rows: usize, offset: i32) -> Vec<u8> {
        let (width, height) = (512usize, 320usize);
        let mut pixels = vec![0u8; width * height * 4];
        for y in 0..height {
            let source = if y < frozen_rows {
                y as i32
            } else {
                y as i32 + offset
            };
            let v = noise(source);
            for x in 0..width {
                let idx = (y * width + x) * 4;
                pixels[idx] = v;
                pixels[idx + 1] = v;
                pixels[idx + 2] = v;
                pixels[idx + 3] = 255;
            }
        }
        pixels
    }

    /// The measurement the redesign exists for. The scrolling part of a screen reports how
    /// far it went, whatever the rest of the screen is doing.
    #[test]
    fn a_region_that_scrolled_reports_how_far() {
        let before = frame(split_page(100, 0), 512, 320);
        let after = frame(split_page(100, 16), 512, 320);

        // Well below the frozen band. Logical size equals pixel size here, so a row is a
        // point.
        let region = LogicalRect::new(0.0, 160.0, 512.0, 120.0);
        let shift =
            shift_within(&before, &after, region).expect("a scrolling region is measurable");

        assert!(
            (shift.dy + 16.0).abs() <= 1.0,
            "expected about -16 points, got {}",
            shift.dy
        );
    }

    /// The other half of the same measurement, and the one the display-wide version got
    /// wrong. A region that did not move reports that it did not move, even though most of
    /// the screen around it did.
    #[test]
    fn a_region_that_did_not_move_reports_nothing_moved() {
        let before = frame(split_page(100, 0), 512, 320);
        let after = frame(split_page(100, 16), 512, 320);

        let region = LogicalRect::new(0.0, 10.0, 512.0, 80.0);
        let shift = shift_within(&before, &after, region).expect("a still region is measurable");

        assert!(
            shift.is_negligible(),
            "a region inside the frozen band must read as still, got {shift:?}"
        );
    }

    /// A region spanning both sides of a boundary settles on neither.
    ///
    /// Sixty rows of this region are still and a hundred scrolled, so "nothing moved" and
    /// "everything moved by sixteen" each explain part of it and neither explains most.
    /// Measured, the two score within half a percent of each other, and the estimator
    /// reports no movement rather than committing to a coin toss. That is the safe half of
    /// the answer: a mark that stays put is at least somewhere the client can see.
    ///
    /// It is not the whole answer, because a mark left on content that scrolled away is
    /// still pointing at the wrong thing. What catches that is [`crate::fingerprint`],
    /// which [`crate::daemon`] now consults even when the measurement comes back zero, for
    /// exactly this case. On the recorded corpus that check caught ten left behind marks
    /// in twelve.
    ///
    /// What would fix it here rather than downstream is measuring the halves of the region
    /// separately and refusing when they disagree, which is the decisiveness guard applied
    /// to a division that can see a horizontal boundary.
    #[test]
    fn a_region_straddling_a_boundary_reports_no_movement() {
        let before = frame(split_page(100, 0), 512, 320);
        let after = frame(split_page(100, 16), 512, 320);

        // Sixty rows of the still band and a hundred of the part that scrolled.
        let region = LogicalRect::new(0.0, 40.0, 512.0, 160.0);
        let shift = shift_within(&before, &after, region).expect("an answer, and it is zero");
        assert!(
            shift.is_negligible(),
            "a split region must not commit to one side, got {shift:?}"
        );
    }

    #[test]
    fn a_region_off_the_edge_of_the_frame_is_refused() {
        let before = frame(split_page(100, 0), 512, 320);
        let after = frame(split_page(100, 16), 512, 320);
        assert_eq!(
            shift_within(&before, &after, LogicalRect::new(0.0, 0.0, 0.0, 0.0)),
            None
        );
    }

    #[test]
    fn frames_of_different_sizes_cannot_be_compared() {
        let before = frame(split_page(100, 0), 512, 320);
        let after = frame(landscape(640, 400, 0), 640, 400);
        assert_eq!(
            shift_within(&before, &after, LogicalRect::new(0.0, 0.0, 100.0, 100.0)),
            None
        );
    }
}
