//! Which hand to write in.
//!
//! EMS Tech unless the person has a handwriting font of their own. Named explicitly, that
//! font wins. Otherwise, if exactly one family the person installed is classed as
//! handwriting, it is used without being asked, because that is almost certainly the one
//! they bought to see their screen written in. Two or more is a choice Arin cannot make
//! for them, and none is the common case.
//!
//! Finding the files is the platform's job, since only it knows where fonts live and which
//! of them came with the system. Deciding is here, so every platform decides the same way.

use super::{Hand, ems_tech, outlines};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What the person asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum HandChoice {
    /// Use the one handwriting family they installed, if there is exactly one.
    #[default]
    Detect,
    /// A family name, or a path to a font file. `EMS Tech` names the default.
    Named(String),
}

/// How the hand in use was chosen, for saying so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum How {
    /// The person named it.
    Named,
    /// The only handwriting family they installed.
    Detected,
    /// The default, because they have no handwriting font installed, or more than one.
    Default {
        /// How many handwriting families were found.
        found: usize,
    },
    /// The default, because the font they named could not be found or read.
    NotFound {
        /// What they named, and why it could not be used.
        reason: String,
    },
}

/// A font file the platform found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontFile {
    /// Where it is.
    pub path: PathBuf,
    /// Whether it came with the operating system rather than from the person.
    ///
    /// Those can be named, but are never picked on their own. macOS ships a dozen script
    /// and handwriting faces, and counting them would mean "exactly one" was never true.
    pub from_system: bool,
}

/// A hand, and how it came to be the one in use.
#[derive(Debug, Clone)]
pub struct Chosen {
    /// The hand.
    pub hand: Arc<Hand>,
    /// Why this one.
    pub how: How,
}

impl Chosen {
    /// One line for a log or a menu: which hand, and why.
    pub fn describe(&self) -> String {
        let name = self.hand.name();
        match &self.how {
            How::Named => format!("{name}, as named"),
            How::Detected => format!("{name}, the handwriting font you installed"),
            How::Default { found: 0 } => format!("{name}, the default"),
            How::Default { found } => {
                format!(
                    "{name}, the default, since {found} handwriting fonts are installed. Name one with --hand"
                )
            }
            How::NotFound { reason } => format!("{name}, the default, since {reason}"),
        }
    }
}

/// One face in a font file.
#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    /// The file it is in.
    pub path: PathBuf,
    /// Its index within the file, for a collection.
    pub index: u32,
    /// The family it belongs to.
    pub family: String,
    /// Whether the font says it is handwriting.
    pub handwriting: bool,
    /// Whether it is an italic, which a regular face is preferred over.
    pub italic: bool,
    /// Its weight, where 400 is regular.
    pub weight: u16,
}

/// Pick a hand from what the person asked for and the fonts available.
pub fn choose(choice: &HandChoice, fonts: &[FontFile]) -> Chosen {
    match choice {
        HandChoice::Named(name) if name.eq_ignore_ascii_case(ems_tech().name()) => Chosen {
            hand: ems_tech(),
            how: How::Named,
        },
        HandChoice::Named(name) => {
            let path = Path::new(name);
            let faces: Vec<Face> = if path.is_file() {
                faces(path)
            } else {
                fonts
                    .iter()
                    .flat_map(|font| faces(&font.path))
                    .filter(|f| f.family.eq_ignore_ascii_case(name))
                    .collect()
            };
            match best(&faces).map(load) {
                Some(Ok(hand)) => Chosen {
                    hand: Arc::new(hand),
                    how: How::Named,
                },
                Some(Err(e)) => fallback(format!("{name} could not be read: {e}")),
                None => fallback(format!("no installed font is called {name}")),
            }
        }
        HandChoice::Detect => {
            let mut families: BTreeMap<String, Vec<Face>> = BTreeMap::new();
            let installed = fonts.iter().filter(|font| !font.from_system);
            for face in installed.flat_map(|font| faces(&font.path)) {
                if face.handwriting {
                    families.entry(face.family.clone()).or_default().push(face);
                }
            }
            if families.len() != 1 {
                return Chosen {
                    hand: ems_tech(),
                    how: How::Default {
                        found: families.len(),
                    },
                };
            }
            let faces = families.into_values().next().unwrap_or_default();
            match best(&faces).map(load) {
                Some(Ok(hand)) => Chosen {
                    hand: Arc::new(hand),
                    how: How::Detected,
                },
                Some(Err(e)) => {
                    fallback(format!("the handwriting font found could not be read: {e}"))
                }
                None => Chosen {
                    hand: ems_tech(),
                    how: How::Default { found: 0 },
                },
            }
        }
    }
}

