//! The shape of a mark drawn by hand, computed once so every renderer draws the same one.
//!
//! A `highlight` outlines a region, and there are two ways to draw that outline. A ruled
//! one is the rectangle a renderer strokes straight from the anchor. A sketched one is
//! this: a loop around the region carrying the wobble and the overshoot of a pen, built
//! here as an ordinary polyline so a renderer strokes it exactly as it strokes a freehand
//! path.
//!
//! The geometry lives in core for [`crate::arrow`]'s reason. Three renderers would
//! otherwise hold three opinions about what a circled region looks like, and the two that
//! nobody was looking at would drift.
//!
//! Everything here is a function of the region, so the same rect always yields the same
//! loop. That is not tidiness. A mark is redrawn every time the content under it scrolls,
//! and a shape that re-rolled its wobble on each redraw would shimmer for as long as the
//! page was moving.

use arin_protocol::{LogicalPoint, LogicalRect};
use std::f64::consts::TAU;

/// How a mark is drawn.
///
/// Whoever runs the daemon chooses this, not the client, for the reason the palette works
/// that way: it is how a mark looks rather than what it means, and a client that could
/// pick would make every client a typographer. [`arin_protocol::TextboxStyle`] is on the
/// wire because `note` against `guide` says what a box is *for*, which is the client's to
/// know and this is not.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MarkStyle {
    /// Drawn by hand: a wobbled loop with its ends crossing.
    #[default]
    Sketch,
    /// Drawn against a straightedge: the plain rounded rectangle.
    Ruled,
}

/// Straight segments the loop is sampled into.
///
/// Enough that the curve has no visible corners on the largest region anyone would circle.
/// These points never travel on the wire, so the count costs nothing but renderer vertices.
const SEGMENTS: usize = 72;

/// How far past a full turn the stroke carries on, in radians.
///
/// The ends crossing is the strongest evidence a loop was drawn rather than placed, and it
/// is what a hand actually does: the pen comes back to where it started while it is still
/// moving, and leaves before it can stop.
const OVERSHOOT: f64 = 0.22;

/// Gap between the region and the ink, as a fraction of the region's short side.
const CLEARANCE_FRACTION: f64 = 0.15;
/// The tightest the loop may sit to what it circles, in logical points.
const CLEARANCE_MIN: f64 = 6.0;
/// The loosest, so a large region is not circled from far out in the margins.
const CLEARANCE_MAX: f64 = 18.0;

/// Wobble amplitude as a fraction of the region's short half-axis.
///
/// The short one, not the mean. A line of text is circled from a few points above and
/// below it, and an amplitude taken from its length would put the ink in the lines either
/// side of the one being marked.
const WOBBLE_FRACTION: f64 = 0.08;
/// Below this the wobble stops reading as a hand and starts reading as a clean curve.
const WOBBLE_MIN: f64 = 2.0;
/// Above this it stops reading as a hand and starts reading as a mistake.
const WOBBLE_MAX: f64 = 6.0;

/// How far the loop opens out on its way round, in logical points.
const OPENING: f64 = 4.0;
/// Where the tail finishes, inside the line it started on, as a fraction of [`OPENING`].
///
/// Half rather than all of it, so the ink that ends up inside the loop stays clear of what
/// the loop is drawn around.
const CLOSING: f64 = 0.5;

/// How far the loop's centre sits off the region's, in logical points.
const DRIFT: f64 = 2.0;

/// The region size that carries the full share of the fixed roughening, in logical points.
///
/// The wobble, the drift and the opening are all absolute, which is right for anything the
/// size of a button and wrong for a favicon: two points of drift is an eighth of a sixteen
/// point icon, and four points of opening a quarter of it, so the loop stops looking like
/// it is centred on the thing it circles. Below this they scale down with the region.
const ROUGH_FULL: f64 = 40.0;

/// How far toward enclosing the region's corners the loop is inflated.
///
/// A loop drawn through the corners has to sit a long way out, and one that ignores them
/// cuts across whatever is in them, which on a paragraph is the first line. Neither is
/// what a person draws. Most of the way, so a mark clips a corner at worst, never a word.
const CONTAINMENT: f64 = 0.6;
/// The most that inflation may add on one axis, in logical points.
///
/// It is a fraction of the region, so without a ceiling a highlight over half the screen
/// would be circled from a hundred points outside it and run off the display. A large
/// region's corners are also the ones least likely to have anything in them, so this is
/// the case that can afford to give them up.
const CORNER_MAX: f64 = 48.0;

/// The exponent for a region as tall as it is wide: a plain ellipse.
const ROUND_EXPONENT: f64 = 2.0;
/// The exponent for a region at or past [`FLAT_ASPECT`].
const FLAT_EXPONENT: f64 = 3.0;
/// The aspect ratio by which the shoulders have flattened as far as they go.
const FLAT_ASPECT: f64 = 6.0;

