//! Choosing an annotation colour that can actually be seen.
//!
//! A fixed colour is wrong somewhere. Amber on a dark editor is excellent and amber on an
//! amber warning banner is invisible, and the daemon cannot know which it is looking at
//! without looking. So it samples the region it is about to draw over and picks from a
//! small palette.
//!
//! # Why the median and neither the mean nor the worst case
//!
//! Two obvious scorings both fail, and they fail on measurements rather than in theory.
//!
//! Contrast against the region's *average* colour is skewed by whatever is brightest.
//! Dark text on a light page averages to mid grey, and the colour that best contrasts
//! with mid grey can vanish into both the text and the paper.
//!
//! Contrast against the *worst* sample is worse still, and it is the more tempting of the
//! two. Any region of real interface contains something near black and something near
//! white: sampling a dark settings panel measured luminances from 0.00 to 0.96, at which
//! point every candidate in the palette scores about 1.0 for its worst pixel and the
//! winner is decided by noise. It is a statistic with no signal left in it.
//!
//! What is scored is the *median*: the contrast a candidate achieves against a typical
//! pixel of the region. On that same panel it separates the palette cleanly, from 1.2 for
//! near black up to 14.8 for near white, with amber at 8.8. An annotation is drawn over
//! the bulk of a region and crosses its outliers, so the bulk is what should decide.
//!
//! # Scoring where the ink goes, not where the mark was asked for
//!
//! A highlight is an outline. Its interior is never painted, so sampling the whole
//! rectangle answers a question nobody asked: what matters is what sits under the stroke.
//! A freehand path is worse, since the bounding box of a diagonal line is mostly pixels
//! the stroke never touches.
//!
//! Each mark therefore reports a [`Footprint`], and the parts of that footprint are
//! scored *independently*, with the worst part deciding. That is what catches a thin
//! element running along one edge of an outline: a small share of the region, and most of
//! the ink on that edge.
//!
//! How many parts is bounded, and that bound is what makes the minimum mean anything. An
//! outline has four edges. A path is cut into four chunks of equal length, however many
//! segments the client actually sent: scoring every segment would be the worst-case
//! statistic again by another name, back to every candidate scoring about 1.0.
//!
//! Four is enough to stop a long stretch over one background deciding for a shorter
//! stretch over a very different one, which is how a line that runs along a coloured band
//! and then leaves it ends up invisible for its last third. It is small enough that a
//! brief crossing inside any one chunk is still outvoted by the median.
//!
//! # Why blue is never a candidate
//!
//! Blue belongs to the orb. An annotation in the orb's own colour reads as part of the
//! orb rather than as a separate mark, and the whole visual grammar rests on those being
//! two different things.

use crate::traits::Frame;
use arin_protocol::{LogicalPoint, LogicalRect};

/// A colour, as the renderer wants it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb {
    /// Red, `0..=255`.
    pub r: u8,
    /// Green, `0..=255`.
    pub g: u8,
    /// Blue, `0..=255`.
    pub b: u8,
}

impl Rgb {
    /// Construct from components.
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Parse `#RRGGBB`.
    ///
    /// Returns `None` for anything else, so a malformed colour falls back to the default
    /// rather than to something arbitrary. A mark in an unexpected colour is harder to
    /// notice than one in the usual one.
    pub fn parse(hex: &str) -> Option<Self> {
        let hex = hex.strip_prefix('#')?;
        if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let component = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
        Some(Self::new(component(0)?, component(2)?, component(4)?))
    }

    /// Components in `0.0..=1.0`, which is what the platform colour APIs take.
    pub fn as_unit(self) -> (f64, f64, f64) {
        (
            f64::from(self.r) / 255.0,
            f64::from(self.g) / 255.0,
            f64::from(self.b) / 255.0,
        )
    }

    /// WCAG relative luminance.
    fn luminance(self) -> f64 {
        fn channel(value: u8) -> f64 {
            let v = f64::from(value) / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(self.r) + 0.7152 * channel(self.g) + 0.0722 * channel(self.b)
    }

    /// WCAG contrast ratio against another colour, from 1.0 to 21.0.
    pub fn contrast(self, other: Self) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        let (lighter, darker) = if a > b { (a, b) } else { (b, a) };
        (lighter + 0.05) / (darker + 0.05)
    }

