//! Outline fonts: TrueType and OpenType, including the handwriting fonts people buy.
//!
//! Text is shaped with the font's own rules, which for a handwriting font usually include
//! contextual alternates so that a letter looks different beside different neighbours.
//! Each glyph keeps its exact outline, and only the order its ink appears in is derived,
//! from a skeleton of the glyph. See [`super::skeleton`].
//!
//! Some fonts sold as single-line fonts for cutting machines close each stroke by tracing
//! back along it, since a font format has no open paths. Their contours enclose no area,
//! and they are drawn as strokes like a single-line font rather than filled.

use super::{GlyphInk, Hand, Letters, Placed, skeleton};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// How many straight pieces a curve in an outline is flattened into.
const CURVE_STEPS: usize = 8;

/// The area, in square ems, under which a contour is a stroke traced out and back.
const RETRACED: f64 = 0.0015;

/// A hand from a font file's bytes and the index of the face within it.
pub fn hand(name: impl Into<String>, data: Vec<u8>, index: u32) -> Result<Hand, String> {
    let font = OutlineFont::new(data, index)?;
    Ok(Hand::new(name, Box::new(font)))
}

#[derive(Debug)]
struct OutlineFont {
    data: Vec<u8>,
    index: u32,
    units: f64,
    ascent: f64,
    descent: f64,
    /// Each glyph's strokes, worked out once. A skeleton costs a fraction of a millisecond,
    /// and a mark following a scroll is laid out again every time the page moves.
    cache: Mutex<HashMap<u32, Arc<GlyphInk>>>,
}

impl OutlineFont {
    fn new(data: Vec<u8>, index: u32) -> Result<Self, String> {
        let (units, ascent, descent) = {
            let face = ttf_parser::Face::parse(&data, index).map_err(|e| e.to_string())?;
            let units = f64::from(face.units_per_em().max(1));
            (
                units,
                f64::from(face.ascender()) / units,
                -f64::from(face.descender()) / units,
            )
        };
        Ok(Self {
            data,
            index,
            units,
            ascent,
            descent,
            cache: Mutex::new(HashMap::new()),
        })
    }

    fn glyph_ink(&self, glyph: u32) -> GlyphInk {
        let Ok(face) = ttf_parser::Face::parse(&self.data, self.index) else {
            return GlyphInk::default();
        };
        let mut pen = Flattener {
            units: self.units,
            contours: Vec::new(),
        };
        let Ok(id) = u16::try_from(glyph) else {
            return GlyphInk::default();
        };
        if face
            .outline_glyph(ttf_parser::GlyphId(id), &mut pen)
            .is_none()
        {
            return GlyphInk::default();
        }
        let contours: Vec<Vec<(f64, f64)>> =
            pen.contours.into_iter().filter(|c| c.len() > 1).collect();
        if contours.is_empty() {
            return GlyphInk::default();
        }

        if contours.iter().all(|c| area(c).abs() < RETRACED) {
            let strokes = contours.iter().map(|c| out_and_back(c)).collect();
            return GlyphInk {
                strokes: super::in_writing_order(strokes, 0.06),
                ..GlyphInk::default()
            };
        }

        let (strokes, thickness) = skeleton::strokes(&contours);
        GlyphInk {
            strokes,
            contours,
            thickness,
        }
    }
}

impl Letters for OutlineFont {
    fn shape(&self, line: &str) -> Option<Vec<Placed>> {
        let face = rustybuzz::Face::from_slice(&self.data, self.index)?;
        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(line);
        buffer.guess_segment_properties();
        let shaped = rustybuzz::shape(&face, &[], buffer);
        let mut placed = Vec::with_capacity(shaped.len());
        for (info, position) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
            // Glyph zero is the font's "missing" box. A line containing one is a line this
            // font cannot write, which is a reason to typeset it rather than draw boxes.
            if info.glyph_id == 0 && !line.chars().all(char::is_whitespace) {
                return None;
            }
            placed.push(Placed {
                glyph: info.glyph_id,
                advance: f64::from(position.x_advance) / self.units,
                offset: (
                    f64::from(position.x_offset) / self.units,
                    -f64::from(position.y_offset) / self.units,
                ),
            });
        }
        Some(placed)
    }

    fn ink(&self, glyph: u32) -> Arc<GlyphInk> {
        if let Some(ink) = self.cache.lock().expect("glyph cache").get(&glyph) {
            return ink.clone();
        }
        let ink = Arc::new(self.glyph_ink(glyph));
        self.cache
            .lock()
            .expect("glyph cache")
            .insert(glyph, ink.clone());
        ink
    }

    fn extent(&self) -> (f64, f64) {
        (self.ascent, self.descent)
    }

    fn roughened(&self) -> bool {
        false
    }

    fn right_to_left(&self, line: &str) -> bool {
        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(line);
        buffer.guess_segment_properties();
        buffer.direction() == rustybuzz::Direction::RightToLeft
    }
}

/// Collects an outline as polylines in ems, y down, flattening its curves.
struct Flattener {
    units: f64,
    contours: Vec<Vec<(f64, f64)>>,
}