/// A loop around `rect`, as a stroked polyline.
///
/// Starts at a seeded angle, runs clockwise on screen, and carries on past its own start
/// so the ends cross. The caller strokes it as it would any path.
pub fn encircle(rect: LogicalRect) -> Vec<LogicalPoint> {
    let seed = seed(rect);
    let half_width = rect.width / 2.0;
    let half_height = rect.height / 2.0;

    let exponent = exponent(rect);
    let clearance = clearance(rect);
    // A superellipse of this exponent passing through the region's corners needs its radii
    // scaled by this much. A higher exponent needs less of it, which is what stops a long
    // region from having to balloon to cover its own ends.
    let corners = CONTAINMENT * (2f64.powf(1.0 / exponent) - 1.0);
    let radius_x = half_width + (half_width * corners).min(CORNER_MAX) + clearance;
    let radius_y = half_height + (half_height * corners).min(CORNER_MAX) + clearance;

    // What share of the fixed roughening this region can carry without the loop ceasing to
    // read as centred on it.
    let rough = (rect.width.min(rect.height) / ROUGH_FULL).clamp(0.0, 1.0);
    let amplitude =
        (half_width.min(half_height) * WOBBLE_FRACTION).clamp(WOBBLE_MIN, WOBBLE_MAX) * rough;

    // Never exactly concentric with what it circles. A loop registered perfectly on its
    // target is the one thing a hand never manages.
    let drift = DRIFT * rough;
    let centre_x = rect.x + half_width + drift * phase(seed, 4).cos();
    let centre_y = rect.y + half_height + drift * phase(seed, 5).cos();

    // Where the pen lands. Seeded rather than fixed, so two highlights on one screen do
    // not cross their ends in the same place as each other.
    let start = phase(seed, 3);

    let mut points = Vec::with_capacity(SEGMENTS + 1);
    for step in 0..=SEGMENTS {
        let along = step as f64 / SEGMENTS as f64;
        let theta = start + along * (TAU + OVERSHOOT);
        let (base_x, base_y) = superellipse(theta, radius_x, radius_y, exponent);

        // Outward along the curve's own radius, so the wobble reads as the pen wandering
        // off the line it is following rather than as the whole loop breathing.
        let reach = base_x.hypot(base_y);
        let deviation = amplitude * wobble(theta, seed) + opening(along) * rough;
        let (offset_x, offset_y) = if reach == 0.0 {
            (0.0, 0.0)
        } else {
            (base_x / reach * deviation, base_y / reach * deviation)
        };

        points.push(LogicalPoint::new(
            centre_x + base_x + offset_x,
            centre_y + base_y + offset_y,
        ));
    }
    points
}

/// How far off the base curve the stroke sits at `along`, in logical points.
///
/// This is what makes the ends cross. The loop opens outward as it goes round, then dives
/// back inside over the overshoot, so the closing stroke passes through the opening one
/// rather than running alongside it. Carrying on past the start is not enough on its own:
/// two arms offset the same way never meet, however far the pen travels.
fn opening(along: f64) -> f64 {
    let turn = TAU / (TAU + OVERSHOOT);
    if along <= turn {
        along / turn * OPENING
    } else {
        let through = (along - turn) / (1.0 - turn);
        OPENING - through * OPENING * (1.0 + CLOSING)
    }
}

/// A point on a superellipse at `theta`.
///
/// At an exponent of two this is a plain ellipse. Raising it squares off the shoulders
/// while leaving the curve smooth, which is the only knob here that changes the shape
/// rather than roughening it.
fn superellipse(theta: f64, radius_x: f64, radius_y: f64, exponent: f64) -> (f64, f64) {
    let (sin, cos) = theta.sin_cos();
    let power = 2.0 / exponent;
    (
        radius_x * cos.signum() * cos.abs().powf(power),
        radius_y * sin.signum() * sin.abs().powf(power),
    )
}

/// How square the loop's shoulders are, from how elongated the region is.
///
/// A near-square region gets a plain ellipse, which is what circling a button looks like.
/// A long, thin one gets flatter sides, because an ellipse drawn around a line of text has
/// to balloon a long way above and below it to clear the ends, and what it balloons into
/// is the lines above and below.
fn exponent(rect: LogicalRect) -> f64 {
    let short = rect.width.min(rect.height);
    if short <= 0.0 {
        return ROUND_EXPONENT;
    }
    let aspect = (rect.width.max(rect.height) / short).clamp(1.0, FLAT_ASPECT);
    let along = (aspect - 1.0) / (FLAT_ASPECT - 1.0);
    ROUND_EXPONENT + along * (FLAT_EXPONENT - ROUND_EXPONENT)
}

