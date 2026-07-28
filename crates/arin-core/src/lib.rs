//! The Arin daemon.
//!
//! Owns the socket server, the session and annotation state machine, and the seams that
//! platform crates plug into. It contains no platform code and no rendering: everything
//! that touches a screen arrives through [`Renderer`] and [`Capture`], and everything
//! that grounds a natural language query arrives through [`Resolver`].
//!
//! This crate must build and test on any target, with no platform crate in the tree.
//! That is enforced in CI. If a macOS dependency lands here, CI breaks, and that is the
//! intended outcome rather than an inconvenience.
//!
//! # What this crate will never do
//!
//! Synthesise input. No clicks, keystrokes, scrolls, or drags. The daemon renders and
//! nothing else, which is what lets it run with Screen Recording alone and no
//! Accessibility grant. Anything reaching for an event-posting API belongs in a
//! different project.

// Deny rather than forbid: `peer` needs two libc calls to read the credentials of the
// process on the other end of the socket, and grants itself a narrow, documented
// exception. Nothing else in this crate may.
#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod annotation;
pub mod client;
pub mod codec;
pub mod config;
pub mod daemon;
pub mod error;
pub mod noop;
pub mod peer;
pub mod policy;
pub mod scroll;
pub mod server;
pub mod session;
pub mod signature;
pub mod traits;

pub use annotation::{Annotation, AnnotationKind};
pub use client::Client;
pub use config::Config;
pub use daemon::{Connection, Daemon};
pub use error::{Error, Result};
pub use noop::{NoopCapture, NoopRenderer};
pub use policy::{OrbState, Rendering};
pub use scroll::ScrollWatcher;
pub use server::Server;
pub use session::Session;
pub use signature::Signature;
pub use traits::{Capture, Frame, Renderer, Resolution, Resolver};

/// The product name, in the one place it is spelled.
///
/// Binary name, socket file, log target, and menu bar title all derive from this. A
/// rename should be a day of work, not a migration.
pub const APP_NAME: &str = "arin";
