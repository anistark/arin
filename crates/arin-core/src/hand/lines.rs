//! Single-line fonts: each glyph is the path a pen travels.
//!
//! Read from SVG 1.1 fonts of moves and lines, the format the EMS and Hershey single-line
//! fonts are distributed in. Curves are not read, because no font in that family uses them.

use super::{GlyphInk, Hand, Letters, Placed};
use arin_protocol::LogicalPoint;
use std::collections::HashMap;
use std::sync::Arc;

/// The source of the default hand.
const EMS_TECH: &str = include_str!("../../../../assets/fonts/ems-tech/EMSTech.svg");

/// Space added after every letter, as a fraction of the size. The font is set tight for
/// engraving, and on a screen its letters run into each other.
pub(super) const TRACKING: f64 = 0.03;

/// A stroke smaller than this, as a fraction of the em, is a dot: the one on an i or a j,
/// or a full stop. Dots are drawn last, after the stems they sit on.
const DOT: f64 = 0.06;

/// A hand from an SVG single-line font, such as the EMS and Hershey fonts.
pub(super) fn from_svg(name: &str, source: &str) -> Result<Hand, String> {
    Ok(Hand::new(name, Box::new(LineFont::from_svg(source)?)))
}

/// The default hand.
pub(super) fn ems_tech() -> Hand {
    Hand::new(
        "EMS Tech",
        Box::new(LineFont::from_svg(EMS_TECH).expect("the bundled font parses")),
    )
}

#[derive(Debug)]
struct LineFont {
    glyphs: HashMap<char, (f64, Arc<GlyphInk>)>,
    ascent: f64,
    descent: f64,
}

impl Letters for LineFont {
    fn shape(&self, line: &str) -> Option<Vec<Placed>> {
        line.chars()
            .map(|c| {
                self.glyphs.get(&c).map(|(advance, _)| Placed {
                    glyph: u32::from(c),
                    advance: *advance,
                    offset: (0.0, 0.0),
                })
            })
            .collect()
    }

    fn ink(&self, glyph: u32) -> Arc<GlyphInk> {
        char::from_u32(glyph)
            .and_then(|c| self.glyphs.get(&c))
            .map(|(_, ink)| ink.clone())
            .unwrap_or_default()
    }

    fn extent(&self) -> (f64, f64) {
        (self.ascent, self.descent)
    }

    fn roughened(&self) -> bool {
        true
    }
}

impl LineFont {
    fn from_svg(source: &str) -> Result<Self, String> {
        // The EMS and Hershey files open with the SVG 1.1 doctype, which declares nothing
        // the glyphs depend on.
        let options = roxmltree::ParsingOptions {
            allow_dtd: true,
            ..roxmltree::ParsingOptions::default()
        };
        let document =
            roxmltree::Document::parse_with_options(source, options).map_err(|e| e.to_string())?;
        let font = document
            .descendants()
            .find(|n| n.has_tag_name("font"))
            .ok_or("no <font> element")?;
        let default_advance: f64 = font
            .attribute("horiz-adv-x")
            .and_then(|a| a.parse().ok())
            .unwrap_or(500.0);
        let face = font
            .children()
            .find(|n| n.has_tag_name("font-face"))
            .ok_or("no <font-face> element")?;
        let number = |name: &str, fallback: f64| {
            face.attribute(name)
                .and_then(|a| a.parse::<f64>().ok())
                .unwrap_or(fallback)
        };
        let units = number("units-per-em", 1000.0);
        let ascent = number("ascent", 800.0) / units;
        let descent = -number("descent", -200.0) / units;

        let mut glyphs = HashMap::new();
        for glyph in font.children().filter(|n| n.has_tag_name("glyph")) {
            let mut chars = glyph.attribute("unicode").unwrap_or_default().chars();
            let (Some(character), None) = (chars.next(), chars.next()) else {
                continue;
            };
            let advance = glyph
                .attribute("horiz-adv-x")
                .and_then(|a| a.parse().ok())
                .unwrap_or(default_advance)
                / units;
            let strokes = glyph
                .attribute("d")
                .map(|d| polylines(d, units))
                .unwrap_or_default();
            let ink = GlyphInk {
                strokes: super::in_writing_order(strokes, DOT),
                ..GlyphInk::default()
            };
            glyphs.insert(character, (advance, Arc::new(ink)));
        }
        if glyphs.is_empty() {
            return Err("the font has no glyphs".into());
        }
        Ok(Self {
            glyphs,
            ascent,
            descent,
        })
    }
}