/// How far outside the region the ink sits.
///
/// A person circling a word draws around it rather than over it, so the loop clears what
/// it marks. Scaled to the region with both ends pinned: proportional alone would put a
/// tall region's loop miles out and a short one's straight through the text.
fn clearance(rect: LogicalRect) -> f64 {
    (rect.width.min(rect.height) * CLEARANCE_FRACTION).clamp(CLEARANCE_MIN, CLEARANCE_MAX)
}

/// Smooth, non-repeating deviation from the base curve, in `[-1, 1]`.
///
/// Three sinusoids rather than per-vertex randomness, which reads as static rather than as
/// a hand. The frequencies are deliberately not whole numbers: a wobble that repeated
/// every turn would lay the overshoot exactly along the start of the stroke, and the
/// crossed ends are most of what makes the loop read as drawn.
fn wobble(theta: f64, seed: u64) -> f64 {
    0.5 * (theta * 1.3 + phase(seed, 0)).sin()
        + 0.3 * (theta * 2.1 + phase(seed, 1)).sin()
        + 0.2 * (theta * 3.7 + phase(seed, 2)).sin()
}

/// A seed taken from the region itself, so a loop is reproducible without carrying one.
///
/// Rounded to whole points first. A difference smaller than a point is finer than the
/// wobble could express anyway, and rounding keeps two ways of describing the same region
/// from drawing visibly different loops.
fn seed(rect: LogicalRect) -> u64 {
    let mut hash = 0x9E37_79B9_7F4A_7C15;
    for value in [rect.x, rect.y, rect.width, rect.height] {
        hash = mix(hash ^ (value.round() as i64 as u64));
    }
    hash
}

