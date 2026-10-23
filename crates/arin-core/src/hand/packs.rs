//! Stroke packs: other scripts, from open data that recorded how each character is written.
//!
//! Built by `packaging/strokes/build.py` from KanjiVG, Make Me a Hanzi, AnimCJK and KiCad's
//! newstroke font, and shipped beside the binary as data files under their own licences,
//! never compiled into it. See that script for the format and the sources.

use super::{GlyphInk, Hand, Letters, Placed, in_writing_order};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

const MAGIC: &[u8; 8] = b"ARINSTRK";
const HEADER: usize = 8 + 2 + 2 + 5 * 4;
/// The stroke order is a person's and is kept as it is.
const HUMAN_ORDER: u16 = 1;

/// A hand from a pack file, named after the file.
pub fn load(path: &Path) -> Result<Hand, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(Hand::new(name, Box::new(Pack::parse(data)?)))
}

#[derive(Debug)]
struct Pack {
    data: Vec<u8>,
    index: HashMap<char, usize>,
    human_order: bool,
    ascent: f64,
    descent: f64,
    origin: (f64, f64),
    span: f64,
    cache: Mutex<HashMap<u32, Arc<GlyphInk>>>,
}

impl Pack {
    fn parse(data: Vec<u8>) -> Result<Self, String> {
        if data.len() < HEADER || &data[..8] != MAGIC {
            return Err("not a stroke pack".into());
        }
        let u16_at = |at: usize| u16::from_le_bytes([data[at], data[at + 1]]);
        let u32_at =
            |at: usize| u32::from_le_bytes(data[at..at + 4].try_into().expect("four bytes"));
        let f32_at = |at: usize| {
            f64::from(f32::from_le_bytes(
                data[at..at + 4].try_into().expect("four bytes"),
            ))
        };
        if u16_at(8) != 1 {
            return Err(format!(
                "stroke pack version {} is not one this build reads",
                u16_at(8)
            ));
        }
        let flags = u16_at(10);
        let (ascent, descent) = (f32_at(12), f32_at(16));
        let origin = (f32_at(20), f32_at(24));
        let span = f32_at(28);
        // The notice says where the strokes came from and under what licence. It is kept
        // in the file for whoever reads it there, and skipped here.
        let notice = u32_at(HEADER) as usize;
        let table = HEADER + 4 + notice + 4;
        if data.len() < table {
            return Err("stroke pack is cut short".into());
        }
        let count = u32_at(table - 4) as usize;
        let glyphs = table + count * 8;
        if data.len() < glyphs {
            return Err("stroke pack is cut short".into());
        }
        let mut index = HashMap::with_capacity(count);
        for i in 0..count {
            let at = table + i * 8;
            if let Some(c) = char::from_u32(u32_at(at)) {
                index.insert(c, glyphs + u32_at(at + 4) as usize);
            }
        }
        Ok(Self {
            data,
            index,
            human_order: flags & HUMAN_ORDER != 0,
            ascent,
            descent,
            origin,
            span,
            cache: Mutex::new(HashMap::new()),
        })
    }

    fn advance(&self, at: usize) -> f64 {
        f64::from(self.data[at]) / 255.0 * self.span
    }

    fn decode(&self, at: usize) -> Option<GlyphInk> {
        let mut cursor = at + 1;
        let strokes_count = *self.data.get(cursor)?;
        cursor += 1;
        let mut strokes = Vec::with_capacity(usize::from(strokes_count));
        for _ in 0..strokes_count {
            let points = usize::from(*self.data.get(cursor)?);
            cursor += 1;
            let bytes = self.data.get(cursor..cursor + points * 2)?;
            cursor += points * 2;
            strokes.push(
                bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|q| {
                        (
                            self.origin.0 + f64::from(q[0]) / 255.0 * self.span,
                            self.origin.1 + f64::from(q[1]) / 255.0 * self.span,
                        )
                    })
                    .collect(),
            );
        }
        let strokes = if self.human_order {
            strokes
        } else {
            in_writing_order(strokes, 0.06)
        };
        Some(GlyphInk {
            strokes,
            ..GlyphInk::default()
        })
    }
}

impl Letters for Pack {
    fn shape(&self, line: &str) -> Option<Vec<Placed>> {
        line.chars()
            .map(|c| {
                self.index.get(&c).map(|&at| Placed {
                    glyph: u32::from(c),
                    advance: self.advance(at),
                    offset: (0.0, 0.0),
                })
            })
            .collect()
    }

    fn ink(&self, glyph: u32) -> Arc<GlyphInk> {
        if let Some(ink) = self.cache.lock().expect("glyph cache").get(&glyph) {
            return ink.clone();
        }
        let ink = Arc::new(
            char::from_u32(glyph)
                .and_then(|c| self.index.get(&c))
                .and_then(|&at| self.decode(at))
                .unwrap_or_default(),
        );
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
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pack with one character, an L of two strokes, written by hand.
    fn tiny(human_order: bool) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(MAGIC);
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&u16::from(human_order).to_le_bytes());
        for value in [0.8f32, 0.2, 0.0, -1.0, 1.0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        let notice = b"made up for a test";
        data.extend_from_slice(&(notice.len() as u32).to_le_bytes());
        data.extend_from_slice(notice);
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&u32::from('L').to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        // Advance, two strokes: the foot drawn first, right to left, then the stem.
        data.extend_from_slice(&[128, 2, 2, 200, 255, 0, 255, 2, 0, 0, 0, 255]);
        data
    }

    #[test]
    fn a_pack_reads_its_characters() {
        let pack = Pack::parse(tiny(true)).unwrap();
        let shaped = pack.shape("L").unwrap();
        assert!((shaped[0].advance - 128.0 / 255.0).abs() < 1e-6);
        assert!(pack.shape("LM").is_none());
        assert_eq!(pack.ink(u32::from('L')).strokes.len(), 2);
    }

    #[test]
    fn a_persons_order_is_kept_and_a_plotters_is_tidied() {
        let kept = Pack::parse(tiny(true)).unwrap().ink(u32::from('L'));
        assert!(
            kept.strokes[0][0].0 > kept.strokes[0][1].0,
            "kept as recorded"
        );
        let tidied = Pack::parse(tiny(false)).unwrap().ink(u32::from('L'));
        let foot = tidied
            .strokes
            .iter()
            .find(|s| (s[0].1 - s[1].1).abs() < 1e-9)
            .unwrap();
        assert!(
            foot[0].0 < foot[1].0,
            "a horizontal stroke starts at its left"
        );
    }

    #[test]
    fn something_else_is_not_a_pack() {
        assert!(Pack::parse(b"not a pack at all, not even close to one".to_vec()).is_err());
    }
}
