//! The two CoreGraphics calls behind the Screen Recording permission.
//!
//! Kept apart from the rest so there is one place that talks to TCC, and so the rule that
//! [`request`] is the only thing in Arin allowed to raise a system dialog is a rule about a
//! file rather than a rule about a habit.

use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};
use std::time::{Duration, Instant};

/// Below this, the system prompt did not appear. See [`request`].
const PROMPT_SHOWN_AFTER: Duration = Duration::from_millis(400);

/// What the system says about the permission, without asking the user anything.
pub fn granted() -> bool {
    CGPreflightScreenCaptureAccess()
}

/// Whether the system actually put a dialog in front of the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prompt {
    /// The dialog appeared and the user answered it.
    Shown,
    /// Nothing appeared, because macOS has asked this user about Arin before.
    Silent,
}

/// Raise the system prompt, and report whether it was actually shown.
///
/// The return value of `CGRequestScreenCaptureAccess` cannot answer that. It reports whether
/// the permission is granted, which for Screen Recording is false either way: the prompt
/// sends the user to System Settings rather than granting anything itself. So the signal is
/// how long the call took. It blocks while the dialog is up, and returns at once when the
/// user has answered it before and the system stays silent.
///
/// A heuristic, and the cost of being wrong is small in both directions now that a
/// [`Prompt::Silent`] answer no longer sends anybody to System Settings on its own. See
/// [`super::flow`] for what does.
pub fn request() -> Prompt {
    let started = Instant::now();
    CGRequestScreenCaptureAccess();
    if started.elapsed() >= PROMPT_SHOWN_AFTER {
        Prompt::Shown
    } else {
        Prompt::Silent
    }
}
