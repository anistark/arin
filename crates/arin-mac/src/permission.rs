//! The Screen Recording permission.
//!
//! # Why this is a flow and not a check
//!
//! Screen Recording is not a permission a dialog can grant. The system prompt for it
//! offers "Open System Settings" and "Deny", so even a user who wants to say yes has to
//! leave, find Arin in a list, and switch it on. A check that reports a denial and stops
//! is therefore wrong almost every time it fires: the usual state is not refusal, it is a
//! user part way through granting.
//!
//! So this prompts once, then watches for the switch to flip.
//!
//! # Why a granted permission is still checked with a real capture
//!
//! macOS reports the permission as granted the moment the user flips the switch, but
//! ScreenCaptureKit will not serve a frame to a process that was already running when the
//! grant happened. Preflight says yes and capture fails, which is the one state a user
//! cannot debug from the outside: everything looks correct and nothing works. The answer
//! is to restart, and nothing else will say so, so [`screen_recording`] proves the grant
//! with an actual frame rather than trusting the preflight.

use crate::capture::MacCapture;
use arin_core::Capture as _;
use arin_protocol::DisplayId;
use objc2_app_kit::NSWorkspace;
use objc2_core_graphics::{
    CGMainDisplayID, CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess,
};
use objc2_foundation::{NSString, NSURL};
use std::time::{Duration, Instant};

/// The Screen Recording list in System Settings.
const SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

/// How long to keep watching for a grant before giving up.
///
/// Long enough for someone to go and find the switch, short enough that a daemon left
/// running for a week is not still polling for something that is never coming.
const WATCH_FOR: Duration = Duration::from_secs(300);

/// How often to look while watching.
const POLL: Duration = Duration::from_secs(2);

/// Below this, the system prompt did not appear. See [`prompt_appeared`].
const PROMPT_SHOWN_AFTER: Duration = Duration::from_millis(400);

/// What to tell someone who has to grant this by hand.
pub const SCREEN_RECORDING_HELP: &str = "open System Settings, go to Privacy and Security, then Screen and System Audio \
     Recording, and switch Arin on";

/// Whether screen capture actually works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenRecording {
    /// Granted, and ScreenCaptureKit served a frame to prove it.
    Working,
    /// The system reports the permission but capture still fails.
    ///
    /// What a grant made while the daemon was already running looks like. Restarting
    /// fixes it, and nothing else will.
    NeedsRestart,
    /// Not granted.
    Missing,
}

impl ScreenRecording {
    /// One line describing the state and what to do about it.
    pub fn explain(self) -> String {
        match self {
            Self::Working => "screen recording is granted and capture works".into(),
            Self::NeedsRestart => {
                "screen recording is granted but capture still fails, which means the \
                 grant happened after Arin started. Restart Arin."
                    .into()
            }
            Self::Missing => {
                format!("screen recording is not granted. To fix it, {SCREEN_RECORDING_HELP}.")
            }
        }
    }
}

/// Whether the system reports the permission. Never prompts.
pub fn screen_recording_granted() -> bool {
    CGPreflightScreenCaptureAccess()
}

/// Whether screen capture actually works, proven by taking a frame.
///
/// Never call this from the main thread. Capture blocks until ScreenCaptureKit answers,
/// and the handlers it waits on need a working run loop.
pub fn screen_recording() -> ScreenRecording {
    if !screen_recording_granted() {
        return ScreenRecording::Missing;
    }

    // Deliberately tiny. This is asking whether a frame arrives at all, not looking at
    // one, and the answer costs a round trip through ScreenCaptureKit either way.
    let main = DisplayId(CGMainDisplayID());
    match MacCapture::downscaled(512).capture(main) {
        Ok(_) => ScreenRecording::Working,
        Err(e) => {
            tracing::debug!(error = %e, "permission reports granted but capture failed");
            ScreenRecording::NeedsRestart
        }
    }
}

/// Open the Screen Recording list in System Settings.
pub fn open_screen_recording_settings() -> bool {
    let Some(url) = NSURL::URLWithString(&NSString::from_str(SETTINGS_URL)) else {
        return false;
    };
    NSWorkspace::sharedWorkspace().openURL(&url)
}