    /// Hue in degrees, `0..360`. Undefined for greys, which report zero.
    fn hue(self) -> f64 {
        let (r, g, b) = self.as_unit();
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        if delta <= f64::EPSILON {
            return 0.0;
        }
        let hue = if max == r {
            60.0 * (((g - b) / delta) % 6.0)
        } else if max == g {
            60.0 * ((b - r) / delta + 2.0)
        } else {
            60.0 * ((r - g) / delta + 4.0)
        };
        if hue < 0.0 { hue + 360.0 } else { hue }
    }

    /// How saturated, `0.0..=1.0`. A grey is zero.
    fn saturation(self) -> f64 {
        let (r, g, b) = self.as_unit();
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        if max <= f64::EPSILON {
            0.0
        } else {
            (max - min) / max
        }
    }

    /// The `#RRGGBB` spelling, which is also how a client would have named it.
    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// Whether this sits in the hue range the orb owns.
    ///
    /// A desaturated colour has no meaningful hue, so a grey is never "blue" however its
    /// components happen to fall.
    pub fn is_blue_family(self) -> bool {
        const BLUE: std::ops::Range<f64> = 185.0..265.0;
        self.saturation() > 0.2 && BLUE.contains(&self.hue())
    }
}

impl std::fmt::Display for Rgb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// The colour used when nothing better can be determined.
///
/// First in the palette, so it also wins any tie. Amber over a dark editor is the case
/// Arin is used in most, and staying put unless something genuinely beats it keeps the
/// look consistent between marks.
pub const DEFAULT: Rgb = Rgb::new(0xFF, 0xB0, 0x20);

/// What the daemon will choose between.
///
/// Deliberately short. Every entry has to be recognisable as "an annotation" rather than
/// as part of the interface underneath, which rules out most of the colour wheel, and
/// none of them may be blue.
pub const PALETTE: &[Rgb] = &[
    DEFAULT,                    // amber
    Rgb::new(0xFF, 0x3B, 0x30), // red
    Rgb::new(0xFF, 0x2D, 0x95), // magenta
    Rgb::new(0x30, 0xD1, 0x58), // green
    Rgb::new(0xF5, 0xF5, 0xF7), // near white
    Rgb::new(0x1C, 0x1C, 0x1E), // near black
];

/// Samples taken across a filled area, per axis.
///
/// A 16 by 16 grid is 256 reads, which is nothing next to the capture that produced the
/// frame, and enough to notice a band of text crossing the region.
const GRID: usize = 16;

/// Samples taken along a stroke.
const ALONG: usize = 48;

/// How many parts a path is cut into, at most.
///
/// Bounded on purpose, and four to match the four edges of an outline. Scoring every
/// segment of a freehand path would be the worst-case statistic by another name, which
/// has no signal in it. Scoring the path as one would let a long stretch over one
/// background decide for a shorter stretch over a very different one, and the shorter
/// stretch then goes invisible. A handful of chunks keeps the minimum meaningful while
/// still outvoting a brief crossing inside any one of them.
const PATH_PARTS: usize = 4;

/// Samples taken across a stroke's width.
///
/// Three: one either side of the centreline and one on it. A stroke is a few points wide
/// and the frame is usually downscaled below that, so asking for more would be reading
/// the same pixel repeatedly.
const ACROSS: usize = 3;

/// How wide the daemon considers its own strokes when deciding what is under them.
///
/// Shared with the renderers so the two cannot drift: sampling a three point band while
/// drawing a one point line would judge the wrong pixels.
pub const STROKE_WIDTH: f64 = 3.0;

/// Where a mark actually puts ink.
///
/// Built by the daemon from what it is about to draw, and scored a part at a time. See
/// the module docs for why the parts matter.
#[derive(Debug, Clone, PartialEq)]
pub enum Footprint {
    /// A filled region: a text box panel, or the area the orb settles into.
    Area(LogicalRect),
    /// The border of a region, and nothing inside it.
    Outline {
        /// The region being outlined.
        rect: LogicalRect,
        /// Stroke width in logical points.
        width: f64,
    },
    /// A stroked path through these points.
    Path {
        /// Ordered vertices in logical points.
        points: Vec<LogicalPoint>,
        /// Stroke width in logical points.
        width: f64,
    },
}

