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
use std::path::PathBuf;

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
