//! Text written by hand, stroke by stroke.
//!
//! The default hand is EMS Tech, a single-line font: each letter is stored as the path a
//! pen travels rather than as the outline of the ink, which is what lets a letter be drawn
//! on in the order a hand would draw it. It is an OFL font derived from Architects
//! Daughter, bundled under `assets/fonts/ems-tech` with its licence beside it.
//!
//! A handwriting font the user has installed can stand in for it. Those are almost always
//! outline fonts, so their letters keep their exact shapes and only the order they appear
//! in is worked out, from a skeleton of each glyph. See [`outlines`] and [`choose`].
//!
//! The geometry lives in core for [`crate::sketch`]'s reason: three renderers would
//! otherwise hold three opinions about what handwriting looks like. Everything here is a
//! function of the text and where it goes, so a mark redrawn after a scroll is written in
//! exactly the same letters rather than re-rolling its wobble.
//!
//! Short text only. [`MAX_LETTERS`] letters fit in a line or two at [`LETTER`] apiece,
//! and anything longer stays typeset, because a paragraph written by hand is slow to
//! appear and slower to read.

pub mod choose;
mod lines;
pub mod outlines;
pub mod packs;
mod skeleton;

use crate::sketch::mix;
use arin_protocol::{LogicalPoint, LogicalRect};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

pub use choose::{Chosen, FontFile, HandChoice, How, choose};

/// How long the pen spends on each letter.
///
/// Settled by watching it at speeds from 15 to 120 ms. Faster starts to read as a wipe,
/// and slower as waiting.
pub const LETTER: Duration = Duration::from_millis(75);

/// The most letters written by hand. Longer text is typeset.
///
/// About 2.3 seconds at [`LETTER`], which covers a label or a short instruction such as
/// "Open Settings, then Privacy" and leaves sentences to the typeset box.
pub const MAX_LETTERS: usize = 30;

/// Em sizes for each place text is written, in logical points.
///
/// Larger than the typeset sizes they replace, because a single pen line carries less ink
/// than a typeface and reads smaller at the same size.
pub const CAPTION_SIZE: f64 = 24.0;
/// See [`CAPTION_SIZE`].
pub const NOTE_SIZE: f64 = 28.0;
/// See [`CAPTION_SIZE`].
pub const GUIDE_SIZE: f64 = 38.0;
/// The smallest a text box's writing shrinks to before it is typeset instead.
pub const MIN_SIZE: f64 = 16.0;

/// Space between a text box's edge and the writing inside it.
pub const BOX_PADDING: f64 = 10.0;

/// The widest a caption runs before it wraps.
///
/// A label is a few words, and one running the width of the display would cover what it
/// describes. The same width the typeset caption truncates at.
const CAPTION_WIDTH: f64 = 260.0;

/// Space between a caption and the mark it labels.
const GAP: f64 = 8.0;

/// How close a caption may sit to the edge of the display.
const MARGIN: f64 = 6.0;

/// How far apart the smoothed points of a stroke are, in logical points.
///
/// Fonts store polylines, which show their corners above about forty points. A curve
/// through them sampled this finely has none at any size handwriting is drawn at.
const SMOOTHING_STEP: f64 = 1.2;

/// A letter's roughening: how far it may turn, grow, and wander off the baseline.
const TILT: f64 = 2.5 * std::f64::consts::PI / 180.0;
const GROWTH: f64 = 0.04;
const DRIFT: f64 = 0.018;
/// How much the space after a letter may vary, as a fraction of the size.
const SPACING: f64 = 0.02;

/// Space between lines, as a multiple of the em.
const LEADING: f64 = 1.08;

/// Polylines in logical points: a glyph's strokes, or the contours of its ink.
type Polylines = Vec<Vec<LogicalPoint>>;