impl Footprint {
    /// The sample positions, grouped into independently scored parts.
    ///
    /// Pure geometry, so the awkward cases are testable without a screen.
    fn parts(&self) -> Vec<Vec<LogicalPoint>> {
        match self {
            Self::Area(rect) => vec![grid(*rect)],
            Self::Outline { rect, width } => outline_bands(*rect, *width),
            Self::Path { points, width } => stroke(points, *width),
        }
    }
}

/// Points spread evenly over a filled region.
fn grid(rect: LogicalRect) -> Vec<LogicalPoint> {
    let mut points = Vec::with_capacity(GRID * GRID);
    for row in 0..GRID {
        // The middle of each band rather than its edge, so a one pixel shift does not
        // move every sample onto a boundary at once.
        let y = rect.y + (row as f64 + 0.5) * rect.height / GRID as f64;
        for col in 0..GRID {
            let x = rect.x + (col as f64 + 0.5) * rect.width / GRID as f64;
            points.push(LogicalPoint::new(x, y));
        }
    }
    points
}

/// The four edges of an outline, each its own part.
///
/// Bands rather than lines: the stroke has width, and a mark is unreadable if what sits
/// under any part of it matches. Corners fall in two bands, which costs a few duplicate
/// reads and saves reasoning about which edge owns them.
fn outline_bands(rect: LogicalRect, width: f64) -> Vec<Vec<LogicalPoint>> {
    let width = width.max(1.0);
    let (x0, y0) = (rect.x, rect.y);
    let (x1, y1) = (rect.x + rect.width, rect.y + rect.height);

    // Inset by half a stroke so the band sits where the border is drawn, straddling the
    // edge rather than hanging outside it.
    let half = width / 2.0;
    vec![
        band(
            LogicalPoint::new(x0, y0 + half),
            LogicalPoint::new(x1, y0 + half),
            width,
        ),
        band(
            LogicalPoint::new(x0, y1 - half),
            LogicalPoint::new(x1, y1 - half),
            width,
        ),
        band(
            LogicalPoint::new(x0 + half, y0),
            LogicalPoint::new(x0 + half, y1),
            width,
        ),
        band(
            LogicalPoint::new(x1 - half, y0),
            LogicalPoint::new(x1 - half, y1),
            width,
        ),
    ]
}

/// Sample positions filling a band of `width` along a segment.
fn band(from: LogicalPoint, to: LogicalPoint, width: f64) -> Vec<LogicalPoint> {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let length = (dx * dx + dy * dy).sqrt();
    if length <= f64::EPSILON {
        return vec![from];
    }
    // Unit normal, which is the direction the stroke has thickness in.
    let (nx, ny) = (-dy / length, dx / length);

    let mut points = Vec::with_capacity(ALONG * ACROSS);
    for step in 0..ALONG {
        let t = (step as f64 + 0.5) / ALONG as f64;
        let (x, y) = (from.x + dx * t, from.y + dy * t);
        for lane in 0..ACROSS {
            let offset = (lane as f64 / (ACROSS - 1).max(1) as f64 - 0.5) * width;
            points.push(LogicalPoint::new(x + nx * offset, y + ny * offset));
        }
    }
    points
}