/// Read a path of moves and lines into polylines, in em units with y growing downward.
fn polylines(d: &str, units: f64) -> Vec<Vec<(f64, f64)>> {
    let mut strokes: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut numbers: Vec<f64> = Vec::new();
    let mut command = 'M';
    let flush = |command: char, numbers: &mut Vec<f64>, strokes: &mut Vec<Vec<(f64, f64)>>| {
        for (i, pair) in numbers.as_chunks::<2>().0.iter().enumerate() {
            let point = (pair[0] / units, -pair[1] / units);
            if command == 'M' && i == 0 || strokes.is_empty() {
                strokes.push(vec![point]);
            } else if let Some(stroke) = strokes.last_mut() {
                stroke.push(point);
            }
        }
        numbers.clear();
    };
    for token in d.split(|c: char| c.is_whitespace() || c == ',') {
        if token.is_empty() {
            continue;
        }
        let first = token.chars().next().unwrap_or(' ');
        if first.is_ascii_alphabetic() {
            flush(command, &mut numbers, &mut strokes);
            command = first.to_ascii_uppercase();
            if let Ok(value) = token[1..].parse() {
                numbers.push(value);
            }
        } else if let Ok(value) = token.parse() {
            numbers.push(value);
        }
    }
    flush(command, &mut numbers, &mut strokes);
    strokes.retain(|s| !s.is_empty());
    strokes
}

/// The ink of one pen stroke, as a closed outline whose width follows the pen's speed.
///
/// The pen is fastest in the middle of a stroke and slowest where it touches down and
/// lifts, which is the timeline [`crate::reveal`] plays it on. Pressure here is the inverse
/// of that speed, so the line is fullest at its ends and thinnest where it ran.
pub(super) fn outline(stroke: &[LogicalPoint], width: f64) -> Vec<LogicalPoint> {
    let total = crate::reveal::length(stroke).max(f64::EPSILON);
    let mut run = 0.0;
    let mut previous = stroke.first().copied();
    let points: Vec<freedraw::InputPoint> = stroke
        .iter()
        .map(|&point| {
            if let Some(last) = previous {
                run += ((point.x - last.x).powi(2) + (point.y - last.y).powi(2)).sqrt();
            }
            previous = Some(point);
            let along = (run / total).clamp(0.0, 1.0);
            let speed = 2.0 * (along * (1.0 - along)).sqrt();
            freedraw::InputPoint::Array([point.x, point.y], Some(1.0 - speed))
        })
        .collect();
    // perfect-freehand's radius runs from `size * (1 - thinning) / 2` at no pressure to
    // `size * (1 + thinning) / 2` at full. These put the line between about 0.6 and 1.15
    // of the pen's width, which reads as one pen rather than a brush.
    let options = freedraw::StrokeOptions {
        size: Some(width * 0.875),
        thinning: Some(0.31),
        smoothing: Some(0.5),
        streamline: Some(0.15),
        simulate_pressure: Some(false),
        last: Some(true),
        ..freedraw::StrokeOptions::default()
    };
    freedraw::get_stroke(&points, &options)
        .into_iter()
        .map(|[x, y]| LogicalPoint::new(x, y))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> LineFont {
        LineFont::from_svg(EMS_TECH).unwrap()
    }

    #[test]
    fn a_dot_is_drawn_after_its_stem() {
        let font = font();
        let i = font.ink(u32::from('i'));
        let last = i.strokes.last().unwrap();
        assert!(
            super::super::extent(last) < DOT,
            "the dot should come last: {:?}",
            i.strokes
        );
    }

    #[test]
    fn a_stem_starts_at_its_top() {
        let font = font();
        for c in ['l', 'i', 'I'] {
            let ink = font.ink(u32::from(c));
            let stem = ink
                .strokes
                .iter()
                .find(|s| super::super::extent(s) > DOT)
                .unwrap();
            let (first, last) = (stem.first().unwrap(), stem.last().unwrap());
            assert!(first.1 < last.1, "{c} starts below where it ends");
        }
    }
}
