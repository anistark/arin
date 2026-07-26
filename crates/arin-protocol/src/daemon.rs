//! Daemon to client messages.

use crate::geom::{DisplayInfo, LogicalPoint};
use crate::ids::{AnnotationId, SessionId};
use serde::{Deserialize, Serialize};

/// Anything the daemon can send.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonMessage {
    /// A client message was accepted.
    Ack(Ack),
    /// An annotation is no longer valid.
    Invalidated(Invalidated),
    /// A client message was rejected.
    Error(ProtocolError),
}

/// A client message was accepted.
///
/// A session start acks with `session_id` and no annotation. Everything else acks with
/// `annotation_id`. Resolver-backed requests additionally carry what was resolved and
/// how confident the resolver was.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Ack {
    /// Present on the ack for a session start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// Present on the ack for an annotation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotation_id: Option<AnnotationId>,
    /// Where a query landed. Only for query-form requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_coords: Option<LogicalPoint>,
    /// How sure the resolver was, in `0.0..=1.0`. Only for query-form requests.
    ///
    /// The daemon, not the client, decides what to draw from this. It is reported so
    /// clients can phrase themselves honestly, not so they can pick a rendering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// Scale and size of the display involved, so clients can convert screenshot pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<DisplayInfo>,
}

impl Ack {
    /// Ack a session start.
    pub fn session(session_id: SessionId) -> Self {
        Self {
            session_id: Some(session_id),
            ..Self::default()
        }
    }

    /// Ack an annotation.
    pub fn annotation(annotation_id: AnnotationId) -> Self {
        Self {
            annotation_id: Some(annotation_id),
            ..Self::default()
        }
    }

    /// Attach display metadata.
    #[must_use]
    pub fn with_display(mut self, display: DisplayInfo) -> Self {
        self.display = Some(display);
        self
    }

    /// Attach what a resolver produced.
    #[must_use]
    pub fn with_resolution(mut self, coords: LogicalPoint, confidence: f64) -> Self {
        self.resolved_coords = Some(coords);
        self.confidence = Some(confidence);
        self
    }
}

/// An annotation is no longer valid and has been removed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Invalidated {
    /// The annotation that went away. Absent when a whole display was invalidated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotation_id: Option<AnnotationId>,
    /// Why.
    pub reason: InvalidationReason,
}

impl Invalidated {
    /// One annotation went away.
    pub fn one(annotation_id: AnnotationId, reason: InvalidationReason) -> Self {
        Self {
            annotation_id: Some(annotation_id),
            reason,
        }
    }
}

/// Why an annotation was invalidated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidationReason {
    /// Content moved under the annotation.
    ///
    /// In 0.1 a scroll invalidates every annotation on that display. Later versions
    /// translate annotations with the scroll and only invalidate when they cannot.
    Scroll,
    /// A display was added, removed, or reconfigured.
    DisplayChange,
    /// The owning session ended, or its socket disconnected.
    SessionEnd,
    /// The annotation's time to live expired.
    Ttl,
    /// A reason introduced by a newer daemon than this build knows about.
    #[serde(untagged)]
    Unknown(String),
}

/// A client message was rejected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolError {
    /// Machine-readable cause.
    pub code: ErrorCode,
    /// Human-readable detail. Never load bearing, so do not parse it.
    pub msg: String,
}

impl ProtocolError {
    /// Build an error.
    pub fn new(code: ErrorCode, msg: impl Into<String>) -> Self {
        Self {
            code,
            msg: msg.into(),
        }
    }
}

/// Machine-readable rejection causes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The message parsed as JSON but is not a valid message.
    BadSchema,
    /// The `type` field names a message this daemon does not implement.
    UnknownType,
    /// The line exceeded [`crate::MAX_PAYLOAD_BYTES`].
    PayloadTooLarge,
    /// A query-form request arrived with no resolver configured.
    NoResolver,
    /// A resolver ran and could not ground the query.
    ResolveFailed,
    /// The `display_id` does not name a connected display.
    UnknownDisplay,
    /// The annotation exists but belongs to a different session.
    NotOwner,
    /// The `v` field names an incompatible major version.
    VersionUnsupported,
    /// A code introduced by a newer daemon than this build knows about.
    #[serde(untagged)]
    Unknown(String),
}

impl ErrorCode {
    /// The wire spelling of this code.
    pub fn as_str(&self) -> &str {
        match self {
            Self::BadSchema => "bad_schema",
            Self::UnknownType => "unknown_type",
            Self::PayloadTooLarge => "payload_too_large",
            Self::NoResolver => "no_resolver",
            Self::ResolveFailed => "resolve_failed",
            Self::UnknownDisplay => "unknown_display",
            Self::NotOwner => "not_owner",
            Self::VersionUnsupported => "version_unsupported",
            Self::Unknown(s) => s,
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_codes_survive_a_round_trip() {
        let json = r#""teleported_into_the_sun""#;
        let code: ErrorCode = serde_json::from_str(json).unwrap();
        assert_eq!(code, ErrorCode::Unknown("teleported_into_the_sun".into()));
        assert_eq!(serde_json::to_string(&code).unwrap(), json);
    }

    #[test]
    fn known_codes_take_priority_over_the_fallback() {
        assert_eq!(
            serde_json::from_str::<ErrorCode>(r#""not_owner""#).unwrap(),
            ErrorCode::NotOwner
        );
        assert_eq!(
            serde_json::from_str::<InvalidationReason>(r#""display_change""#).unwrap(),
            InvalidationReason::DisplayChange
        );
    }
}
