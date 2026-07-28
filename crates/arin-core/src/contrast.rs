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
//! # Why blue is never a candidate
//!
//! Blue belongs to the orb. An annotation in the orb's own colour reads as part of the
//! orb rather than as a separate mark, and the whole visual grammar rests on those being
//! two different things.

use crate::traits::Frame;
use arin_protocol::LogicalRect;

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

/// Samples taken across the region, per axis.
///
/// A 16 by 16 grid is 256 reads, which is nothing next to the capture that produced the
/// frame, and enough to notice a band of text crossing the region.
const GRID: usize = 16;

/// The contrast a mark needs before it counts as legible.
///
/// Three to one, which is the accepted floor for a graphical object as opposed to body
/// text. Above this a mark is comfortably visible and there is nothing to fix.
const LEGIBLE: f64 = 3.0;

/// Pick the colour to draw a region in.
///
/// The question asked is "can the usual colour be seen here", not "which colour scores
/// highest". Those give different answers and the second one is wrong: on a dark editor
/// white outscores amber by a wide margin, but amber is already legible there at better
/// than eight to one, and swapping it for white would trade a mark that reads as an
/// annotation for one that reads as more interface. The palette moves when the default
/// genuinely fails, and not to chase a number.
///
/// Returns [`DEFAULT`] when the frame is empty or unreadable, since a mark in the usual
/// colour beats no mark at all.
pub fn pick(frame: &Frame, rect: LogicalRect) -> Rgb {
    let samples = sample(frame, rect);
    if samples.is_empty() {
        return DEFAULT;
    }

    // Against a typical pixel, not the average and not the worst. See the module docs.
    let typical = |candidate: &Rgb| {
        let mut scores: Vec<f64> = samples.iter().map(|s| candidate.contrast(*s)).collect();
        scores.sort_by(f64::total_cmp);
        scores[scores.len() / 2]
    };

    if typical(&DEFAULT) >= LEGIBLE {
        return DEFAULT;
    }

    PALETTE
        .iter()
        .copied()
        .max_by(|a, b| typical(a).total_cmp(&typical(b)))
        .unwrap_or(DEFAULT)
}

/// Read a grid of pixels from the region, in physical pixels.
///
/// The rect arrives in logical points and the frame is in physical pixels at whatever
/// size the capture backend produced, which is not necessarily the display's own scale:
/// a downscaled capture is both cheaper and perfectly good for averaging colour. So the
/// mapping goes through the frame's own dimensions rather than through `frame.scale`.
fn sample(frame: &Frame, rect: LogicalRect) -> Vec<Rgb> {
    let (width, height) = (frame.width as usize, frame.height as usize);
    if width == 0 || height == 0 || frame.pixels.len() < width * height * 4 {
        return Vec::new();
    }
    let [logical_width, logical_height] = frame.logical_size;
    if logical_width <= 0.0 || logical_height <= 0.0 {
        return Vec::new();
    }

    let to_x = |v: f64| ((v / logical_width) * width as f64).round();
    let to_y = |v: f64| ((v / logical_height) * height as f64).round();

    // Clamped rather than rejected: a mark half off the edge of the screen still wants a
    // colour, chosen from whichever part of it is actually on the display.
    let x0 = to_x(rect.x).clamp(0.0, width as f64 - 1.0) as usize;
    let y0 = to_y(rect.y).clamp(0.0, height as f64 - 1.0) as usize;
    let x1 = to_x(rect.x + rect.width).clamp(0.0, width as f64 - 1.0) as usize;
    let y1 = to_y(rect.y + rect.height).clamp(0.0, height as f64 - 1.0) as usize;

    // A small region on a downscaled capture can collapse to a single pixel. Sampling it
    // once is correct rather than a failure.
    let span_x = x1.saturating_sub(x0).max(1);
    let span_y = y1.saturating_sub(y0).max(1);

    let mut samples = Vec::with_capacity(GRID * GRID);
    for row in 0..GRID {
        let y = y0 + row * span_y / GRID;
        for col in 0..GRID {
            let x = x0 + col * span_x / GRID;
            let idx = (y.min(height - 1) * width + x.min(width - 1)) * 4;
            if let Some(px) = frame.pixels.get(idx..idx + 4) {
                // Packed BGRA, as the capture backends document.
                samples.push(Rgb::new(px[2], px[1], px[0]));
            }
        }
    }
    samples
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

    fn whole(frame: &Frame) -> LogicalRect {
        LogicalRect::new(0.0, 0.0, frame.logical_size[0], frame.logical_size[1])
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
        let picked = pick(&flat(Rgb::new(0x1E, 0x1E, 0x1E)), whole(&flat(DEFAULT)));
        assert_eq!(picked, DEFAULT);
    }

    #[test]
    fn amber_on_amber_moves_away() {
        // The failure a fixed colour cannot avoid: a warning banner the same colour as
        // the annotation.
        let frame = flat(DEFAULT);
        let picked = pick(&frame, whole(&frame));
        assert_ne!(picked, DEFAULT, "amber on amber must not stay amber");
        assert!(
            picked.contrast(DEFAULT) > 2.0,
            "{picked:?} is still too close to the background"
        );
    }

    #[test]
    fn a_white_page_gets_something_readable() {
        let frame = flat(Rgb::new(0xFF, 0xFF, 0xFF));
        let picked = pick(&frame, whole(&frame));
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

        let picked = pick(&frame, whole(&frame));
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

        assert_eq!(pick(&frame, whole(&frame)), DEFAULT);
    }

    #[test]
    fn a_region_outside_the_frame_falls_back() {
        let frame = flat(Rgb::new(0x1E, 0x1E, 0x1E));
        let outside = LogicalRect::new(9000.0, 9000.0, 10.0, 10.0);
        // Off the edge entirely: clamped to the nearest real pixel rather than refused.
        assert_eq!(pick(&frame, outside), DEFAULT);
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
            pick(&frame, LogicalRect::new(0.0, 0.0, 10.0, 10.0)),
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
            pick(&frame, LogicalRect::new(0.0, 0.0, 10.0, 10.0)),
            DEFAULT
        );
    }

    #[test]
    fn well_formed_colours_parse_and_bad_ones_do_not() {
        assert_eq!(Rgb::parse("#FFB020"), Some(DEFAULT));
        for bad in ["FFB020", "#FFB", "#GGGGGG", "#FFB0200", "", "#"] {
            assert_eq!(Rgb::parse(bad), None, "{bad:?} should not parse");
        }
    }
}