fn fallback(reason: String) -> Chosen {
    Chosen {
        hand: ems_tech(),
        how: How::NotFound { reason },
    }
}

/// The face a family is written in: upright, and nearest a regular weight.
fn best(faces: &[Face]) -> Option<&Face> {
    faces
        .iter()
        .min_by_key(|f| (f.italic, (i32::from(f.weight) - 400).unsigned_abs()))
}

fn load(face: &Face) -> Result<Hand, String> {
    if face
        .path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
    {
        let source = std::fs::read_to_string(&face.path).map_err(|e| e.to_string())?;
        return super::lines::from_svg(&face.family, &source);
    }
    let data = std::fs::read(&face.path).map_err(|e| e.to_string())?;
    outlines::hand(face.family.clone(), data, face.index)
}

/// Every face in a font file, with what it says about itself.
///
/// An unreadable file has none, rather than failing the choice: one broken font in a
/// folder of them is not a reason to give up on the rest.
pub fn faces(path: &Path) -> Vec<Face> {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
    {
        let family = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        return vec![Face {
            path: path.to_owned(),
            index: 0,
            family,
            handwriting: true,
            italic: false,
            weight: 400,
        }];
    }
    let Ok(data) = std::fs::read(path) else {
        return Vec::new();
    };
    let count = ttf_parser::fonts_in_collection(&data).unwrap_or(1);
    (0..count)
        .filter_map(|index| {
            let face = ttf_parser::Face::parse(&data, index).ok()?;
            Some(Face {
                path: path.to_owned(),
                index,
                family: family(&face)?,
                handwriting: face
                    .raw_face()
                    .table(ttf_parser::Tag::from_bytes(b"OS/2"))
                    .is_some_and(is_handwriting),
                italic: face.is_italic(),
                weight: face.weight().to_number(),
            })
        })
        .collect()
}

/// A face's family name: the typographic one where it has one, else the legacy one.
///
/// Read from Unicode records first, then from Mac Roman ones, which older fonts, several
/// of macOS's own among them, carry instead.
fn family(face: &ttf_parser::Face) -> Option<String> {
    let named = |id: u16| {
        let records = face.names().into_iter().filter(|n| n.name_id == id);
        let unicode = records
            .clone()
            .filter(|n| n.is_unicode())
            .find_map(|n| n.to_string());
        unicode.or_else(|| {
            records
                .filter(|n| {
                    n.platform_id == ttf_parser::PlatformId::Macintosh && n.encoding_id == 0
                })
                .map(|n| mac_roman(n.name))
                .next()
        })
    };
    named(ttf_parser::name_id::TYPOGRAPHIC_FAMILY).or_else(|| named(ttf_parser::name_id::FAMILY))
}

