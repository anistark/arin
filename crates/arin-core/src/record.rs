//! Recording what the screen actually looked like, for calibrating against later.
//!
//! Every threshold in [`crate::signature`] was chosen against synthetic patterns, and real
//! interface does not behave like them. Measured on a laptop display, a correct offset and
//! a knowingly wrong one score within 1.3 of each other, where the same two numbers on a
//! generated pattern are 85 apart. Nothing in the test suite could have caught that,
//! because the test suite generates its own content.
//!
//! Fixing it needs real frames rather than better guesses. This writes the before and
//! after of a tick to disk so a scorer can be tried against them offline, as many times as
//! it takes, without a person sitting at the screen scrolling on request.
//!
//! Off unless `ARIN_RECORD` names a directory. It writes raw captures of the user's screen,
//! so it is opt in, it says loudly what it is doing, and it stops on its own rather than
//! filling a disk.

use crate::traits::Frame;
use arin_protocol::{DisplayId, LogicalRect};
use std::path::{Path, PathBuf};

/// How many pairs to keep before stopping.
///
/// Enough to cover a scrolling session across a few applications, bounded so that leaving
/// it on by accident costs a few hundred megabytes rather than a disk. Most of what it
/// catches on a real screen is a caret blinking, so the useful ones are a minority and the
/// budget has to allow for that.
const LIMIT: usize = 200;

/// Writes frame pairs for offline calibration.
pub struct Recorder {
    dir: Option<PathBuf>,
    written: usize,
}

impl Recorder {
    /// Read the destination from `ARIN_RECORD`, or produce one that does nothing.
    pub fn from_env() -> Self {
        let Some(dir) = std::env::var_os("ARIN_RECORD").map(PathBuf::from) else {
            return Self {
                dir: None,
                written: 0,
            };
        };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(path = %dir.display(), error = %e, "cannot record there, carrying on without");
            return Self {
                dir: None,
                written: 0,
            };
        }
        tracing::warn!(
            path = %dir.display(),
            limit = LIMIT,
            "recording raw screen captures to disk for calibration. Unset ARIN_RECORD to stop."
        );
        Self {
            dir: Some(dir),
            written: 0,
        }
    }

    /// Whether anything is being written at all.
    pub fn is_recording(&self) -> bool {
        self.dir.is_some() && self.written < LIMIT
    }

    /// Keep one before and after pair, with the regions the daemon measured in them.
    ///
    /// Skips pairs where nothing changed. A still screen produces one of these twice a
    /// second and none of them tell a scorer anything it does not already know.
    pub fn keep(
        &mut self,
        display: DisplayId,
        before: &Frame,
        after: &Frame,
        regions: &[LogicalRect],
    ) {
        if !self.is_recording() {
            return;
        }
        let Some(dir) = self.dir.clone() else { return };
        if before.pixels == after.pixels {
            return;
        }

        let index = self.written;
        let stem = format!("{index:03}-display{}", display.0);
        let manifest = serde_json::json!({
            "display": display.0,
            "width": after.width,
            "height": after.height,
            "logical_size": after.logical_size,
            "scale": after.scale,
            "regions": regions,
        });

        let write = |name: &str, bytes: &[u8]| std::fs::write(dir.join(name), bytes);
        let outcome = write(&format!("{stem}-before.bgra"), &before.pixels)
            .and_then(|()| write(&format!("{stem}-after.bgra"), &after.pixels))
            .and_then(|()| write(&format!("{stem}.json"), manifest.to_string().as_bytes()));

        match outcome {
            Ok(()) => {
                self.written += 1;
                if self.written == LIMIT {
                    tracing::warn!(count = LIMIT, "recording limit reached, stopping");
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not write a capture pair"),
        }
    }
}

/// One recorded pair, rebuilt as the frames the daemon measures.
pub struct Recording {
    /// The manifest's stem, for naming a pair in output.
    pub name: String,
    /// The frame captured before whatever the user did.
    pub before: Frame,
    /// The frame captured after it.
    pub after: Frame,
    /// The regions that were being measured when this pair was captured.
    pub regions: Vec<LogicalRect>,
}

/// Read back everything [`Recorder`] wrote to a directory.
///
/// Here rather than in the harnesses that consume it. The format has no schema beyond
/// what [`Recorder::pair`] writes, so a reader living anywhere else is a second definition
/// of it that drifts.
///
/// Anything unreadable is skipped rather than failing the run: one truncated file should
/// not cost the rest of the corpus.
pub fn replay(dir: &Path) -> Vec<Recording> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut manifests: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    manifests.sort();

    manifests.iter().filter_map(|m| one(dir, m)).collect()
}