/// Text laid out as strokes, ready to be drawn on.
#[derive(Debug, Clone, PartialEq)]
pub struct Writing {
    /// Every stroke in the order the pen draws it, in logical points.
    ///
    /// These are what the pen follows and what paces the writing.
    pub strokes: Vec<Vec<LogicalPoint>>,
    /// The ink each stroke reveals, as closed contours, in the order of [`Self::strokes`].
    ///
    /// A renderer fills each one and reveals it along its stroke, through a line
    /// [`Self::width`] wide. For a single-line font this is the stroke's own outline,
    /// thinning where the pen runs fast. For an outline font it is the whole glyph the
    /// stroke belongs to, so a glyph appears in the order its strokes are drawn. A stroke
    /// with no contours is stroked as a line instead.
    pub outlines: Vec<Vec<Vec<LogicalPoint>>>,
    /// How wide the ink is at its widest, in logical points.
    pub width: f64,
    /// The area the ink covers.
    pub bounds: LogicalRect,
    /// How many letters were written, which is what paces the writing.
    pub letters: usize,
}

impl Writing {
    /// How long the pen takes to write this.
    pub fn duration(&self) -> Duration {
        LETTER * self.letters as u32
    }

    /// Move every stroke by a logical offset.
    pub fn translate(&mut self, dx: f64, dy: f64) {
        for point in self
            .strokes
            .iter_mut()
            .chain(self.outlines.iter_mut().flatten())
            .flatten()
        {
            point.x += dx;
            point.y += dy;
        }
        self.bounds.x += dx;
        self.bounds.y += dy;
    }

    /// Every point the pen touches, in drawing order, for sampling what is under the ink.
    pub fn ink(&self) -> Vec<LogicalPoint> {
        self.strokes.iter().flatten().copied().collect()
    }
}

/// How text is laid out before it is written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    /// The em size, in logical points.
    pub size: f64,
    /// The widest a line may run before it wraps, in logical points.
    pub max_width: f64,
    /// Whether each line is centred in the widest one, rather than starting at the left.
    pub centred: bool,
}

/// Where a font's letters come from. One for single-line fonts, one for outline fonts.
pub(crate) trait Letters: Send + Sync + std::fmt::Debug {
    /// The glyphs a line is written in, in order. `None` when any of them is missing.
    fn shape(&self, line: &str) -> Option<Vec<Placed>>;
    /// A glyph's strokes and ink, in ems with y growing downward from the baseline.
    fn ink(&self, glyph: u32) -> Arc<GlyphInk>;
    /// The em's top above the baseline, and its bottom below it, as fractions of the em.
    fn extent(&self) -> (f64, f64);
    /// Whether letters are roughened, and spaced apart beyond the font's own advances.
    ///
    /// A single-line engraving font is too regular to read as a hand without it. A
    /// handwriting font someone chose has its own character, and joins between letters
    /// that turning and spacing them would break.
    fn roughened(&self) -> bool;
    /// Whether a line is written from the right, as Arabic and Hebrew are.
    ///
    /// Glyphs are always placed left to right, and this only changes the order the pen
    /// visits them in.
    fn right_to_left(&self, _line: &str) -> bool {
        false
    }
}

/// One glyph in a shaped line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Placed {
    pub glyph: u32,
    /// How far the pen moves on after it, in ems.
    pub advance: f64,
    /// Where it sits relative to the pen, in ems, y down.
    pub offset: (f64, f64),
}

/// A glyph's strokes and the ink they reveal, in ems with y down.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct GlyphInk {
    pub strokes: Vec<Vec<(f64, f64)>>,
    /// The glyph's filled contours, or empty for a single-line font, whose strokes are the
    /// ink.
    pub contours: Vec<Vec<(f64, f64)>>,
    /// How thick the ink is at its thickest, in ems. Zero for a single-line font.
    pub thickness: f64,
}

/// A hand: something that can write text as strokes.
#[derive(Debug)]
pub struct Hand {
    name: String,
    letters: Box<dyn Letters>,
}

/// Every hand text can be written in: the one chosen, and others for other scripts.
#[derive(Debug, Clone)]
pub struct Hands {
    /// The hand the person's text is written in, and how it was chosen.
    pub chosen: Chosen,
    packs: Vec<Arc<Hand>>,
    /// Handwriting fonts for scripts no stroke pack covers, tried last.
    fonts: Vec<Arc<Hand>>,
}

