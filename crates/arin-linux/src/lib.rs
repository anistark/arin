//! Linux renderer and capture backends.
//!
//! # Status: reserved for 0.4
//!
//! Empty by design. The crate exists so the workspace matches the documented
//! architecture, and so the name is taken.
//!
//! The plan, when the cycle comes:
//!
//! - Renderer on the wlr layer shell, drawn with wgpu. Read wayscriber before writing
//!   anything, since it solves this exact problem and is worth following closely.
//! - The particle system ported to wgpu. The ember trail is the one visual that has to
//!   reach parity with the Mac renderer. Everything else can differ slightly.
//! - Capture through the xdg desktop portal, including the consent dialog flow.
//! - An X11 fallback path.
//!
//! Compositor support is KDE Plasma, Hyprland, and sway. **GNOME is out of scope** and
//! stays that way: it does not implement layer shell, and there is no way to put a
//! click-through overlay on top of it without one.

#![cfg(target_os = "linux")]
#![warn(missing_docs)]
