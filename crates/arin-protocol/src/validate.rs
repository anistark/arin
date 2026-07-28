//! Semantic validation, beyond what the type system and serde already enforce.
//!
//! Serde rejects structurally wrong JSON. This catches the rest: messages that parse but
//! cannot be acted on, such as a `point` carrying neither coordinates nor a query.

use crate::daemon::ErrorCode;

/// A message that parsed but cannot be acted on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    /// Neither form of a two-form message was supplied.
    #[error("{message} requires one of: {expected}")]
    MissingTarget {
        /// The message type at fault.
        message: &'static str,
        /// The alternatives that would have been accepted.
        expected: &'static str,
    },

    /// Both forms were supplied and they cannot be reconciled.
    #[error("{message} accepts only one of: {expected}")]
    AmbiguousTarget {
        /// The message type at fault.
        message: &'static str,
        /// The mutually exclusive alternatives.
        expected: &'static str,
    },

    /// A rect was non-finite or had a non-positive extent.
    #[error("{field} is not a drawable rect: width and height must be finite and positive")]
    InvalidRect {
        /// The offending field.
        field: &'static str,
    },

    /// A coordinate was NaN or infinite.
    #[error("{field} contains a non-finite coordinate")]
    NonFiniteCoordinate {
        /// The offending field.
        field: &'static str,
    },

    /// A field that must carry content was empty or whitespace.
    #[error("{field} must not be empty")]
    Empty {
        /// The offending field.
        field: &'static str,
    },

    /// A freehand path had too few points to draw.
    #[error("path needs at least 2 points, got {got}")]
    PathTooShort {
        /// How many points were supplied.
        got: usize,
    },

    /// A named position was not one this daemon knows.
    #[error("{got:?} is not a position: use a name like `top-left`, or `50%,30%`")]
    UnknownPosition {
        /// What was sent, so the client can see what was read.
        got: String,
    },

    /// A percentage fell outside the display.
    ///
    /// Carries the text rather than the parsed number so that [`ValidationError`] stays
    /// `Eq`, which every other variant already is and which the wire tests rely on.
    #[error("{got} is outside the display: a percentage must be between 0% and 100%")]
    PositionOutOfRange {
        /// The offending component, as it was written.
        got: String,
    },

    /// A time to live was zero.
    ///
    /// Its own error rather than a silent clamp, because a zero here is a unit mistake
    /// often enough that drawing a mark and removing it in the same breath is more likely
    /// to be a bug than an intent.
    #[error("ttl_ms must be greater than zero, or omitted to draw until cleared")]
    ZeroTtl,
}

impl ValidationError {
    /// The wire error code this maps to.
    ///
    /// Every validation failure is a schema failure from the client's point of view.
    pub const fn code(&self) -> ErrorCode {
        ErrorCode::BadSchema
    }
}

/// Messages that can be checked beyond their parsed shape.
pub trait Validate {
    /// Check the message. Called by the daemon before anything is rendered.
    fn validate(&self) -> Result<(), ValidationError>;
}