impl Hands {
    /// The chosen hand alone.
    pub fn new(chosen: Chosen) -> Self {
        Self {
            chosen,
            packs: Vec::new(),
            fonts: Vec::new(),
        }
    }

    /// Add every font in a directory, for scripts no stroke pack covers.
    ///
    /// These are written as an installed font is, in their own shapes in a derived order,
    /// so the order is a heuristic. They come after the packs, whose order a person wrote.
    #[must_use]
    pub fn with_fonts(mut self, directory: &std::path::Path) -> Self {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return self;
        };
        let mut paths: Vec<_> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("ttf") || e.eq_ignore_ascii_case("otf"))
            })
            .collect();
        paths.sort();
        for path in paths {
            let loaded = choose::faces(&path)
                .into_iter()
                .next()
                .ok_or_else(|| format!("{} has no readable face", path.display()))
                .and_then(|face| {
                    let data = std::fs::read(&face.path).map_err(|e| e.to_string())?;
                    outlines::hand(face.family, data, face.index)
                });
            match loaded {
                Ok(hand) => self.fonts.push(Arc::new(hand)),
                Err(e) => tracing::warn!(error = %e, "a script font could not be read"),
            }
        }
        self
    }

    /// Add every stroke pack in a directory, for text the chosen hand cannot write.
    ///
    /// A pack that cannot be read is left out and named in the log, since the rest still
    /// write their scripts. A directory that is not there adds nothing: a build without
    /// the packs typesets those scripts, as every build did before.
    #[must_use]
    pub fn with_packs(mut self, directory: &std::path::Path) -> Self {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return self;
        };
        let mut paths: Vec<_> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "strk"))
            .collect();
        paths.sort();
        for path in paths {
            match packs::load(&path) {
                Ok(hand) => self.packs.push(Arc::new(hand)),
                Err(e) => tracing::warn!(error = %e, "a stroke pack could not be read"),
            }
        }
        self
    }

    /// The names of the packs and script fonts loaded.
    pub fn others(&self) -> Vec<&str> {
        self.packs
            .iter()
            .chain(&self.fonts)
            .map(|hand| hand.name())
            .collect()
    }

    /// The hand to write `text` in, or `None` when none can and it should be typeset.
    ///
    /// The chosen hand first. Then a pack, preferring the one a reader of the text would
    /// expect: text with kana in it is Japanese, Han characters alone are read as Chinese
    /// first, since the two share most of them and differ in a few strokes' order.
    pub fn for_text(&self, text: &str) -> Option<&Hand> {
        if self.chosen.hand.can_write(text) {
            return Some(&self.chosen.hand);
        }
        let kana = text.chars().any(|c| ('\u{3040}'..='\u{30FF}').contains(&c));
        let preference: &[&str] = if kana {
            &["japanese", "chinese-simplified", "chinese-traditional"]
        } else {
            &["chinese-simplified", "chinese-traditional", "japanese"]
        };
        let rank = |hand: &Hand| {
            preference
                .iter()
                .position(|name| *name == hand.name())
                .unwrap_or(preference.len())
        };
        let mut packs: Vec<&Arc<Hand>> = self.packs.iter().collect();
        packs.sort_by_key(|hand| rank(hand));
        packs
            .into_iter()
            .chain(&self.fonts)
            .find(|hand| hand.can_write(text))
            .map(|hand| hand.as_ref())
    }
}

/// The default hand, EMS Tech.
pub fn ems_tech() -> Arc<Hand> {
    static HAND: OnceLock<Arc<Hand>> = OnceLock::new();
    HAND.get_or_init(|| Arc::new(lines::ems_tech())).clone()
}

impl Hand {
    pub(crate) fn new(name: impl Into<String>, letters: Box<dyn Letters>) -> Self {
        Self {
            name: name.into(),
            letters,
        }
    }

