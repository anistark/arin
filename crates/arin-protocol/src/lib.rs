//! Wire protocol types for Arin.
//!
//! This crate is the public contract. It is pure types plus validation: no IO, no async
//! runtime, no platform code. It must build and test anywhere.
//!
//! # Shape
//!
//! JSON lines over a Unix domain socket, one object per line, UTF-8. Every message
//! carries a `v` field. The daemon accepts any minor version within its major and
//! ignores fields it does not know, so changes stay additive within a major version.
//!
//! ```
//! use arin_protocol::{ClientMessage, Envelope};
//!
//! let line = r#"{"v":"0.1","type":"point","x":412.0,"y":88.0,"display_id":1}"#;
//! let msg: Envelope<ClientMessage> = serde_json::from_str(line).unwrap();
//! assert!(msg.version.is_compatible_with(arin_protocol::PROTOCOL_VERSION));
//! ```
//!
//! # Coordinates
//!
//! Every coordinate is a logical point paired with an explicit display. Never physical
//! pixels. Physical conversion belongs inside platform crates and nowhere else.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod anchor;
mod client;
mod daemon;
mod envelope;
mod geom;
mod ids;
mod validate;
mod version;

pub use anchor::Anchor;
pub use client::{
    Clear, ClientMessage, Draw, Highlight, HighlightTarget, Point, PointTarget, SessionStart,
    StrokeStyle, Textbox,
};
pub use daemon::{Ack, DaemonMessage, ErrorCode, Invalidated, InvalidationReason, ProtocolError};
pub use envelope::Envelope;
pub use geom::{DisplayId, DisplayInfo, LogicalPoint, LogicalRect};
pub use ids::{AnnotationId, SessionId};
pub use validate::{Validate, ValidationError};
pub use version::{PROTOCOL_VERSION, Version, VersionParseError};

/// Maximum accepted payload for a single line, in bytes.
///
/// Anything larger is rejected with [`ErrorCode::PayloadTooLarge`] rather than buffered.
pub const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