/// splitmix64's finaliser, which is enough to turn four coordinates into unrelated phases.
const fn mix(hash: u64) -> u64 {
    let hash = (hash ^ (hash >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let hash = (hash ^ (hash >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    hash ^ (hash >> 31)
}

/// One of the seed's phases, in `[0, TAU)`.
fn phase(seed: u64, index: u32) -> f64 {
    let bits = mix(seed ^ u64::from(index).wrapping_mul(0x517C_C1B7_2722_0A95));
    // The top 53, which is what an f64 mantissa holds without rounding.
    (bits >> 11) as f64 / (1u64 << 53) as f64 * TAU
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A button: near enough square that the loop should be a plain oval.
    const BUTTON: LogicalRect = LogicalRect::new(400.0, 300.0, 120.0, 40.0);
    /// A line of text, which is the case an ellipse handles badly.
    const LINE: LogicalRect = LogicalRect::new(100.0, 200.0, 340.0, 20.0);

    fn bounds(points: &[LogicalPoint]) -> LogicalRect {
        let left = points.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
        let top = points.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
        let right = points.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
        let bottom = points.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);
        LogicalRect::new(left, top, right - left, bottom - top)
    }

    #[test]
    fn the_same_region_always_draws_the_same_loop() {
        assert_eq!(encircle(BUTTON), encircle(BUTTON));
        assert_eq!(encircle(LINE), encircle(LINE));
    }

    #[test]
    fn different_regions_draw_different_loops() {
        let other = LogicalRect::new(BUTTON.x + 3.0, BUTTON.y, BUTTON.width, BUTTON.height);
        assert_ne!(encircle(BUTTON), encircle(other));
    }

    /// Where a region is counts towards its seed, not just how big it is. Circling three
    /// buttons of one size should not stamp the same wobble three times, which is what
    /// gives away a shape that was generated rather than drawn.
    #[test]
    fn regions_of_one_size_are_circled_differently() {
        let elsewhere = LogicalRect::new(
            BUTTON.x + 260.0,
            BUTTON.y + 90.0,
            BUTTON.width,
            BUTTON.height,
        );
        let here = encircle(BUTTON);
        let there = encircle(elsewhere);

        // Same size, so the loops are the same size, give or take the wobble.
        let slack = WOBBLE_MAX + DRIFT;
        assert!((bounds(&here).width - bounds(&there).width).abs() < slack);

        // Shifted back on top of each other, no two vertices should agree.
        let (dx, dy) = (elsewhere.x - BUTTON.x, elsewhere.y - BUTTON.y);
        assert!(
            here.iter()
                .zip(&there)
                .all(|(a, b)| (a.x + dx - b.x).abs() > 1e-9 || (a.y + dy - b.y).abs() > 1e-9),
            "two regions of one size were circled with the same stroke"
        );
    }

    #[test]
    fn every_vertex_is_finite() {
        for rect in [BUTTON, LINE, LogicalRect::new(0.0, 0.0, 1.0, 1.0)] {
            assert!(encircle(rect).iter().all(LogicalPoint::is_finite));
        }
    }

    /// A person circles around a word, not through it, so the ink clears what it marks.
    #[test]
    fn the_loop_encloses_the_region_it_circles() {
        let around = bounds(&encircle(BUTTON));
        assert!(around.x < BUTTON.x, "left edge is inside the region");
        assert!(around.y < BUTTON.y, "top edge is inside the region");
        assert!(around.x + around.width > BUTTON.x + BUTTON.width);
        assert!(around.y + around.height > BUTTON.y + BUTTON.height);
    }

    /// Loose enough to read as drawn, tight enough that it still points at one thing.
    ///
    /// The lower bound is [`the_loop_encloses_the_region_it_circles`]. This is the other
    /// end: the wobble and the drift both push outward as well as in, so the furthest the
    /// ink can land is the clearance plus the pair of them.
    #[test]
    fn the_loop_stays_near_what_it_circles() {
        for rect in [BUTTON, LINE, LogicalRect::new(0.0, 0.0, 900.0, 700.0)] {
            let around = bounds(&encircle(rect));
            let margin = rect.x - around.x;
            let reach = CORNER_MAX + CLEARANCE_MAX + WOBBLE_MAX + DRIFT + OPENING;
            assert!(
                margin <= reach,
                "the loop sits {margin} points clear of a {} point region",
                rect.width
            );
        }
    }

    /// An ellipse around a line of text has to balloon to clear the ends. The flattened
    /// shoulders are what keep it off the lines above and below.
    #[test]
    fn a_long_region_is_circled_without_ballooning() {
        let around = bounds(&encircle(LINE));
        let ellipse_would_need = LINE.height / 2.0 * std::f64::consts::SQRT_2;
        let bulge = LINE.y - around.y;
        assert!(
            bulge < ellipse_would_need + CLEARANCE_MAX,
            "a line of text is circled from {bulge} points above it"
        );
    }

    /// Whether two segments properly cross, sharing neither endpoint.
    fn crosses(a: [LogicalPoint; 2], b: [LogicalPoint; 2]) -> bool {
        let side = |p: LogicalPoint, q: LogicalPoint, r: LogicalPoint| {
            (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x)
        };
        let (d1, d2) = (side(b[0], b[1], a[0]), side(b[0], b[1], a[1]));
        let (d3, d4) = (side(a[0], a[1], b[0]), side(a[0], a[1], b[1]));
        (d1 * d2) < 0.0 && (d3 * d4) < 0.0
    }

    /// A favicon is a sixteenth of the size the fixed roughening was picked against, so
    /// carrying all of it puts the loop visibly off the thing it circles. Small marks scale
    /// it down instead. Measured as how far the loop's middle sits off the region's.
    #[test]
    fn a_small_region_is_circled_around_its_middle() {
        for side in [16.0, 24.0, 40.0, 120.0] {
            let rect = LogicalRect::new(400.0, 300.0, side, side);
            let ink = bounds(&encircle(rect));
            let off = (ink.center().x - rect.center().x)
                .abs()
                .max((ink.center().y - rect.center().y).abs());
            assert!(
                off < side * 0.1,
                "a {side} point region is circled {off} points off centre"
            );
        }
    }

    /// The ends crossing is what says a hand drew this rather than a renderer placing it.
    ///
    /// Asserted as a real intersection rather than as a gap between the endpoints. A loop
    /// that merely overshot would pass this on distance while running neatly alongside
    /// itself, which is exactly the shape this is here to rule out.
    #[test]
    fn the_ends_of_the_stroke_cross() {
        for rect in [BUTTON, LINE, LogicalRect::new(400.0, 300.0, 16.0, 16.0)] {
            let points = encircle(rect);
            let tail: Vec<[LogicalPoint; 2]> = points[points.len() - 6..]
                .windows(2)
                .map(|w| [w[0], w[1]])
                .collect();
            let head: Vec<[LogicalPoint; 2]> =
                points[..8].windows(2).map(|w| [w[0], w[1]]).collect();

            assert!(
                tail.iter().any(|t| head.iter().any(|h| crosses(*t, *h))),
                "the stroke closed without crossing itself on a {} by {} region",
                rect.width,
                rect.height
            );
        }
    }

    /// Squarish regions get a plain ellipse, elongated ones get flatter sides, and nothing
    /// goes past either end however extreme the region.
    #[test]
    fn shoulders_flatten_with_the_region() {
        assert_eq!(
            exponent(LogicalRect::new(0.0, 0.0, 50.0, 50.0)),
            ROUND_EXPONENT
        );
        assert_eq!(exponent(LINE), FLAT_EXPONENT);
        assert_eq!(
            exponent(LogicalRect::new(0.0, 0.0, 4000.0, 10.0)),
            FLAT_EXPONENT
        );
        let middling = exponent(LogicalRect::new(0.0, 0.0, 150.0, 50.0));
        assert!((ROUND_EXPONENT..FLAT_EXPONENT).contains(&middling));
    }
}