    /// The font's name, for saying which hand is in use.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether this hand can write every letter of `text`.
    pub fn can_write(&self, text: &str) -> bool {
        self.letters.shape(&normalise(text)).is_some()
    }

    /// Lay `text` out with its top left corner at `origin`.
    ///
    /// `None` when the hand is missing a letter or the text is longer than is written by
    /// hand at all, either of which means it should be typeset instead. Mixing two hands
    /// within a line, or one hand and a typeface, reads as broken.
    pub fn write(
        &self,
        text: &str,
        layout: Layout,
        origin: LogicalPoint,
        seed: u64,
    ) -> Option<Writing> {
        let text = normalise(text);
        let letters = text.chars().filter(|c| !c.is_whitespace()).count();
        if letters == 0 || letters > MAX_LETTERS || !self.can_write(&text) {
            return None;
        }

        let size = layout.size;
        let (ascent, descent) = self.letters.extent();
        let rough = self.letters.roughened();
        let tracking = if rough { lines::TRACKING } else { 0.0 };
        let lines = self.wrap(&text, size, layout.max_width);
        let line_height = (ascent + descent) * size * LEADING;
        let widest = lines
            .iter()
            .map(|line| self.measure(line, size))
            .fold(0.0, f64::max);

        let mut strokes = Vec::new();
        let mut outlines = Vec::new();
        let mut thickest: f64 = 0.0;
        let mut index = 0u64;
        for (row, line) in lines.iter().enumerate() {
            let indent = if layout.centred {
                (widest - self.measure(line, size)) / 2.0
            } else {
                0.0
            };
            let baseline = origin.y + ascent * size + row as f64 * line_height;
            let mut cursor = origin.x + indent;
            let mut glyphs: Vec<(Polylines, Vec<Polylines>)> = Vec::new();
            for placed in self.letters.shape(line)? {
                index += 1;
                let roll = if rough {
                    Roll::new(mix(seed ^ index.wrapping_mul(0x9E37_79B9_7F4A_7C15)))
                } else {
                    Roll::still()
                };
                let lift = if rough {
                    (index as f64 * 0.55).sin() * size * DRIFT
                } else {
                    0.0
                };
                let ink = self.letters.ink(placed.glyph);
                let to_screen = |&(x, y): &(f64, f64)| {
                    let (x, y) = roll.apply(x + placed.offset.0, y + placed.offset.1);
                    LogicalPoint::new(cursor + x * size, baseline + y * size + lift)
                };
                let contours: Vec<Vec<LogicalPoint>> = ink
                    .contours
                    .iter()
                    .map(|contour| contour.iter().map(to_screen).collect())
                    .collect();
                thickest = thickest.max(ink.thickness * size);
                let glyph_strokes: Vec<Vec<LogicalPoint>> = ink
                    .strokes
                    .iter()
                    .map(|stroke| smooth(&stroke.iter().map(to_screen).collect::<Vec<_>>()))
                    .collect();
                let glyph_outlines = vec![contours; glyph_strokes.len()];
                glyphs.push((glyph_strokes, glyph_outlines));
                cursor += (placed.advance + tracking + roll.spacing) * size;
            }
            if self.letters.right_to_left(line) {
                glyphs.reverse();
            }
            for (glyph_strokes, glyph_outlines) in glyphs {
                strokes.extend(glyph_strokes);
                outlines.extend(glyph_outlines);
            }
        }

        // A single-line font's ink is drawn here, around each stroke. An outline font's is
        // the glyph itself, and is revealed by a line as thick as the glyph's thickest part.
        let width = if thickest > 0.0 {
            thickest
        } else {
            pen_width(size)
        };
        for (stroke, outline) in strokes.iter().zip(outlines.iter_mut()) {
            if outline.is_empty() {
                outline.push(lines::outline(stroke, width));
            }
        }
        let ink: Vec<LogicalPoint> = strokes
            .iter()
            .flatten()
            .chain(outlines.iter().flatten().flatten())
            .copied()
            .collect();
        if ink.is_empty() {
            return None;
        }
        let bounds = LogicalRect::containing(&ink);
        Some(Writing {
            strokes,
            outlines,
            width,
            bounds,
            letters,
        })
    }

