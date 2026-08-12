//! The Screen Recording permission on macOS.
//!
//! Arin's implementation of [`arin_core::Permissions`]. Core owns the vocabulary, this owns
//! everything macOS specific about getting the answer and about saying what to do with it.
//!
//! # Why this is a flow and not a check
//!
//! Screen Recording is not a permission a dialog can grant. The system prompt for it offers
//! "Open System Settings" and "Deny", so even a user who wants to say yes has to leave, find
//! Arin in a list, and switch it on. A check that reports a denial and stops is therefore
//! wrong almost every time it fires: the usual state is not refusal, it is a user part way
//! through granting. So [`flow`] prompts, then watches for the switch to flip.
//!
//! # Why a granted permission is still checked with a real capture
//!
//! macOS reports the permission as granted the moment the user flips the switch, but
//! ScreenCaptureKit will not serve a frame to a process that was already running when the
//! grant happened. Preflight says yes and capture fails, which is the one state a user
//! cannot debug from the outside. The answer is to restart, and nothing else will say so,
//! so [`access`] proves the grant with an actual frame rather than trusting the preflight.
//!
//! # Layout
//!
//! - [`tcc`] asks the system, and is the only place the CoreGraphics calls appear.
//! - [`identity`] answers whether a grant can stick to this build at all.
//! - [`settings`] opens the pane with the switch on it.
//! - [`flow`] is the startup sequence, and owns the decision to interrupt the user.

mod flow;
mod identity;
mod settings;
mod tcc;

pub use flow::begin_screen_recording_flow;
pub use identity::Identity;
pub use settings::open_screen_recording_settings;

use crate::capture::MacCapture;
use arin_core::{Access, Capture as _, Permissions};
use arin_protocol::DisplayId;
use objc2_core_graphics::CGMainDisplayID;

/// What to tell someone who has to grant this by hand.
pub const SCREEN_RECORDING_HELP: &str = "open System Settings, go to Privacy and Security, then Screen and System Audio \
     Recording, and switch Arin on";

/// The macOS answer to [`arin_core::Permissions`].
///
/// A unit struct because everything it reports is process global. It exists so the daemon
/// and the command line can hold a `dyn Permissions` and stop naming the platform.
#[derive(Debug, Clone, Copy, Default)]
pub struct MacPermissions;

impl Permissions for MacPermissions {
    fn access(&self) -> Access {
        access()
    }

    fn granted(&self) -> bool {
        screen_recording_granted()
    }

    fn open_settings(&self) -> bool {
        open_screen_recording_settings()
    }

    fn explain(&self, access: Access) -> String {
        explain(access)
    }
}

/// Whether the system reports the permission. Never prompts, never captures.
pub fn screen_recording_granted() -> bool {
    tcc::granted()
}

/// What macOS can pin a Screen Recording grant to for this build.
///
/// Answers the question the permission state cannot: whether granting it will do anything
/// that lasts. See the `identity` module for why a build can fail to have one.
pub fn screen_recording_identity() -> Identity {
    identity::of_this_build()
}

/// Whether screen capture actually works, proven by taking a frame.
///
/// Never call this from the main thread. Capture blocks until ScreenCaptureKit answers, and
/// the handlers it waits on need a working run loop.
pub fn access() -> Access {
    if !screen_recording_granted() {
        // Only on the way to reporting a failure, because this is the moment the answer is
        // worth the fork and exec that finding it costs. A build the system cannot identify
        // will report `Missing` forever no matter what the user does in System Settings, and
        // that is not a conclusion anybody reaches unaided.
        return match identity::of_this_build() {
            Identity::Unstable(_) => Access::Unidentified,
            _ => Access::Missing,
        };
    }

    // Deliberately tiny. This is asking whether a frame arrives at all, not looking at one,
    // and the answer costs a round trip through ScreenCaptureKit either way.
    let main = DisplayId(CGMainDisplayID());
    match MacCapture::downscaled(512).capture(main) {
        Ok(_) => Access::Working,
        Err(e) => {
            tracing::debug!(error = %e, "permission reports granted but capture failed");
            Access::NeedsRestart
        }
    }
}

/// One line describing the state and what to do about it.
pub fn explain(access: Access) -> String {
    match access {
        Access::Working => "screen recording is granted and capture works".into(),
        Access::NotRequired => "screen recording needs no grant on this system".into(),
        Access::NeedsRestart => {
            "screen recording is granted but capture still fails, which means the grant \
             happened after Arin started. Restart Arin."
                .into()
        }
        Access::Missing => {
            format!("screen recording is not granted. To fix it, {SCREEN_RECORDING_HELP}.")
        }
        Access::Unidentified => identity::of_this_build().explain(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_says_what_to_do_about_itself() {
        // The point of the enum is that a user reading one line knows their next move, so an
        // empty or interchangeable explanation is a bug.
        for access in [
            Access::Working,
            Access::NotRequired,
            Access::NeedsRestart,
            Access::Missing,
            Access::Unidentified,
        ] {
            assert!(!explain(access).is_empty(), "{access:?} explains nothing");
        }

        assert!(
            explain(Access::NeedsRestart).contains("Restart"),
            "the restart state is the one a user cannot guess, so it must say the word"
        );
        assert!(
            explain(Access::Missing).contains("System Settings"),
            "a missing permission must name where to grant it"
        );
    }

    #[test]
    fn preflight_agrees_with_the_frame_it_can_take() {
        // Whatever this machine's answer is, the two must not contradict each other:
        // capture working while the permission is absent would mean the check is looking at
        // the wrong thing entirely.
        if access() == Access::Working {
            assert!(
                screen_recording_granted(),
                "a frame arrived without the permission, so the preflight is not the gate"
            );
        }
    }
}
