//! macOS renderer and capture backends.
//!
//! # Threading
//!
//! AppKit owns the main thread, so on macOS the daemon does not. [`launch`] takes over
//! the main thread for the event loop and hands your setup closure a renderer to drive
//! the daemon with from a worker thread. This inverts the usual arrangement, and it is
//! the reason `arin daemon` looks different on macOS than elsewhere.
//!
//! # Rules this crate exists to keep
//!
//! - **It never synthesises input.** No `CGEventPost`, no `CGEventTap`, nothing that
//!   posts to the event stream. Screen Recording is the only permission Arin asks for,
//!   and that stays true only if nothing here reaches for Accessibility. The overlay is
//!   click through, so there is nothing to actuate anyway.
//! - **Physical pixels stop here.** The protocol is in logical points, and the conversion
//!   to backing pixels happens inside this crate and nowhere above it.
//! - **No bird geometry.** The phoenix is a static brand asset. The daemon renders the
//!   orb, which is the same primitive at every size.
//!
//! # Status
//!
//! The overlay panel, display enumeration, the orb, and highlight rendering work. Still
//! to come in 0.1: the particle emitter and bezier flight, ScreenCaptureKit capture and
//! its permission flow, the menu bar item, and the global hotkey.
//!
//! # Building
//!
//! Never run `xcodebuild` from a terminal for the Mac app: it invalidates the TCC grant
//! and Screen Recording silently stops working. Build through Xcode.

#![cfg(target_os = "macos")]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

mod capture;
mod display;
mod host;
mod orb;
mod panel;

pub use capture::MacCapture;
pub use display::Screen;
pub use host::{MacRenderer, known_screens};
pub use orb::MINIMUM_FEATURED_SIZE;

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

/// Take over the main thread and run the overlay.
///
/// Sets up the application, creates one overlay panel per display, hands `setup` a
/// renderer and a capture backend, then runs the AppKit event loop. `setup` is expected
/// to start the daemon on another thread and return promptly.
///
/// Does not return: the event loop runs until the process exits.
///
/// The activation policy is `Accessory`, so Arin has no Dock icon and never becomes the
/// active application. Combined with the non activating panel, using Arin never takes
/// focus away from whatever the user is actually working in.
pub fn launch<F>(setup: F) -> !
where
    F: FnOnce(MacRenderer, MacCapture),
{
    let mtm = MainThreadMarker::new()
        .expect("launch must be called from the main thread, before any runtime starts");

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let renderer = MacRenderer::install(mtm);
    setup(renderer, MacCapture::default());

    app.run();
    unreachable!("the AppKit event loop does not return");
}