    /// How wide a line of text runs at a size, before any roughening.
    fn measure(&self, line: &str, size: f64) -> f64 {
        let tracking = if self.letters.roughened() {
            lines::TRACKING
        } else {
            0.0
        };
        self.letters
            .shape(line)
            .map(|glyphs| glyphs.iter().map(|g| g.advance + tracking).sum::<f64>())
            .unwrap_or(0.0)
            * size
    }

    /// Break text into lines no wider than `max_width`, at spaces.
    ///
    /// A single word wider than the line is left whole on a line of its own. Breaking it
    /// mid word would be a choice no person writing it would make.
    fn wrap(&self, text: &str, size: f64, max_width: f64) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let mut current = String::new();
        for word in text.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_owned()
            } else {
                format!("{current} {word}")
            };
            if !current.is_empty() && self.measure(&candidate, size) > max_width {
                lines.push(std::mem::replace(&mut current, word.to_owned()));
            } else {
                current = candidate;
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
        lines
    }

    /// How tall `lines` lines of text run at a size.
    pub fn height(&self, lines: usize, size: f64) -> f64 {
        let (ascent, descent) = self.letters.extent();
        let em = ascent + descent;
        em * size + em * size * LEADING * lines.saturating_sub(1) as f64
    }

    /// Lay text out to fit a box, shrinking from `layout.size` until it does.
    ///
    /// `None` when even the smallest size overflows, which a caller takes as a reason to
    /// typeset instead.
    pub fn write_within(
        &self,
        text: &str,
        layout: Layout,
        rect: LogicalRect,
        min_size: f64,
        seed: u64,
    ) -> Option<Writing> {
        let text = normalise(text);
        let mut size = layout.size;
        loop {
            let lines = self.wrap(&text, size, rect.width).len();
            let fits = self.height(lines, size) <= rect.height;
            if fits || size <= min_size {
                if !fits {
                    return None;
                }
                let mut writing = self.write(
                    &text,
                    Layout {
                        size,
                        max_width: rect.width,
                        ..layout
                    },
                    LogicalPoint::new(rect.x, rect.y),
                    seed,
                )?;
                if layout.centred {
                    let slack = rect.width - writing.bounds.width;
                    writing.translate(slack / 2.0 - (writing.bounds.x - rect.x), 0.0);
                }
                return Some(writing);
            }
            size = (size - 2.0).max(min_size);
        }
    }
}

/// A caption written beside a point, preferring its right.
///
/// Clear of the orb, centred on it vertically, flipped to the left when the right has no
/// room, and kept on the display. The same rules the typeset caption follows, in the
/// protocol's coordinates.
pub fn beside(hand: &Hand, text: &str, at: LogicalPoint, display: [f64; 2]) -> Option<Writing> {
    let clearance = crate::policy::ORB_DIAMETER / 2.0 + GAP;
    caption(hand, text, at, display, |size| {
        let right = at.x + clearance;
        let x = if right + size.0 + MARGIN > display[0] {
            at.x - clearance - size.0
        } else {
            right
        };
        (x, at.y - size.1 / 2.0)
    })
}

/// A caption written against a region, preferring above it and left aligned with it.
pub fn against(hand: &Hand, text: &str, rect: LogicalRect, display: [f64; 2]) -> Option<Writing> {
    caption(
        hand,
        text,
        LogicalPoint::new(rect.x, rect.y),
        display,
        |size| {
            let above = rect.y - GAP - size.1;
            let y = if above < MARGIN {
                rect.y + rect.height + GAP
            } else {
                above
            };
            (rect.x, y)
        },
    )
}

