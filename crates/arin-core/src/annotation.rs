//! What the daemon is currently showing.

use crate::contrast::{self, Rgb};
use crate::fingerprint::Fingerprint;
use crate::policy::Rendering;
use crate::signature::Shift;
use arin_protocol::{
    Anchor, AnnotationId, DisplayId, LogicalPoint, SessionId, StrokeStyle, TextboxStyle,
};
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

    /// Record what was under this mark when it was drawn.
    ///
    /// Read back after the daemon follows a scroll, to check the mark landed on the same
    /// content rather than being carried somewhere arbitrary. See [`crate::fingerprint`].
    #[must_use]
    pub fn with_fingerprint(mut self, fingerprint: Option<Fingerprint>) -> Self {
        self.anchor.content_hash = fingerprint.map(|f| f.encode());
        self
    }

    /// What was under this mark when it was drawn, if it was recorded.
    pub fn fingerprint(&self) -> Option<Fingerprint> {
        self.anchor
            .content_hash
            .as_deref()
            .and_then(Fingerprint::parse)
    }

    /// Move by a logical offset, following content that shifted underneath.
    ///
    /// Everything the mark draws moves, not just the anchor. A point carries its target
    /// and a path carries every vertex, and leaving either behind would slide the anchor
    /// out from under the ink it describes.
    pub fn translate(&mut self, shift: Shift) {
        self.anchor.screen_rect.x += shift.dx;
        self.anchor.screen_rect.y += shift.dy;
        match &mut self.kind {
            AnnotationKind::Point { at, .. } => {
                at.x += shift.dx;
                at.y += shift.dy;
            }
            AnnotationKind::Path { points, .. }
            | AnnotationKind::Highlight {
                path: Some(points), ..
            } => {
                for point in points {
                    point.x += shift.dx;
                    point.y += shift.dy;
                }
            }
            // Drawn from the anchor alone, which has already moved.
            AnnotationKind::Highlight { path: None, .. } | AnnotationKind::Textbox { .. } => {}
        }
    }

    /// Whether any of this is still on a display of the given size.
    ///
    /// A mark carried entirely off the edge by a scroll is not a mark any more. Clearing
    /// it is both tidier than drawing into nothing and truthful: the thing it pointed at
    /// has left the screen.
    pub fn is_on_screen(&self, logical_size: [f64; 2]) -> bool {
        let rect = self.anchor.screen_rect;
        rect.x < logical_size[0]
            && rect.y < logical_size[1]
            && rect.x + rect.width > 0.0
            && rect.y + rect.height > 0.0
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
        /// The loop to stroke, when the region is circled by hand.
        ///
        /// `None` outlines the anchor, which is the ruled rectangle a renderer can draw
        /// from the anchor alone. A sketched loop cannot be, so the daemon computes it
        /// once and hands it over as vertices, exactly as it does for an arrow. Computing
        /// it once is also what keeps it still: a shape rebuilt on each redraw would
        /// re-roll its wobble every time the content under it scrolled.
        path: Option<Vec<LogicalPoint>>,
    },
    /// A block of explanatory text. Display only.
    Textbox {
        /// The text to render.
        text: String,
        /// What the text is for. Resolved from the wire's optional field before it is
        /// stored, so a renderer never holds an opinion about the default.
        style: TextboxStyle,
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
            AnnotationKind::Highlight {
                label: None,
                path: None,
            },
        );
        assert!(!annotation.is_expired(annotation.created + Duration::from_secs(86_400)));
    }

    /// A point's target is separate from its anchor, so moving one without the other
    /// puts the orb somewhere the anchor no longer describes.
    #[test]
    fn moving_a_point_moves_what_it_points_at() {
        let mut annotation = Annotation::new(
            next_session_id(),
            Anchor::new(LogicalRect::new(100.0, 200.0, 40.0, 40.0), DisplayId(1)),
            AnnotationKind::Point {
                at: LogicalPoint::new(120.0, 220.0),
                label: None,
                rendering: Rendering::Point,
            },
        );

        annotation.translate(Shift { dx: 5.0, dy: -30.0 });

        assert_eq!(annotation.anchor.screen_rect.x, 105.0);
        assert_eq!(annotation.anchor.screen_rect.y, 170.0);
        let AnnotationKind::Point { at, .. } = annotation.kind else {
            panic!("expected a point");
        };
        assert_eq!(at, LogicalPoint::new(125.0, 190.0));
    }

    /// A path draws from its vertices, not its bounding box, so every vertex travels.
    #[test]
    fn moving_a_path_moves_every_vertex() {
        let points = vec![LogicalPoint::new(10.0, 10.0), LogicalPoint::new(50.0, 30.0)];
        let mut annotation = Annotation::new(
            next_session_id(),
            Anchor::new(LogicalRect::new(10.0, 10.0, 40.0, 20.0), DisplayId(1)),
            AnnotationKind::Path {
                points,
                style: None,
            },
        );

        annotation.translate(Shift { dx: -4.0, dy: 6.0 });

        let AnnotationKind::Path { points, .. } = annotation.kind else {
            panic!("expected a path");
        };
        assert_eq!(
            points,
            vec![LogicalPoint::new(6.0, 16.0), LogicalPoint::new(46.0, 36.0)]
        );
    }

    /// A sketched highlight carries its own loop, so following a scroll moves the ink
    /// rather than redrawing it. Recomputing the shape against the moved anchor would
    /// re-roll its wobble on every tick, and the mark would shimmer for as long as the
    /// page kept moving.
    #[test]
    fn moving_a_sketched_highlight_moves_its_loop() {
        let rect = LogicalRect::new(100.0, 200.0, 340.0, 90.0);
        let drawn = crate::sketch::encircle(rect);
        let mut annotation = Annotation::new(
            next_session_id(),
            Anchor::new(rect, DisplayId(1)),
            AnnotationKind::Highlight {
                label: None,
                path: Some(drawn.clone()),
            },
        );

        annotation.translate(Shift {
            dx: 12.0,
            dy: -40.0,
        });

        let AnnotationKind::Highlight {
            path: Some(moved), ..
        } = annotation.kind
        else {
            panic!("expected a sketched highlight");
        };
        let expected: Vec<LogicalPoint> = drawn
            .iter()
            .map(|p| LogicalPoint::new(p.x + 12.0, p.y - 40.0))
            .collect();
        assert_eq!(moved, expected, "the loop did not travel with its anchor");
    }

    #[test]
    fn a_mark_carried_off_the_edge_is_no_longer_on_screen() {
        const DISPLAY: [f64; 2] = [1728.0, 1117.0];
        let mut annotation = Annotation::new(
            next_session_id(),
            Anchor::new(LogicalRect::new(100.0, 40.0, 200.0, 60.0), DisplayId(1)),
            AnnotationKind::Highlight {
                label: None,
                path: None,
            },
        );
        assert!(annotation.is_on_screen(DISPLAY));

        // Still hanging over the top edge by most of its height.
        annotation.translate(Shift { dx: 0.0, dy: -80.0 });
        assert!(annotation.is_on_screen(DISPLAY));

        // And now entirely above it.
        annotation.translate(Shift { dx: 0.0, dy: -40.0 });
        assert!(!annotation.is_on_screen(DISPLAY));
    }

    #[test]
    fn a_fingerprint_survives_the_anchor_it_rides_on() {
        let annotation = Annotation::new(
            next_session_id(),
            anchor(),
            AnnotationKind::Highlight {
                label: None,
                path: None,
            },
        );
        assert_eq!(annotation.fingerprint(), None);

        let recorded = Fingerprint::parse(&"7f".repeat(36)).expect("a well formed fingerprint");
        let annotation = annotation.with_fingerprint(Some(recorded.clone()));
        assert_eq!(annotation.fingerprint(), Some(recorded));
        assert!(annotation.anchor.content_hash.is_some());
    }

    #[test]
    fn a_ttl_expires_on_schedule() {
        let annotation = Annotation::new(
            next_session_id(),
            anchor(),
            AnnotationKind::Highlight {
                label: None,
                path: None,
            },
        )
        .with_ttl(Some(Duration::from_secs(10)));

        assert!(!annotation.is_expired(annotation.created + Duration::from_secs(9)));
        assert!(annotation.is_expired(annotation.created + Duration::from_secs(10)));
    }
}