/// Decode Mac OS Roman, the encoding of a font's Macintosh name records.
fn mac_roman(bytes: &[u8]) -> String {
    const HIGH: &str = "ÄÅÇÉÑÖÜáàâäãåçéèêëíìîïñóòôöõúùûü†°¢£§•¶ß®©™´¨≠ÆØ∞±≤≥¥µ∂∑∏π∫ªºΩæø\
                        ¿¡¬√ƒ≈∆«»…\u{A0}ÀÃÕŒœ–—“”‘’÷◊ÿŸ⁄€‹›ﬁﬂ‡·‚„‰ÂÊÁËÈÍÎÏÌÓÔ\u{F8FF}ÒÚÛÙıˆ˜¯˘˙˚¸˝˛ˇ";
    let high: Vec<char> = HIGH.chars().collect();
    bytes
        .iter()
        .map(|&b| {
            if b < 0x80 {
                char::from(b)
            } else {
                high.get(usize::from(b - 0x80)).copied().unwrap_or('?')
            }
        })
        .collect()
}

/// Whether a font's OS/2 table classes it as handwriting.
///
/// Two fields can say so. PANOSE's family kind "Latin Hand Written" says exactly that. The
/// IBM family class "Scripts" says script of any kind, which takes in formal calligraphy
/// too, so it counts only where PANOSE does not say the font is something else. Many fonts
/// leave both blank and are missed, which is what naming a font is for.
pub fn is_handwriting(os2: &[u8]) -> bool {
    const FAMILY_CLASS: usize = 30;
    const PANOSE: usize = 32;
    const LATIN_HAND_WRITTEN: u8 = 3;
    const SCRIPTS: u8 = 10;
    let (Some(&class), Some(&panose)) = (os2.get(FAMILY_CLASS), os2.get(PANOSE)) else {
        return false;
    };
    panose == LATIN_HAND_WRITTEN || class == SCRIPTS && matches!(panose, 0 | 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os2(class: u8, panose: u8) -> Vec<u8> {
        let mut table = vec![0u8; 96];
        table[30] = class;
        table[32] = panose;
        table
    }

    #[test]
    fn panose_hand_written_is_handwriting() {
        assert!(is_handwriting(&os2(0, 3)));
    }

    #[test]
    fn a_script_class_counts_unless_panose_says_otherwise() {
        assert!(is_handwriting(&os2(10, 0)));
        assert!(
            !is_handwriting(&os2(10, 2)),
            "a text face classed as script by mistake"
        );
        assert!(!is_handwriting(&os2(8, 2)));
    }

    #[test]
    fn a_short_table_is_not_handwriting() {
        assert!(!is_handwriting(&[0u8; 20]));
    }

    #[test]
    fn with_nothing_installed_the_default_is_used() {
        let chosen = choose(&HandChoice::Detect, &[]);
        assert_eq!(chosen.hand.name(), "EMS Tech");
        assert_eq!(chosen.how, How::Default { found: 0 });
    }

    #[test]
    fn naming_the_default_is_a_choice_too() {
        let chosen = choose(&HandChoice::Named("ems tech".into()), &[]);
        assert_eq!(chosen.how, How::Named);
    }

    #[test]
    fn a_name_nothing_answers_to_falls_back_and_says_why() {
        let chosen = choose(&HandChoice::Named("Nonesuch Hand".into()), &[]);
        assert_eq!(chosen.hand.name(), "EMS Tech");
        assert!(matches!(chosen.how, How::NotFound { .. }));
        assert!(chosen.describe().contains("Nonesuch Hand"));
    }

    #[test]
    fn mac_roman_names_decode() {
        assert_eq!(mac_roman(b"Bradley Hand"), "Bradley Hand");
        assert_eq!(mac_roman(&[0x8E]), "é");
        assert_eq!(mac_roman(&[0xFF]).chars().count(), 1);
    }

    #[test]
    fn a_regular_upright_face_is_preferred() {
        let face = |italic, weight| Face {
            path: PathBuf::new(),
            index: 0,
            family: "Hand".into(),
            handwriting: true,
            italic,
            weight,
        };
        let faces = [face(true, 400), face(false, 700), face(false, 400)];
        assert_eq!(best(&faces), Some(&faces[2]));
    }
}