/// Sample positions along a path, cut into at most [`PATH_PARTS`] parts of equal length.
///
/// Spread by length rather than by vertex, so a path made of one long segment and twenty
/// short ones is not sampled almost entirely inside the short ones.
fn stroke(points: &[LogicalPoint], width: f64) -> Vec<Vec<LogicalPoint>> {
    let width = width.max(1.0);
    if points.len() < 2 {
        return points
            .first()
            .copied()
            .map(|only| vec![vec![only]])
            .unwrap_or_default();
    }

    let lengths: Vec<f64> = points
        .windows(2)
        .map(|pair| {
            let (dx, dy) = (pair[1].x - pair[0].x, pair[1].y - pair[0].y);
            (dx * dx + dy * dy).sqrt()
        })
        .collect();
    let total: f64 = lengths.iter().sum();
    if total <= f64::EPSILON {
        return vec![vec![points[0]]];
    }

    let per_part = ALONG * ACROSS / PATH_PARTS + ACROSS;
    let mut parts: Vec<Vec<LogicalPoint>> = (0..PATH_PARTS)
        .map(|_| Vec::with_capacity(per_part))
        .collect();
    for step in 0..ALONG {
        let fraction = (step as f64 + 0.5) / ALONG as f64;
        let part = ((fraction * PATH_PARTS as f64) as usize).min(PATH_PARTS - 1);
        let target = fraction * total;
        let mut travelled = 0.0;
        let mut index = 0;
        while index + 1 < lengths.len() && travelled + lengths[index] < target {
            travelled += lengths[index];
            index += 1;
        }
        let (from, to) = (points[index], points[index + 1]);
        let span = lengths[index];
        let t = if span <= f64::EPSILON {
            0.0
        } else {
            ((target - travelled) / span).clamp(0.0, 1.0)
        };
        let (dx, dy) = (to.x - from.x, to.y - from.y);
        let (nx, ny) = if span <= f64::EPSILON {
            (0.0, 0.0)
        } else {
            (-dy / span, dx / span)
        };
        let (x, y) = (from.x + dx * t, from.y + dy * t);
        for lane in 0..ACROSS {
            let offset = (lane as f64 / (ACROSS - 1).max(1) as f64 - 0.5) * width;
            parts[part].push(LogicalPoint::new(x + nx * offset, y + ny * offset));
        }
    }
    parts.retain(|part| !part.is_empty());
    parts
}

/// The contrast a mark needs before it counts as legible.
///
/// Three to one, which is the accepted floor for a graphical object as opposed to body
/// text. Above this a mark is comfortably visible and there is nothing to fix.
const LEGIBLE: f64 = 3.0;

/// Pick the colour to draw a mark in.
///
/// The question asked is "can the usual colour be seen everywhere this mark is drawn",
/// not "which colour scores highest". Those give different answers and the second one is
/// wrong: on a dark editor white outscores amber by a wide margin, but amber is already
/// legible there at better than eight to one, and swapping it for white would trade a
/// mark that reads as an annotation for one that reads as more interface. The palette
/// moves when the default genuinely fails, and not to chase a number.
///
/// Within a part the score is the median, so a minority of awkward pixels is outvoted.
/// Across parts it is the worst, so an edge that has gone invisible is not.
///
/// Returns [`DEFAULT`] when the frame is empty or unreadable, since a mark in the usual
/// colour beats no mark at all.
pub fn pick(frame: &Frame, footprint: &Footprint) -> Rgb {
    let parts: Vec<Vec<Rgb>> = footprint
        .parts()
        .iter()
        .map(|positions| read(frame, positions))
        .filter(|samples| !samples.is_empty())
        .collect();
    if parts.is_empty() {
        return DEFAULT;
    }

    // The typical pixel of the least forgiving part. See the module docs for why the
    // median within a part and the minimum across them.
    let score = |candidate: &Rgb| {
        parts
            .iter()
            .map(|samples| {
                let mut scores: Vec<f64> = samples.iter().map(|s| candidate.contrast(*s)).collect();
                scores.sort_by(f64::total_cmp);
                scores[scores.len() / 2]
            })
            .fold(f64::INFINITY, f64::min)
    };

    if score(&DEFAULT) >= LEGIBLE {
        return DEFAULT;
    }

    PALETTE
        .iter()
        .copied()
        .max_by(|a, b| score(a).total_cmp(&score(b)))
        .unwrap_or(DEFAULT)
}

