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
//! Complete for 0.1. The overlay, display enumeration, every annotation kind, the orb
//! with its flight and ember trail, ScreenCaptureKit capture and its permission flow,
//! the menu bar item, and the global hotkey all work.
//!
//! # Building
//!
//! Never run `xcodebuild` from a terminal for the Mac app: it invalidates the TCC grant
//! and Screen Recording silently stops working. Build through Xcode.

#![cfg(target_os = "macos")]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

mod caption;
mod capture;
mod display;
mod host;
mod menubar;
mod orb;
mod panel;
mod permission;

pub use capture::MacCapture;
pub use display::Screen;
pub use host::{MacRenderer, known_screens, on_displays_changed};
pub use menubar::{MenuBar, on_clear, on_quit, on_status};
pub use orb::MINIMUM_FEATURED_SIZE;
pub use permission::{
    SCREEN_RECORDING_HELP, ScreenRecording, begin_screen_recording_flow,
    open_screen_recording_settings, screen_recording, screen_recording_granted,
};

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
    // Held for the life of the process. Dropping it takes the icon out of the menu bar.
    let _menu = MenuBar::install(mtm);
    setup(renderer, MacCapture::default());

    // After the daemon, so the overlay is already usable while the user is off granting
    // the permission. Returns at once and does its waiting on its own thread.
    permission::begin_screen_recording_flow();

    app.run();
    unreachable!("the AppKit event loop does not return");
}