/// Write a caption, then move it to where `place` says its top left corner goes.
fn caption(
    hand: &Hand,
    text: &str,
    anchor: LogicalPoint,
    display: [f64; 2],
    place: impl Fn((f64, f64)) -> (f64, f64),
) -> Option<Writing> {
    let layout = Layout {
        size: CAPTION_SIZE,
        max_width: CAPTION_WIDTH,
        centred: false,
    };
    let mut writing = hand.write(
        text,
        layout,
        LogicalPoint::new(0.0, 0.0),
        seed(text, anchor),
    )?;
    let size = (writing.bounds.width, writing.bounds.height);
    let (x, y) = place(size);
    let x = clamp(x, MARGIN, display[0] - size.0 - MARGIN);
    let y = clamp(y, MARGIN, display[1] - size.1 - MARGIN);
    writing.translate(x - writing.bounds.x, y - writing.bounds.y);
    Some(writing)
}

/// Clamp, tolerating a range that has collapsed, which a caption wider than its display
/// leaves. Pinned to the near edge so the text stays visible from one side.
fn clamp(value: f64, min: f64, max: f64) -> f64 {
    if max < min {
        min
    } else {
        value.clamp(min, max)
    }
}

/// A seed for writing `text` at a place, so the same request always writes the same way.
pub fn seed(text: &str, at: LogicalPoint) -> u64 {
    let mut hash = 0xA076_1D64_78BD_642F_u64;
    for byte in text.bytes() {
        hash = mix(hash ^ u64::from(byte));
    }
    mix(hash ^ (at.x.round() as i64 as u64) ^ ((at.y.round() as i64 as u64) << 32))
}

/// How wide the pen is for text at an em size.
fn pen_width(size: f64) -> f64 {
    (1.2 + size * 0.045).clamp(1.8, 3.4)
}

/// Spell text the way a hand can write it.
///
/// Typographic quotes and the ellipsis are the common characters the EMS fonts lack, and
/// each has a plain spelling a person writing by hand would use anyway.
fn normalise(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{2032}' => out.push('\''),
            '\u{2026}' => out.push_str("..."),
            '\u{00A0}' | '\u{2009}' | '\u{202F}' => out.push(' '),
            '\u{2010}' | '\u{2011}' => out.push('-'),
            c if c.is_whitespace() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// One letter's roughening, drawn from its own seed.
struct Roll {
    sin: f64,
    cos: f64,
    growth: f64,
    spacing: f64,
}

impl Roll {
    fn new(seed: u64) -> Self {
        let unit = |index: u64| {
            let bits = mix(seed ^ index.wrapping_mul(0x517C_C1B7_2722_0A95));
            (bits >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        };
        let angle = unit(1) * TILT;
        Self {
            sin: angle.sin(),
            cos: angle.cos(),
            growth: 1.0 + unit(2) * GROWTH,
            spacing: unit(3) * SPACING,
        }
    }

    /// No roughening at all.
    fn still() -> Self {
        Self {
            sin: 0.0,
            cos: 1.0,
            growth: 1.0,
            spacing: 0.0,
        }
    }

    /// Turn and scale a point of the letter about its own baseline origin.
    fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            (x * self.cos - y * self.sin) * self.growth,
            (x * self.sin + y * self.cos) * self.growth,
        )
    }
}

/// Put a glyph's strokes in the order and direction a hand draws them.
///
/// Fonts are drawn by whatever made them, which is rarely a hand: a plotter's idea of
/// efficiency for a single-line font, or a skeleton's arbitrary walk for an outline one.
/// Two habits matter on screen: a dot goes on after the stem it belongs to, not before,
/// and a stroke starts at its top or its left.
pub(crate) fn in_writing_order(strokes: Vec<Vec<(f64, f64)>>, dot: f64) -> Vec<Vec<(f64, f64)>> {
    let (dots, mut rest): (Vec<_>, Vec<_>) = strokes.into_iter().partition(|s| extent(s) < dot);
    for stroke in &mut rest {
        let (Some(&first), Some(&last)) = (stroke.first(), stroke.last()) else {
            continue;
        };
        let (dx, dy) = (last.0 - first.0, last.1 - first.1);
        // A stroke that comes back to where it began is a bowl or a loop, and has no
        // wrong end to start from.
        if dx.hypot(dy) < extent(stroke) * 0.25 {
            continue;
        }
        let backwards = if dy.abs() > dx.abs() {
            dy < 0.0
        } else {
            dx < 0.0
        };
        if backwards {
            stroke.reverse();
        }
    }
    rest.extend(dots);
    rest
}