/// Rebuild a single pair, or skip it.
fn one(dir: &Path, manifest: &Path) -> Option<Recording> {
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(manifest).ok()?).ok()?;
    let name = manifest.file_stem()?.to_string_lossy().to_string();

    let number = |key: &str| json.get(key)?.as_f64();
    let width = number("width")? as u32;
    let height = number("height")? as u32;
    let scale = number("scale")?;
    let display = DisplayId(number("display")? as u32);

    let size = json.get("logical_size")?.as_array()?;
    let logical_size = [size.first()?.as_f64()?, size.get(1)?.as_f64()?];

    let regions = json
        .get("regions")
        .and_then(|r| r.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let v = row.as_array()?;
                    Some(LogicalRect::new(
                        v.first()?.as_f64()?,
                        v.get(1)?.as_f64()?,
                        v.get(2)?.as_f64()?,
                        v.get(3)?.as_f64()?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    let read = |suffix: &str| -> Option<Frame> {
        let pixels = std::fs::read(dir.join(format!("{name}-{suffix}.bgra"))).ok()?;
        // A file shorter than its own dimensions would index out of bounds later.
        if pixels.len() < width as usize * height as usize * 4 {
            return None;
        }
        Some(Frame {
            display,
            scale,
            logical_size,
            width,
            height,
            pixels: std::sync::Arc::from(pixels),
        })
    };

    let (before, after) = (read("before")?, read("after")?);
    Some(Recording {
        name,
        before,
        after,
        regions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn frame(fill: u8) -> Frame {
        Frame {
            display: DisplayId(1),
            scale: 1.0,
            logical_size: [64.0, 32.0],
            width: 64,
            height: 32,
            pixels: Arc::from(vec![fill; 64 * 32 * 4]),
        }
    }

    #[test]
    fn recording_is_off_unless_asked_for() {
        // The environment is shared across tests in this binary, so this asserts the
        // shape rather than manipulating it: with nothing configured there is nothing to
        // write to, and every call is a no-op.
        let mut recorder = Recorder {
            dir: None,
            written: 0,
        };
        assert!(!recorder.is_recording());
        recorder.keep(DisplayId(1), &frame(0), &frame(255), &[]);
        assert_eq!(recorder.written, 0);
    }

    #[test]
    fn a_still_screen_is_not_worth_keeping() {
        let dir = std::env::temp_dir().join("arin-record-test-still");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut recorder = Recorder {
            dir: Some(dir.clone()),
            written: 0,
        };
        recorder.keep(DisplayId(1), &frame(7), &frame(7), &[]);
        assert_eq!(
            recorder.written, 0,
            "identical frames teach a scorer nothing"
        );

        recorder.keep(DisplayId(1), &frame(7), &frame(9), &[]);
        assert_eq!(recorder.written, 1);
        assert!(dir.join("000-display1-before.bgra").exists());
        assert!(dir.join("000-display1.json").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn it_stops_rather_than_filling_a_disk() {
        let recorder = Recorder {
            dir: Some(std::env::temp_dir().join("arin-record-test-limit")),
            written: LIMIT,
        };
        assert!(!recorder.is_recording());
    }
}