/// Read the frame at a set of logical positions.
///
/// The positions arrive in logical points and the frame is in physical pixels at whatever
/// size the capture backend produced, which is not necessarily the display's own scale: a
/// downscaled capture is both cheaper and perfectly good for averaging colour. So the
/// mapping goes through the frame's own dimensions rather than through `frame.scale`.
///
/// Positions off the edge of the frame are clamped rather than dropped. A mark half off
/// the screen still wants a colour, chosen from whichever part of it is on the display.
fn read(frame: &Frame, positions: &[LogicalPoint]) -> Vec<Rgb> {
    let (width, height) = (frame.width as usize, frame.height as usize);
    if width == 0 || height == 0 || frame.pixels.len() < width * height * 4 {
        return Vec::new();
    }
    let [logical_width, logical_height] = frame.logical_size;
    if logical_width <= 0.0 || logical_height <= 0.0 {
        return Vec::new();
    }

    positions
        .iter()
        .filter_map(|at| {
            let x = ((at.x / logical_width) * width as f64).round();
            let y = ((at.y / logical_height) * height as f64).round();
            if !x.is_finite() || !y.is_finite() {
                return None;
            }
            let x = x.clamp(0.0, width as f64 - 1.0) as usize;
            let y = y.clamp(0.0, height as f64 - 1.0) as usize;
            let idx = (y * width + x) * 4;
            // Packed BGRA, as the capture backends document.
            frame
                .pixels
                .get(idx..idx + 4)
                .map(|px| Rgb::new(px[2], px[1], px[0]))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use arin_protocol::DisplayId;
    use std::sync::Arc;

    /// A frame filled with one colour.
    fn flat(color: Rgb) -> Frame {
        let (w, h) = (64usize, 64usize);
        let mut pixels = vec![0u8; w * h * 4];
        for px in pixels.chunks_exact_mut(4) {
            px[0] = color.b;
            px[1] = color.g;
            px[2] = color.r;
            px[3] = 255;
        }
        Frame {
            display: DisplayId(1),
            scale: 1.0,
            logical_size: [w as f64, h as f64],
            width: w as u32,
            height: h as u32,
            pixels: Arc::from(pixels),
        }
    }

    fn whole(frame: &Frame) -> Footprint {
        Footprint::Area(LogicalRect::new(
            0.0,
            0.0,
            frame.logical_size[0],
            frame.logical_size[1],
        ))
    }

    #[test]
    fn no_candidate_is_the_orbs_colour() {
        for candidate in PALETTE {
            assert!(
                !candidate.is_blue_family(),
                "{candidate:?} is in the blue family, which belongs to the orb"
            );
        }
    }

    #[test]
    fn the_orbs_own_blue_is_recognised() {
        // The three the orb is built from.
        assert!(Rgb::new(0x1E, 0x3A, 0x8A).is_blue_family());
        assert!(Rgb::new(0x3B, 0x82, 0xF6).is_blue_family());
        assert!(Rgb::new(0x7F, 0xE3, 0xFF).is_blue_family());
    }

    #[test]
    fn a_grey_is_not_blue_however_its_components_fall() {
        assert!(!Rgb::new(0x80, 0x80, 0x82).is_blue_family());
        assert!(!Rgb::new(0x00, 0x00, 0x00).is_blue_family());
        assert!(!Rgb::new(0xFF, 0xFF, 0xFF).is_blue_family());
    }

    #[test]
    fn contrast_is_symmetric_and_bounded() {
        let black = Rgb::new(0, 0, 0);
        let white = Rgb::new(255, 255, 255);
        assert!((black.contrast(white) - 21.0).abs() < 0.01);
        assert!((white.contrast(black) - 21.0).abs() < 0.01);
        assert!((black.contrast(black) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_dark_editor_keeps_the_default() {
        // The case Arin is used in most. Amber is already excellent here, so nothing
        // should displace it and marks stay consistent.
        let picked = pick(&flat(Rgb::new(0x1E, 0x1E, 0x1E)), &whole(&flat(DEFAULT)));
        assert_eq!(picked, DEFAULT);
    }

    #[test]
    fn amber_on_amber_moves_away() {
        // The failure a fixed colour cannot avoid: a warning banner the same colour as
        // the annotation.
        let frame = flat(DEFAULT);
        let picked = pick(&frame, &whole(&frame));
        assert_ne!(picked, DEFAULT, "amber on amber must not stay amber");
        assert!(
            picked.contrast(DEFAULT) > 2.0,
            "{picked:?} is still too close to the background"
        );
    }

    #[test]
    fn a_white_page_gets_something_readable() {
        let frame = flat(Rgb::new(0xFF, 0xFF, 0xFF));
        let picked = pick(&frame, &whole(&frame));
        assert!(
            picked.contrast(Rgb::new(0xFF, 0xFF, 0xFF)) >= 3.0,
            "{picked:?} is not readable on white"
        );
    }

    /// The case the module docs argue about. The mean of this region is mid grey, which
    /// is a colour that appears nowhere in it, so the answer has to come from the page
    /// the mark is mostly drawn over rather than from an average of page and ink.
    #[test]
    fn dark_text_on_a_light_page_answers_to_the_page() {
        let (w, h) = (64usize, 64usize);
        let mut pixels = vec![0u8; w * h * 4];
        for (i, px) in pixels.chunks_exact_mut(4).enumerate() {
            // Bands of near-black text over near-white paper.
            let dark = (i / w) % 4 == 0;
            let v = if dark { 0x14 } else { 0xF8 };
            px[0] = v;
            px[1] = v;
            px[2] = v;
            px[3] = 255;
        }
        let frame = Frame {
            display: DisplayId(1),
            scale: 1.0,
            logical_size: [w as f64, h as f64],
            width: w as u32,
            height: h as u32,
            pixels: Arc::from(pixels),
        };

        let picked = pick(&frame, &whole(&frame));
        let against_paper = picked.contrast(Rgb::new(0xF8, 0xF8, 0xF8));
        assert!(
            against_paper >= LEGIBLE,
            "{picked:?} scored only {against_paper:.2} against the page it sits on"
        );
        // And amber, which is unreadable on white, must not have been what survived.
        assert_ne!(picked, DEFAULT);
    }

    /// A dark panel with white text on it, which is most of a code editor and was the
    /// case that exposed worst-case scoring as unusable. Amber is plainly legible here
    /// and has to survive the glyphs crossing the region.
    #[test]
    fn white_text_on_a_dark_panel_keeps_the_default() {
        let (w, h) = (64usize, 64usize);
        let mut pixels = vec![0u8; w * h * 4];
        for (i, px) in pixels.chunks_exact_mut(4).enumerate() {
            // A quarter of the rows are text, the rest is panel.
            let ink = (i / w) % 4 == 0;
            let v = if ink { 0xFF } else { 0x1E };
            px[0] = v;
            px[1] = v;
            px[2] = v;
            px[3] = 255;
        }
        let frame = Frame {
            display: DisplayId(1),
            scale: 1.0,
            logical_size: [w as f64, h as f64],
            width: w as u32,
            height: h as u32,
            pixels: Arc::from(pixels),
        };

        assert_eq!(pick(&frame, &whole(&frame)), DEFAULT);
    }

    #[test]
    fn a_region_outside_the_frame_falls_back() {
        let frame = flat(Rgb::new(0x1E, 0x1E, 0x1E));
        let outside = Footprint::Area(LogicalRect::new(9000.0, 9000.0, 10.0, 10.0));
        // Off the edge entirely: clamped to the nearest real pixel rather than refused.
        assert_eq!(pick(&frame, &outside), DEFAULT);
    }

    #[test]
    fn an_empty_frame_falls_back() {
        let frame = Frame {
            display: DisplayId(1),
            scale: 1.0,
            logical_size: [0.0, 0.0],
            width: 0,
            height: 0,
            pixels: Arc::from(Vec::new()),
        };
        assert_eq!(
            pick(
                &frame,
                &Footprint::Area(LogicalRect::new(0.0, 0.0, 10.0, 10.0))
            ),
            DEFAULT
        );
    }

    #[test]
    fn a_truncated_frame_does_not_panic() {
        let frame = Frame {
            display: DisplayId(1),
            scale: 1.0,
            logical_size: [64.0, 64.0],
            width: 64,
            height: 64,
            pixels: Arc::from(vec![0u8; 16]),
        };
        assert_eq!(
            pick(
                &frame,
                &Footprint::Area(LogicalRect::new(0.0, 0.0, 10.0, 10.0))
            ),
            DEFAULT
        );
    }

    /// A frame with a horizontal band of one colour across an otherwise flat background.
    fn banded(background: Rgb, band: Rgb, top: usize, height: usize) -> Frame {
        let (w, h) = (256usize, 256usize);
        let mut pixels = vec![0u8; w * h * 4];
        for y in 0..h {
            let c = if (top..top + height).contains(&y) {
                band
            } else {
                background
            };
            for x in 0..w {
                let idx = (y * w + x) * 4;
                pixels[idx] = c.b;
                pixels[idx + 1] = c.g;
                pixels[idx + 2] = c.r;
                pixels[idx + 3] = 255;
            }
        }
        Frame {
            display: DisplayId(1),
            scale: 1.0,
            logical_size: [w as f64, h as f64],
            width: w as u32,
            height: h as u32,
            pixels: Arc::from(pixels),
        }
    }

    /// The case that motivated all of this. An amber band runs under the top edge of a
    /// highlight, so a quarter of the outline is drawn amber on amber and cannot be seen,
    /// while the region as a whole is overwhelmingly dark and reads as perfectly fine.
    #[test]
    fn an_outline_adapts_when_one_edge_lands_on_its_own_colour() {
        let frame = banded(Rgb::new(0x1E, 0x1E, 0x1E), DEFAULT, 40, 6);
        let rect = LogicalRect::new(20.0, 40.0, 200.0, 160.0);

        // Sampling the whole region sees mostly dark and stays.
        assert_eq!(pick(&frame, &Footprint::Area(rect)), DEFAULT);

        // Sampling the four edges separately notices that the top one has gone.
        let picked = pick(
            &frame,
            &Footprint::Outline {
                rect,
                width: STROKE_WIDTH,
            },
        );
        assert_ne!(picked, DEFAULT, "the top edge is invisible");
        // No palette entry is legible against amber and near black at once: the best
        // worst-part score available here is red at 1.94, against amber's 1.00. So the
        // answer is the best compromise rather than a comfortable margin, and what is
        // being asserted is that it improved on drawing amber over amber.
        assert!(
            picked.contrast(DEFAULT) > 1.5,
            "{picked:?} is barely better than amber on amber"
        );
    }

    /// And an outline nowhere near the band must not be disturbed, or every mark on a
    /// busy screen would end up a different colour.
    #[test]
    fn an_outline_clear_of_the_band_keeps_the_default() {
        let frame = banded(Rgb::new(0x1E, 0x1E, 0x1E), DEFAULT, 40, 6);
        let rect = LogicalRect::new(20.0, 100.0, 200.0, 100.0);
        assert_eq!(
            pick(
                &frame,
                &Footprint::Outline {
                    rect,
                    width: STROKE_WIDTH
                }
            ),
            DEFAULT
        );
    }

    /// A path is scored along its stroke. Its bounding box here is mostly background,
    /// so a line running along the band would keep the default if the box were sampled.
    #[test]
    fn a_path_running_along_its_own_colour_adapts() {
        let frame = banded(Rgb::new(0x1E, 0x1E, 0x1E), DEFAULT, 40, 6);
        // An L: a long run inside the band, then a short spur out of it. Most of the
        // length is on the band, while most of the bounding box is not.
        let along = vec![
            LogicalPoint::new(20.0, 43.0),
            LogicalPoint::new(220.0, 43.0),
            LogicalPoint::new(220.0, 80.0),
        ];

        assert_eq!(
            pick(&frame, &Footprint::Area(bounds_of(&along))),
            DEFAULT,
            "the bounding box is mostly background, which is the whole problem"
        );
        assert_ne!(
            pick(
                &frame,
                &Footprint::Path {
                    points: along.clone(),
                    width: STROKE_WIDTH
                }
            ),
            DEFAULT,
            "most of the stroke is drawn on its own colour"
        );
    }

    /// A path that merely crosses the band is mostly legible, and a brief bad stretch is
    /// outvoted by the median inside its own chunk rather than deciding for the stroke.
    #[test]
    fn a_path_crossing_its_own_colour_briefly_is_not_disturbed() {
        let frame = banded(Rgb::new(0x1E, 0x1E, 0x1E), DEFAULT, 40, 6);
        let across = vec![
            LogicalPoint::new(120.0, 0.0),
            LogicalPoint::new(120.0, 250.0),
        ];
        assert_eq!(
            pick(
                &frame,
                &Footprint::Path {
                    points: across,
                    width: STROKE_WIDTH
                }
            ),
            DEFAULT
        );
    }

    fn bounds_of(points: &[LogicalPoint]) -> LogicalRect {
        let (mut x0, mut y0) = (f64::MAX, f64::MAX);
        let (mut x1, mut y1) = (f64::MIN, f64::MIN);
        for p in points {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        LogicalRect::new(x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0))
    }

    // footprint geometry, which is pure and needs no screen

    #[test]
    fn an_outline_is_four_parts_and_an_area_is_one() {
        let rect = LogicalRect::new(10.0, 20.0, 100.0, 50.0);
        assert_eq!(Footprint::Area(rect).parts().len(), 1);
        assert_eq!(
            Footprint::Outline {
                rect,
                width: STROKE_WIDTH
            }
            .parts()
            .len(),
            4
        );
    }

    /// A path is cut into a fixed number of parts however many segments it has. Scoring
    /// each segment would be worst-case scoring by another name, and that statistic has
    /// no signal left in it.
    #[test]
    fn a_path_is_a_bounded_number_of_parts_however_long() {
        let zigzag: Vec<LogicalPoint> = (0..200)
            .map(|i| LogicalPoint::new(i as f64, (i % 2) as f64 * 10.0))
            .collect();
        assert_eq!(
            Footprint::Path {
                points: zigzag,
                width: STROKE_WIDTH
            }
            .parts()
            .len(),
            PATH_PARTS
        );
    }

    #[test]
    fn outline_samples_sit_on_the_border_and_not_inside_it() {
        let rect = LogicalRect::new(0.0, 0.0, 100.0, 100.0);
        let parts = Footprint::Outline { rect, width: 4.0 }.parts();

        // A four point stroke straddles its edge, so samples reach four points in.
        for part in &parts {
            for at in part {
                let near_edge = at.x <= 4.0 || at.x >= 96.0 || at.y <= 4.0 || at.y >= 96.0;
                assert!(near_edge, "{at:?} is in the middle of the region");
            }
        }
    }

    /// Spread by length, so one long segment among many short ones is still sampled.
    #[test]
    fn path_samples_follow_length_rather_than_vertices() {
        let mut points = vec![LogicalPoint::new(0.0, 0.0)];
        // Twenty tiny steps, then one very long one.
        for i in 1..=20 {
            points.push(LogicalPoint::new(i as f64 * 0.5, 0.0));
        }
        points.push(LogicalPoint::new(1000.0, 0.0));

        let parts = Footprint::Path { points, width: 1.0 }.parts();
        let all: Vec<&LogicalPoint> = parts.iter().flatten().collect();
        let beyond = all.iter().filter(|p| p.x > 10.0).count();
        assert!(
            beyond > all.len() / 2,
            "the long segment holds most of the length and should hold most of the samples"
        );
    }

    /// The weakness that bounded chunks exist to fix. A stroke running along a coloured
    /// band and then leaving it must be legible on both, not only on whichever background
    /// happens to hold more of its length.
    #[test]
    fn a_path_that_leaves_the_band_stays_visible_on_both() {
        let dark = Rgb::new(0x1E, 0x1E, 0x1E);
        let frame = banded(dark, DEFAULT, 40, 6);
        // Two thirds along the band, one third down into the dark.
        let leaving = vec![
            LogicalPoint::new(20.0, 43.0),
            LogicalPoint::new(220.0, 43.0),
            LogicalPoint::new(220.0, 140.0),
        ];

        let picked = pick(
            &frame,
            &Footprint::Path {
                points: leaving,
                width: STROKE_WIDTH,
            },
        );
        assert!(
            picked.contrast(DEFAULT) > 1.5 && picked.contrast(dark) > 1.5,
            "{picked:?} is invisible on one of the two backgrounds it crosses"
        );
    }

    #[test]
    fn a_degenerate_path_does_not_panic() {
        let same = vec![LogicalPoint::new(5.0, 5.0), LogicalPoint::new(5.0, 5.0)];
        let parts = Footprint::Path {
            points: same,
            width: STROKE_WIDTH,
        }
        .parts();
        assert!(!parts.is_empty());
    }

    #[test]
    fn a_zero_sized_outline_does_not_panic() {
        let parts = Footprint::Outline {
            rect: LogicalRect::new(5.0, 5.0, 0.0, 0.0),
            width: STROKE_WIDTH,
        }
        .parts();
        assert_eq!(parts.len(), 4);
    }

    #[test]
    fn well_formed_colours_parse_and_bad_ones_do_not() {
        assert_eq!(Rgb::parse("#FFB020"), Some(DEFAULT));
        for bad in ["FFB020", "#FFB", "#GGGGGG", "#FFB0200", "", "#"] {
            assert_eq!(Rgb::parse(bad), None, "{bad:?} should not parse");
        }
    }
}
