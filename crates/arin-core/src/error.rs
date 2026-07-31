//! Daemon errors, and how they map onto the wire.

use arin_protocol::{DisplayId, ErrorCode, ProtocolError, ValidationError, Version};

/// Shorthand for daemon results.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Anything that can go wrong handling a client message.
///
/// Every variant maps to exactly one wire [`ErrorCode`], so a client never has to parse
/// the message text to know what happened.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The socket or filesystem failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// The line was not valid JSON.
    #[error("malformed json: {0}")]
    Json(#[from] serde_json::Error),

    /// The message parsed but is not actionable.
    #[error(transparent)]
    Validation(#[from] ValidationError),

    /// The line was not valid UTF-8.
    #[error("line is not valid utf-8")]
    NotUtf8,

    /// A message other than `session_start` arrived before a session existed.
    #[error("no session: send session_start first")]
    NoSession,

    /// The line exceeded the payload cap.
    #[error("payload exceeds {} bytes", arin_protocol::MAX_PAYLOAD_BYTES)]
    PayloadTooLarge,

    /// The client speaks an incompatible major version.
    #[error("protocol version {0} is not compatible with {ours}", ours = arin_protocol::PROTOCOL_VERSION)]
    VersionUnsupported(Version),

    /// No resolver is configured but a query-form message arrived.
    #[error("query form requires a configured resolver")]
    NoResolver,

    /// A resolver exists and the user has not permitted grounding.
    ///
    /// Separate from [`Self::NoResolver`] on purpose. One says grounding is not set up and
    /// the other says it is set up and was declined, and a client can do something about
    /// exactly one of those.
    #[error("grounding is not permitted: {0}")]
    NotPermitted(String),

    /// A resolver ran and could not ground the query.
    #[error("could not resolve {query:?}: {reason}")]
    ResolveFailed {
        /// What the client asked for.
        query: String,
        /// Why it failed.
        reason: String,
    },

    /// A resolver adapter failed on its own terms.
    ///
    /// Separate from [`Self::ResolveFailed`], which is the daemon reporting that a resolve
    /// did not produce coordinates. This is the adapter saying why, in its own words: a
    /// rejected key, an unreachable host, an answer that did not parse. The daemon wraps
    /// it, so both the query and the cause reach the client.
    #[error("resolver: {0}")]
    Resolver(String),

    /// The `display_id` does not name a connected display.
    #[error("no display with id {0}")]
    UnknownDisplay(DisplayId),

    /// The annotation exists, but in another session.
    #[error("annotation belongs to another session")]
    NotOwner,

    /// The session is already holding as many annotations as it may.
    ///
    /// Reported as `bad_schema` rather than getting a code of its own. A client that has
    /// filled its allowance has sent a request the daemon cannot act on, which is what that
    /// code means, and a new code is wire surface that cannot be taken back for a condition
    /// nothing has hit yet.
    #[error(
        "this session already holds {0} annotations, which is the limit. Clear some before \
         drawing more"
    )]
    TooManyAnnotations(usize),

    /// A platform renderer failed.
    #[error("renderer: {0}")]
    Renderer(String),

    /// A platform capture backend failed.
    #[error("capture: {0}")]
    Capture(String),

    /// The peer failed the credential check.
    #[error("peer rejected: {0}")]
    PeerRejected(String),
}

impl Error {
    /// The wire code for this error.
    pub fn code(&self) -> ErrorCode {
        match self {
            // A malformed line and a well-formed but nonsensical one look the same to a
            // client: it sent something the daemon cannot act on.
            Self::Json(_) | Self::Validation(_) | Self::NotUtf8 | Self::NoSession => {
                ErrorCode::BadSchema
            }
            Self::PayloadTooLarge => ErrorCode::PayloadTooLarge,
            Self::VersionUnsupported(_) => ErrorCode::VersionUnsupported,
            Self::NoResolver => ErrorCode::NoResolver,
            Self::NotPermitted(_) => ErrorCode::NotPermitted,
            Self::ResolveFailed { .. } | Self::Resolver(_) => ErrorCode::ResolveFailed,
            Self::UnknownDisplay(_) => ErrorCode::UnknownDisplay,
            Self::NotOwner => ErrorCode::NotOwner,
            // Internal faults are not the client's fault, but there is no wire code for
            // "our problem" and inventing one would be a protocol change.
            Self::Io(_)
            | Self::Renderer(_)
            | Self::Capture(_)
            | Self::PeerRejected(_)
            | Self::TooManyAnnotations(_) => ErrorCode::BadSchema,
        }
    }

    /// Render as the wire message a client receives.
    pub fn to_wire(&self) -> ProtocolError {
        ProtocolError::new(self.code(), self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_carry_their_wire_code() {
        assert_eq!(Error::NoResolver.code(), ErrorCode::NoResolver);
        assert_eq!(Error::NotOwner.code(), ErrorCode::NotOwner);
        assert_eq!(
            Error::UnknownDisplay(DisplayId(9)).code(),
            ErrorCode::UnknownDisplay
        );
    }

    #[test]
    fn wire_errors_keep_the_message() {
        let wire = Error::NoResolver.to_wire();
        assert_eq!(wire.code, ErrorCode::NoResolver);
        assert!(wire.msg.contains("resolver"));
    }
}
