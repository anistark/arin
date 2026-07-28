//! What the daemon is currently showing.

use crate::contrast::{self, Rgb};
use crate::policy::Rendering;
use arin_protocol::{Anchor, AnnotationId, DisplayId, LogicalPoint, SessionId, StrokeStyle};
use std::time::{Duration, Instant};

/// A single mark on the screen.
#[derive(Debug, Clone)]
pub struct Annotation {
    /// Opaque identifier, unique across the daemon.
    pub id: AnnotationId,
    /// The session that owns it. Only that session can clear it.
    pub session: SessionId,
    /// Where it is pinned.
    pub anchor: Anchor,
    /// What it looks like.
    pub kind: AnnotationKind,
    /// When it was created, for time to live accounting.
    pub created: Instant,
    /// How long it should live. `None` means until cleared or invalidated.
    pub ttl: Option<Duration>,
    /// What colour to draw it in.
    ///
    /// Always concrete by the time a renderer sees it. The daemon owns this decision, so
    /// a platform backend never has to know about palettes, contrast, or what a client
    /// asked for. See [`crate::contrast`].
    pub color: Rgb,
}

impl Annotation {
    /// Create an annotation with a freshly minted id.
    pub fn new(session: SessionId, anchor: Anchor, kind: AnnotationKind) -> Self {
        Self {
            id: next_annotation_id(),
            session,
            anchor,
            kind,
            created: Instant::now(),
            ttl: None,
            color: contrast::DEFAULT,
        }
    }

    /// Set the colour to draw in.
    #[must_use]
    pub fn with_color(mut self, color: Rgb) -> Self {
        self.color = color;
        self
    }

    /// Set a time to live.
    #[must_use]
    pub fn with_ttl(mut self, ttl: Option<Duration>) -> Self {
        self.ttl = ttl;
        self
    }

    /// The display this is drawn on.
    pub fn display_id(&self) -> DisplayId {
        self.anchor.display_id
    }

    /// Whether the time to live has run out as of `now`.
    pub fn is_expired(&self, now: Instant) -> bool {
        self.ttl
            .is_some_and(|ttl| now.duration_since(self.created) >= ttl)
    }
}

/// The kinds of mark the daemon can draw.
///
/// There is no variant here that accepts input. Text boxes render text and nothing
/// else, and the overlay is fully click through with no controls in it.
#[derive(Debug, Clone, PartialEq)]
pub enum AnnotationKind {
    /// The orb, settled on a target.
    Point {
        /// Exact target in logical points.
        at: LogicalPoint,
        /// Optional caption.
        label: Option<String>,
        /// Whether to draw the point precisely or fall back to the anchor region.
        rendering: Rendering,
    },
    /// An outlined region.
    Highlight {
        /// Optional caption.
        label: Option<String>,
    },
    /// A block of explanatory text. Display only.
    Textbox {
        /// The text to render.
        text: String,
    },
    /// A freehand path.
    Path {
        /// Ordered vertices in logical points.
        points: Vec<LogicalPoint>,
        /// Stroke appearance. Daemon defaults apply when absent.
        style: Option<StrokeStyle>,
    },
}

/// Mint an annotation id.
///
/// Opaque to clients: the `a_` prefix is a debugging affordance, not a contract.
pub fn next_annotation_id() -> AnnotationId {
    AnnotationId::new(format!("a_{}", short_id()))
}

/// Mint a session id.
pub fn next_session_id() -> SessionId {
    SessionId::new(format!("s_{}", short_id()))
}

fn short_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use arin_protocol::LogicalRect;

    fn anchor() -> Anchor {
        Anchor::new(LogicalRect::new(0.0, 0.0, 10.0, 10.0), DisplayId(1))
    }

    #[test]
    fn ids_are_unique_and_prefixed() {
        let a = next_annotation_id();
        let b = next_annotation_id();
        assert_ne!(a, b);
        assert!(a.as_str().starts_with("a_"));
        assert!(next_session_id().as_str().starts_with("s_"));
    }

    #[test]
    fn annotations_without_a_ttl_never_expire() {
        let annotation = Annotation::new(
            next_session_id(),
            anchor(),
            AnnotationKind::Highlight { label: None },
        );
        assert!(!annotation.is_expired(annotation.created + Duration::from_secs(86_400)));
    }

    #[test]
    fn a_ttl_expires_on_schedule() {
        let annotation = Annotation::new(
            next_session_id(),
            anchor(),
            AnnotationKind::Highlight { label: None },
        )
        .with_ttl(Some(Duration::from_secs(10)));

        assert!(!annotation.is_expired(annotation.created + Duration::from_secs(9)));
        assert!(annotation.is_expired(annotation.created + Duration::from_secs(10)));
    }
}