/// The diagonal of a stroke's bounding box, in em units.
pub(crate) fn extent(stroke: &[(f64, f64)]) -> f64 {
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &(x, y) in stroke {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    (max_x - min_x).hypot(max_y - min_y)
}

/// A Catmull-Rom curve through a polyline, sampled every [`SMOOTHING_STEP`] points.
fn smooth(points: &[LogicalPoint]) -> Vec<LogicalPoint> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let at = |i: isize| points[i.clamp(0, points.len() as isize - 1) as usize];
    let mut out = Vec::new();
    for i in 0..points.len() as isize - 1 {
        let (p0, p1, p2, p3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
        let span = ((p2.x - p1.x).powi(2) + (p2.y - p1.y).powi(2)).sqrt();
        let steps = ((span / SMOOTHING_STEP).ceil() as usize).max(1);
        for k in 0..steps {
            let t = k as f64 / steps as f64;
            let (t2, t3) = (t * t, t * t * t);
            let blend = |a: f64, b: f64, c: f64, d: f64| {
                0.5 * (2.0 * b
                    + (-a + c) * t
                    + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2
                    + (-a + 3.0 * b - 3.0 * c + d) * t3)
            };
            out.push(LogicalPoint::new(
                blend(p0.x, p1.x, p2.x, p3.x),
                blend(p0.y, p1.y, p2.y, p3.y),
            ));
        }
    }
    out.push(*points.last().expect("checked above"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(size: f64) -> Layout {
        Layout {
            size,
            max_width: 1000.0,
            centred: false,
        }
    }

    #[test]
    fn the_bundled_hand_has_the_printable_ascii_set() {
        let hand = ems_tech();
        for c in ' '..='~' {
            assert!(hand.can_write(&c.to_string()), "missing {c:?}");
        }
    }

    #[test]
    fn the_same_text_in_the_same_place_is_written_the_same_way() {
        let hand = ems_tech();
        let at = LogicalPoint::new(100.0, 200.0);
        let a = hand.write(
            "Click Save first",
            layout(28.0),
            at,
            seed("Click Save first", at),
        );
        let b = hand.write(
            "Click Save first",
            layout(28.0),
            at,
            seed("Click Save first", at),
        );
        assert_eq!(a, b);
    }

    #[test]
    fn a_different_place_rolls_a_different_hand() {
        let a = LogicalPoint::new(100.0, 200.0);
        let b = LogicalPoint::new(400.0, 200.0);
        assert_ne!(seed("Save", a), seed("Save", b));
    }

    #[test]
    fn text_is_written_inside_its_line() {
        let hand = ems_tech();
        let at = LogicalPoint::new(10.0, 10.0);
        let writing = hand.write("Hello", layout(30.0), at, 7).unwrap();
        assert!(writing.bounds.y >= at.y - 4.0, "{:?}", writing.bounds);
        assert!(writing.bounds.y + writing.bounds.height <= at.y + 30.0 * 1.1);
        assert_eq!(writing.letters, 5);
    }

    #[test]
    fn long_text_is_left_to_be_typeset() {
        let hand = ems_tech();
        let long = "a".repeat(MAX_LETTERS + 1);
        assert!(
            hand.write(&long, layout(20.0), LogicalPoint::new(0.0, 0.0), 1)
                .is_none()
        );
        let fits = "a".repeat(MAX_LETTERS);
        assert!(
            hand.write(&fits, layout(20.0), LogicalPoint::new(0.0, 0.0), 1)
                .is_some()
        );
    }

    #[test]
    fn a_letter_the_hand_lacks_leaves_the_text_to_be_typeset() {
        let hand = ems_tech();
        assert!(
            hand.write("保存", layout(20.0), LogicalPoint::new(0.0, 0.0), 1)
                .is_none()
        );
        assert!(
            hand.write("Save ✓", layout(20.0), LogicalPoint::new(0.0, 0.0), 1)
                .is_none()
        );
    }

    #[test]
    fn typographic_quotes_and_ellipses_are_written_plainly() {
        let hand = ems_tech();
        assert!(hand.can_write("it\u{2019}s here\u{2026}"));
    }

    #[test]
    fn words_wrap_to_the_width() {
        let hand = ems_tech();
        let narrow = Layout {
            size: 20.0,
            max_width: 90.0,
            centred: false,
        };
        let one = hand
            .write(
                "Open Settings",
                layout(20.0),
                LogicalPoint::new(0.0, 0.0),
                3,
            )
            .unwrap();
        let two = hand
            .write("Open Settings", narrow, LogicalPoint::new(0.0, 0.0), 3)
            .unwrap();
        assert!(two.bounds.height > one.bounds.height * 1.5);
        assert!(two.bounds.width < one.bounds.width);
    }

    #[test]
    fn fitting_a_box_shrinks_the_text_until_it_fits() {
        let hand = ems_tech();
        let rect = LogicalRect::new(0.0, 0.0, 200.0, 40.0);
        let fitted = hand
            .write_within("Open Settings, then Privacy", layout(32.0), rect, 14.0, 9)
            .unwrap();
        assert!(fitted.bounds.height <= rect.height + fitted.width);
        let tiny = LogicalRect::new(0.0, 0.0, 40.0, 10.0);
        assert!(
            hand.write_within("Open Settings", layout(32.0), tiny, 14.0, 9)
                .is_none()
        );
    }

    #[test]
    fn every_stroke_has_ink_around_it() {
        let hand = ems_tech();
        let writing = hand
            .write("Save", layout(30.0), LogicalPoint::new(0.0, 0.0), 1)
            .unwrap();
        assert_eq!(writing.outlines.len(), writing.strokes.len());
        for (stroke, outline) in writing.strokes.iter().zip(&writing.outlines) {
            let contour = &outline[0];
            assert!(contour.len() > 3);
            let ink = LogicalRect::containing(stroke);
            let around = LogicalRect::containing(contour);
            assert!(around.x <= ink.x + 0.01 && around.y <= ink.y + 0.01);
        }
    }

    #[test]
    fn a_caption_beside_a_point_clears_the_orb_and_flips_at_the_edge() {
        let hand = ems_tech();
        let display = [1000.0, 800.0];
        let left = beside(&hand, "Save", LogicalPoint::new(100.0, 400.0), display).unwrap();
        assert!(left.bounds.x >= 100.0 + crate::policy::ORB_DIAMETER / 2.0);
        let edge = beside(&hand, "Save", LogicalPoint::new(980.0, 400.0), display).unwrap();
        assert!(edge.bounds.x + edge.bounds.width <= 980.0 - crate::policy::ORB_DIAMETER / 2.0);
    }

    #[test]
    fn a_caption_against_a_region_sits_above_it_unless_there_is_no_room() {
        let hand = ems_tech();
        let display = [1000.0, 800.0];
        let rect = LogicalRect::new(200.0, 300.0, 120.0, 40.0);
        let above = against(&hand, "here", rect, display).unwrap();
        assert!(above.bounds.y + above.bounds.height <= rect.y);
        let top = LogicalRect::new(200.0, 4.0, 120.0, 40.0);
        let below = against(&hand, "here", top, display).unwrap();
        assert!(below.bounds.y >= top.y + top.height);
    }

    #[test]
    fn the_pen_takes_a_letter_time_per_letter() {
        let hand = ems_tech();
        let writing = hand
            .write("Save it", layout(20.0), LogicalPoint::new(0.0, 0.0), 1)
            .unwrap();
        assert_eq!(writing.duration(), LETTER * 6);
    }
}
