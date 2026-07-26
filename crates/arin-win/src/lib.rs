//! Windows renderer and capture backends.
//!
//! # Status: reserved for 0.6
//!
//! Empty by design. The crate exists so the workspace matches the documented
//! architecture, and so the name is taken.
//!
//! The plan, when the cycle comes:
//!
//! - A layered window renderer using `WS_EX_TRANSPARENT` and `WS_EX_TOPMOST`.
//! - DXGI capture.
//! - A tray icon at parity with the Mac menu bar.
//! - Per-monitor DPI awareness v2. Windows makes the mixed-DPI problem worse than either
//!   other platform, and the logical-points rule is what keeps it survivable.
//! - SmartScreen reputation and signing, which take calendar time rather than work.

#![cfg(target_os = "windows")]
#![warn(missing_docs)]