impl Flattener {
    fn point(&self, x: f32, y: f32) -> (f64, f64) {
        (f64::from(x) / self.units, -f64::from(y) / self.units)
    }

    fn last(&self) -> (f64, f64) {
        self.contours
            .last()
            .and_then(|c| c.last().copied())
            .unwrap_or_default()
    }

    fn push(&mut self, point: (f64, f64)) {
        if let Some(contour) = self.contours.last_mut() {
            contour.push(point);
        }
    }
}

impl ttf_parser::OutlineBuilder for Flattener {
    fn move_to(&mut self, x: f32, y: f32) {
        let point = self.point(x, y);
        self.contours.push(vec![point]);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let point = self.point(x, y);
        self.push(point);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (a, b, c) = (self.last(), self.point(x1, y1), self.point(x, y));
        for i in 1..=CURVE_STEPS {
            let t = i as f64 / CURVE_STEPS as f64;
            let u = 1.0 - t;
            self.push((
                u * u * a.0 + 2.0 * u * t * b.0 + t * t * c.0,
                u * u * a.1 + 2.0 * u * t * b.1 + t * t * c.1,
            ));
        }
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (a, b, c, d) = (
            self.last(),
            self.point(x1, y1),
            self.point(x2, y2),
            self.point(x, y),
        );
        for i in 1..=CURVE_STEPS {
            let t = i as f64 / CURVE_STEPS as f64;
            let u = 1.0 - t;
            self.push((
                u * u * u * a.0 + 3.0 * u * u * t * b.0 + 3.0 * u * t * t * c.0 + t * t * t * d.0,
                u * u * u * a.1 + 3.0 * u * u * t * b.1 + 3.0 * u * t * t * c.1 + t * t * t * d.1,
            ));
        }
    }

    fn close(&mut self) {}
}

/// The signed area of a closed polyline, in square ems.
fn area(contour: &[(f64, f64)]) -> f64 {
    let n = contour.len();
    (0..n)
        .map(|i| {
            let (a, b) = (contour[i], contour[(i + 1) % n]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum::<f64>()
        / 2.0
}

/// The outward half of a contour that runs out along a stroke and back again.
///
/// Cut at the point furthest from where it starts, which is the far end of the stroke.
fn out_and_back(contour: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let start = contour[0];
    let far = contour
        .iter()
        .enumerate()
        .max_by(|a, b| {
            let da = (a.1.0 - start.0).hypot(a.1.1 - start.1);
            let db = (b.1.0 - start.0).hypot(b.1.1 - start.1);
            da.total_cmp(&db)
        })
        .map_or(contour.len() - 1, |(i, _)| i);
    contour[..=far.max(1)].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_contour_traced_out_and_back_has_no_area_and_keeps_its_outward_half() {
        let there = [(0.0, 0.0), (0.5, 0.0), (0.5, 0.3)];
        let back: Vec<(f64, f64)> = there.iter().rev().skip(1).copied().collect();
        let contour: Vec<(f64, f64)> = there.iter().copied().chain(back).collect();
        assert!(area(&contour).abs() < RETRACED);
        assert_eq!(out_and_back(&contour), there.to_vec());
    }

    fn bundled(file: &str) -> Hand {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/fonts/scripts")
            .join(file);
        hand(file, std::fs::read(path).unwrap(), 0).unwrap()
    }

    fn layout() -> super::super::Layout {
        super::super::Layout {
            size: 32.0,
            max_width: 1000.0,
            centred: false,
        }
    }

    #[test]
    fn devanagari_is_shaped_and_written_from_an_outline_font() {
        let kalam = bundled("Kalam-Regular.ttf");
        let origin = arin_protocol::LogicalPoint::new(0.0, 0.0);
        let writing = kalam.write("सहेजें", layout(), origin, 1).unwrap();
        assert!(!writing.strokes.is_empty());
        assert_eq!(writing.outlines.len(), writing.strokes.len());
        assert!(
            writing.width > 1.0,
            "the reveal line covers the glyphs' thickness"
        );
    }

    #[test]
    fn arabic_is_written_from_the_right() {
        let ruqaa = bundled("ArefRuqaa-Regular.ttf");
        let origin = arin_protocol::LogicalPoint::new(0.0, 0.0);
        let writing = ruqaa.write("حفظ الملف", layout(), origin, 1).unwrap();
        let first = writing.strokes.first().unwrap()[0];
        let last = writing.strokes.last().unwrap()[0];
        assert!(
            first.x > last.x,
            "the pen starts at the right: {first:?} then {last:?}"
        );
    }

    #[test]
    fn a_font_without_the_script_cannot_write_it() {
        let itim = bundled("Itim-Regular.ttf");
        assert!(itim.can_write("บันทึก"));
        assert!(!itim.can_write("حفظ"));
    }

    #[test]
    fn a_square_has_its_area() {
        let square = [(0.0, 0.0), (0.5, 0.0), (0.5, 0.5), (0.0, 0.5)];
        assert!((area(&square).abs() - 0.25).abs() < 1e-9);
    }
}