/// Raise the system prompt, and report whether it was actually shown.
///
/// The return value of `CGRequestScreenCaptureAccess` cannot answer that. It reports
/// whether the permission is granted, which for Screen Recording is false either way: the
/// prompt sends the user to System Settings rather than granting anything itself. So the
/// signal is how long the call took. It blocks while the dialog is up, and returns at once
/// when the user has answered it before and the system stays silent.
///
/// A heuristic, and the cost of it being wrong is small in both directions: a spurious
/// true opens System Settings for someone who was already headed there, and a spurious
/// false only means Arin waits to be granted rather than helping the user along.
fn prompt_appeared() -> bool {
    let started = Instant::now();
    CGRequestScreenCaptureAccess();
    started.elapsed() >= PROMPT_SHOWN_AFTER
}

/// Prompt for Screen Recording if it is missing, then watch until it is granted.
///
/// Returns immediately, doing the work on its own thread. Nothing waits on the outcome:
/// the overlay draws perfectly well without capture, and only scroll invalidation is off
/// until the permission lands.
pub fn begin_screen_recording_flow() {
    std::thread::Builder::new()
        .name("arin-permission".into())
        .spawn(screen_recording_flow)
        .expect("spawn the permission thread");
}

fn screen_recording_flow() {
    match screen_recording() {
        ScreenRecording::Working => {
            // At info, alongside the socket and the hotkey. A user checking whether
            // capture works should find the answer in the startup lines rather than
            // having to infer it from the absence of a complaint.
            tracing::info!("screen recording granted, scroll detection is live");
            return;
        }
        ScreenRecording::NeedsRestart => {
            tracing::warn!("{}", ScreenRecording::NeedsRestart.explain());
            return;
        }
        ScreenRecording::Missing => {}
    }

    tracing::info!(
        "screen recording is needed for scroll detection. Annotations will draw either \
         way, but they will not clear when the page moves under them."
    );

    if !prompt_appeared() {
        // No dialog, which means this was answered once before and the system will not
        // ask again. Saying so and stopping is the failure this flow exists to avoid, so
        // take the user to the switch instead.
        tracing::info!("opening System Settings, since macOS will not ask again");
        open_screen_recording_settings();
    }

    watch_for_grant();
}

/// Poll until the permission appears, then say what state it left things in.
fn watch_for_grant() {
    let deadline = Instant::now() + WATCH_FOR;
    while Instant::now() < deadline {
        std::thread::sleep(POLL);
        if !screen_recording_granted() {
            continue;
        }
        match screen_recording() {
            ScreenRecording::Working => {
                tracing::info!("screen recording granted, scroll detection is live");
            }
            // The common ending. The user granted it just now, so ScreenCaptureKit will
            // not serve this process until it restarts.
            other => tracing::warn!("{}", other.explain()),
        }
        return;
    }

    tracing::warn!(
        "gave up waiting for screen recording. Scroll detection stays off for this run. \
         To turn it on, {SCREEN_RECORDING_HELP}, then restart Arin."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_says_what_to_do_about_itself() {
        // The point of the tri-state is that a user reading one line knows their next
        // move, so an empty or interchangeable explanation is a bug.
        let states = [
            ScreenRecording::Working,
            ScreenRecording::NeedsRestart,
            ScreenRecording::Missing,
        ];
        for state in states {
            assert!(!state.explain().is_empty(), "{state:?} explains nothing");
        }
        assert!(
            ScreenRecording::NeedsRestart.explain().contains("Restart"),
            "the restart state is the one a user cannot guess, so it must say the word"
        );
        assert!(
            ScreenRecording::Missing
                .explain()
                .contains("System Settings"),
            "a missing permission must name where to grant it"
        );
    }

    #[test]
    fn the_settings_link_points_at_screen_recording() {
        // A wrong pane here drops the user in Settings with no idea what to switch on.
        assert!(SETTINGS_URL.starts_with("x-apple.systempreferences:"));
        assert!(SETTINGS_URL.ends_with("Privacy_ScreenCapture"));
    }

    #[test]
    fn preflight_agrees_with_the_frame_it_can_take() {
        // Whatever this machine's answer is, the two must not contradict each other:
        // capture working while the permission is absent would mean the check is looking
        // at the wrong thing entirely.
        if matches!(screen_recording(), ScreenRecording::Working) {
            assert!(
                screen_recording_granted(),
                "a frame arrived without the permission, so the preflight is not the gate"
            );
        }
    }
}
