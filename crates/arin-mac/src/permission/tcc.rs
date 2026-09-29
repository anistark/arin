//! The two CoreGraphics calls behind the Screen Recording permission.
//!
//! Kept apart from the rest so there is one place that talks to TCC, and so the rule that
//! [`request`] is the only thing in Arin allowed to raise a system dialog is a rule about a
//! file rather than a rule about a habit.

use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};

/// What the system says about the permission, without asking the user anything.
pub fn granted() -> bool {
    CGPreflightScreenCaptureAccess()
}

/// Ask macOS to put its Screen Recording dialog in front of the user.
///
/// Whether the dialog appears is decided by macOS, and this call cannot tell. It returns at
/// once either way, because tccd refuses the request on the spot and has a separate process,
/// `universalAccessAuthWarn`, put the dialog up afterwards. Measured on macOS 26.6: the call
/// returned in about 10ms and the dialog followed. Arin used to read a slow return as the
/// dialog having been shown, and so reported silence while the dialog was on screen.
///
/// macOS shows it whenever it holds no answer for this exact build. That includes the state
/// after an upgrade, where its one row for Arin belongs to the build before, and there it
/// shows it at every call. [`super::flow`] is what keeps that to once per build.
pub fn request() {
    CGRequestScreenCaptureAccess();
}
