//! The Rust API for [Arin](https://github.com/anistark/arin), an annotation layer any
//! agent can draw on.
//!
//! # This crate is not the application
//!
//! Arin is a daemon that draws on your screen, and it is installed as an application
//! rather than with Cargo:
//!
//! ```text
//! brew install --cask arin
//! ```
//!
//! `cargo install arin` will not work and is not meant to. There is no binary here. What
//! is here is the contract for talking to the daemon from Rust, which is everything in
//! [`arin_protocol`] re-exported under the name the project is actually called.
//!
//! # Why a facade
//!
//! Two names for one contract, so that neither audience has to know about the other.
//! Someone writing a client reaches for `arin` because that is the name of the thing they
//! are talking to. Someone implementing the wire format in another language reaches for
//! `arin-protocol` because that is what it describes. Both get the same types.
//!
//! Depend on whichever reads better. This crate tracks the protocol within a compatible
//! range, so the two do not drift.
//!
//! ```
//! use arin::{Anchor, DisplayId, LogicalRect};
//!
//! let rect = LogicalRect::new(412.0, 88.0, 120.0, 32.0);
//! assert!(rect.is_valid());
//!
//! let anchor = Anchor::new(rect, DisplayId(1));
//! assert_eq!(anchor.display_id, DisplayId(1));
//! ```

pub use arin_protocol::*;

/// Compiles the README's examples as doctests.
///
/// Same reason as `arin-protocol`: the README is what someone reads on crates.io before
/// anything else, so its code is held to the standard of code rather than prose that
/// happens to be fenced. It matters more here, because this README's job is to tell people
/// they want `brew` instead, and being wrong about that wastes their time.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct Readme;
