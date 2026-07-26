//! macOS renderer and capture backends.
//!
//! # Status: 0.1 scaffold
//!
//! The types exist and the trait seams are wired, but the platform work is not written.
//! Every method returns an error rather than panicking, so the daemon runs end to end
//! against this crate and fails honestly at the point where drawing would happen.
//!
//! The remaining work, in the order it should be done. The overlay window is the risky
//! part and belongs first, before any protocol polish:
//!
//! 1. A transparent, click-through, non-activating `NSPanel` per display, on all Spaces.
//! 2. The Core Animation orb: three concentric radial gradients plus a particle emitter,
//!    driving the five states in [`arin_core::OrbState`].
//! 3. Bezier flight between targets, with the trail spawning along the arc.
//! 4. ScreenCaptureKit capture, and the first-run permission flow for it.
//! 5. The menu bar item: status, permissions, clear, quit.
//!
//! # Rules this crate exists to keep
//!
//! - **It never synthesises input.** No `CGEventPost`, no `CGEventTap`, nothing that
//!   posts to the event stream. Screen Recording is the only permission Arin asks for,
//!   and that stays true only if nothing here reaches for Accessibility.
//! - **Physical pixels stop here.** The protocol is in logical points, and the conversion
//!   to backing pixels happens inside this crate and nowhere above it.
//! - **No bird geometry.** The phoenix is a static brand asset. The daemon renders the
//!   orb, which is the same primitive at every size.
//!
//! # Building
//!
//! Never run `xcodebuild` from a terminal for the Mac app: it invalidates the TCC grant
//! and Screen Recording silently stops working. Build through Xcode.

#![cfg(target_os = "macos")]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

use arin_core::{Annotation, Capture, Error, Frame, OrbState, Renderer, Result};
use arin_protocol::{AnnotationId, DisplayId, DisplayInfo};

fn unimplemented(what: &str) -> Error {
    Error::Renderer(format!(
        "arin-mac is a scaffold: {what} is not implemented yet"
    ))
}

/// Draws the overlay on macOS.
///
/// One transparent, click-through `NSPanel` per display, with the orb and annotations
/// rendered as Core Animation layers inside it.
#[derive(Debug, Default)]
pub struct MacRenderer {
    _private: (),
}

impl MacRenderer {
    /// Construct the renderer.
    ///
    /// Will eventually create one panel per connected display and register for display
    /// reconfiguration notifications.
    pub fn new() -> Result<Self> {
        Err(unimplemented("the overlay panel"))
    }
}

impl Renderer for MacRenderer {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        Err(unimplemented("display enumeration"))
    }

    fn draw(&self, _annotation: &Annotation) -> Result<()> {
        Err(unimplemented("drawing"))
    }

    fn clear(&self, _id: &AnnotationId) -> Result<()> {
        Err(unimplemented("clearing"))
    }

    fn clear_all(&self) -> Result<()> {
        Err(unimplemented("clearing"))
    }

    fn set_orb_state(&self, _state: OrbState) -> Result<()> {
        Err(unimplemented("the orb"))
    }
}

/// Captures the screen on macOS via ScreenCaptureKit.
///
/// Requires the Screen Recording permission. This is the only permission Arin asks for,
/// and the first-run flow that requests it belongs here.
#[derive(Debug, Default)]
pub struct MacCapture {
    _private: (),
}

impl MacCapture {
    /// Construct the capture backend.
    pub fn new() -> Result<Self> {
        Err(Error::Capture(
            "arin-mac is a scaffold: ScreenCaptureKit is not wired up yet".into(),
        ))
    }
}

impl Capture for MacCapture {
    fn capture(&self, _display: DisplayId) -> Result<Frame> {
        Err(Error::Capture(
            "arin-mac is a scaffold: capture is not implemented yet".into(),
        ))
    }
}

/// The orb states this renderer will need to animate.
///
/// Restated here as a compile-time reminder that the vocabulary is fixed by the daemon,
/// not chosen by the renderer. A client never requests a state.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scaffold_fails_rather_than_pretending() {
        let renderer = MacRenderer::default();
        assert!(renderer.displays().is_err());
        assert!(renderer.set_orb_state(OrbState::Idle).is_err());
        assert!(MacCapture::default().capture(DisplayId(1)).is_err());
    }
}
