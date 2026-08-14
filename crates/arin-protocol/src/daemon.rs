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
    /// What was brought forward. Only for a focus request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activated: Option<Activated>,
    /// Whether the window turned up. Only for an `await_window` request.
    ///
    /// `false` is an answer rather than a failure: the user was asked to go somewhere and
    /// has not, which is a thing a client should handle rather than an error it should
    /// report. Waiting longer, asking again, or giving up are all reasonable, and only the
    /// client knows which.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appeared: Option<bool>,
}

/// The application a focus request brought forward.
///
/// Reported back because a client names an application loosely and the daemon resolves it,
/// so the only way to know what actually came forward is to be told. An agent that asked
/// for `"slack"` and reads `"Slack"` back can say what it did with some confidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activated {
    /// The application's own name for itself, as the system reports it.
    pub app: String,
    /// Its bundle identifier, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    /// Which browser profile window holds the target, when that is how it was found.
    ///
    /// Present only when the match came from a browser's open tabs rather than from a
    /// window, which is the case where raising the application is not enough on its own: a
    /// browser is one application holding many profile windows, and activation cannot
    /// choose between them. So this is the difference between "Chrome is in front of you"
    /// and "it is in the window called Your Chrome", and the second is the one a user can
    /// act on.
    ///
    /// It is the profile's display name, the label in the browser's own profile switcher.
    /// Nothing else read from the tab index ever reaches a client: not a title, not a URL,
    /// not how many matched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
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

    /// Ack a wait with whether the window turned up.
    pub fn appeared(appeared: bool) -> Self {
        Self {
            appeared: Some(appeared),
            ..Self::default()
        }
    }

    /// Ack a focus request with what came forward.
    ///
    /// Carries no annotation id, because nothing was drawn. This is the one ack that
    /// reports a change to the user's screen rather than to what is over it.
    pub fn activated(app: Activated) -> Self {
        Self {
            activated: Some(app),
            ..Self::default()
        }
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
    /// The user cleared it, from the menu bar or the global hotkey.
    ///
    /// Distinct from `session_end` because the client did not ask for it and may want to
    /// say so. The clear affordance belongs to the user, not to the agent driving.
    Cleared,
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
    /// A resolver exists, and the user has not permitted this daemon to ground for a
    /// client.
    ///
    /// Distinct from [`Self::NoResolver`], which says grounding is not set up. This says it
    /// is set up and was refused, which is a different thing for a client to do something
    /// about: one is a configuration problem and the other is a person declining.
    ///
    /// Drawing is never refused this way. Any peer that reaches the socket can draw, since
    /// a process running as the user could open its own window and draw anyway. Grounding
    /// is the only capability Arin actually holds, because it is the one backed by the
    /// Screen Recording grant.
    NotPermitted,
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
            Self::NotPermitted => "not_permitted",
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
