//! One client's connection: a session, and the messages it sends.

use super::Daemon;
use crate::annotation::{Annotation, AnnotationKind};
use crate::contrast::{self, Footprint};
use crate::error::{Error, Result};
use crate::policy::{OrbState, Rendering};
use crate::session::Session;
use arin_protocol::{
    Ack, Anchor, AnnotationId, ClientMessage, DaemonMessage, DisplayId, Envelope, Highlight,
    HighlightTarget, LogicalPoint, LogicalRect, Point, PointTarget, SessionId, Validate,
};
use std::sync::Arc;

/// How large a region to highlight around a low-confidence resolution, in logical points.
///
/// Deliberately generous. A slightly large highlight reads as intentional, while a
/// confident mark in the wrong place reads as broken.
const UNCERTAIN_REGION: f64 = 120.0;

/// One client connection, and the session it holds.
///
/// A connection owns at most one session. Dropping the connection ends it, which is why
/// a crashed client cannot leave marks on the screen.
pub struct Connection {
    daemon: Arc<Daemon>,
    session: Option<SessionId>,
}

impl Connection {
    /// Open a connection against a daemon. No session exists until the client starts one.
    pub fn new(daemon: Arc<Daemon>) -> Self {
        Self {
            daemon,
            session: None,
        }
    }

    /// The session this connection holds, once started.
    pub fn session(&self) -> Option<&SessionId> {
        self.session.as_ref()
    }

    /// Handle one envelope and produce the reply.
    pub async fn handle(&mut self, envelope: Envelope<ClientMessage>) -> Result<DaemonMessage> {
        if !envelope
            .version
            .is_compatible_with(arin_protocol::PROTOCOL_VERSION)
        {
            return Err(Error::VersionUnsupported(envelope.version));
        }
        envelope.body.validate()?;
        self.dispatch(envelope.body).await
    }

    async fn dispatch(&mut self, message: ClientMessage) -> Result<DaemonMessage> {
        match message {
            ClientMessage::SessionStart(start) => {
                let session = Session::new(start.client_name);
                let id = session.id.clone();
                self.daemon
                    .state
                    .lock()
                    .expect("state lock")
                    .sessions
                    .insert(id.clone(), session);
                self.session = Some(id.clone());
                self.daemon.renderer.set_orb_state(OrbState::Idle)?;
                Ok(DaemonMessage::Ack(Ack::session(id)))
            }

            ClientMessage::Point(point) => self.point(point).await,
            ClientMessage::Highlight(highlight) => self.highlight(highlight).await,

            ClientMessage::Textbox(textbox) => {
                let session = self.require_session()?;
                let anchor = textbox.resolved_anchor()?;
                let display = self.daemon.display(anchor.display_id)?;
                let (anchor_display, anchor_rect) = (anchor.display_id, anchor.screen_rect);
                // A text box is a filled panel, so every pixel of it is ink.
                let look = self.daemon.appearance(
                    None,
                    anchor_display,
                    &Footprint::Area(anchor_rect),
                    anchor_rect,
                );
                let annotation = Annotation::new(
                    session,
                    anchor,
                    AnnotationKind::Textbox { text: textbox.text },
                )
                .with_ttl(self.daemon.ttl_for(textbox.ttl_ms))
                .with_color(look.color)
                .with_fingerprint(look.fingerprint);
                let id = self.daemon.store(annotation)?;
                Ok(DaemonMessage::Ack(
                    Ack::annotation(id).with_display(display),
                ))
            }

            ClientMessage::Draw(draw) => {
                let session = self.require_session()?;
                let display = self.daemon.display(draw.display_id)?;
                let points: Vec<LogicalPoint> = draw.points().collect();
                let bounds = bounding_rect(&points);
                let display_id = draw.display_id;
                let path = points.clone();
                let stroke_width = draw
                    .style
                    .as_ref()
                    .and_then(|s| s.width)
                    .unwrap_or(contrast::STROKE_WIDTH);
                let asked = draw.style.as_ref().and_then(|s| s.color.clone());
                let anchor = Anchor::new(bounds, draw.display_id);
                // Along the stroke, not over the bounding box. The box of a diagonal line
                // is mostly pixels the stroke never touches.
                let look = self.daemon.appearance(
                    asked.as_deref(),
                    display_id,
                    &Footprint::Path {
                        points: path,
                        width: stroke_width,
                    },
                    bounds,
                );
                let annotation = Annotation::new(
                    session,
                    anchor,
                    AnnotationKind::Path {
                        points,
                        style: draw.style,
                    },
                )
                .with_ttl(self.daemon.ttl_for(draw.ttl_ms))
                .with_color(look.color)
                .with_fingerprint(look.fingerprint);
                let id = self.daemon.store(annotation)?;
                Ok(DaemonMessage::Ack(
                    Ack::annotation(id).with_display(display),
                ))
            }

            ClientMessage::Clear(clear) => {
                let session = self.require_session()?;
                let cleared = self.clear(&session, &clear)?;
                Ok(DaemonMessage::Ack(Ack {
                    annotation_id: cleared,
                    ..Ack::default()
                }))
            }

            ClientMessage::SessionEnd => {
                let session = self.require_session()?;
                self.daemon.drop_session(&session);
                self.session = None;
                self.daemon.renderer.set_orb_state(OrbState::Ending)?;
                // Acked, not `invalidated`. Every request gets an ack or an error, which
                // leaves `invalidated` meaning only: something the client did not ask for.
                Ok(DaemonMessage::Ack(Ack::default()))
            }
        }
    }

    async fn point(&mut self, point: Point) -> Result<DaemonMessage> {
        let session = self.require_session()?;
        let display = self.daemon.display(point.display_id)?;

        let (at, rect, confidence) = match point.target()? {
            PointTarget::Coords(at) => (at, None, None),
            // Resolved here because only the daemon knows the display's size, which is
            // the point of the form.
            PointTarget::Named(position) => (position.resolve(display.logical_size), None, None),
            PointTarget::Query(query) => {
                let resolution = self.resolve(query, point.display_id).await?;
                (
                    resolution.point,
                    resolution.rect,
                    Some(resolution.confidence),
                )
            }
        };

        let rendering = confidence.map_or(Rendering::Point, Rendering::for_confidence);
        let anchor_rect = rect.unwrap_or_else(|| region_around(at, UNCERTAIN_REGION));
        let anchor = Anchor::new(anchor_rect, point.display_id);

        let look = self.daemon.appearance(
            None,
            point.display_id,
            &Footprint::Area(anchor_rect),
            anchor_rect,
        );
        let annotation = Annotation::new(
            session,
            anchor,
            AnnotationKind::Point {
                at,
                label: point.label,
                rendering,
            },
        )
        .with_ttl(self.daemon.ttl_for(point.ttl_ms))
        .with_color(look.color)
        .with_fingerprint(look.fingerprint);

        let id = self.daemon.store(annotation)?;
        self.daemon.renderer.set_orb_state(OrbState::Pointing)?;

        let mut ack = Ack::annotation(id).with_display(display);
        if let Some(confidence) = confidence {
            ack = ack.with_resolution(at, confidence);
        }
        Ok(DaemonMessage::Ack(ack))
    }

    async fn highlight(&mut self, highlight: Highlight) -> Result<DaemonMessage> {
        let session = self.require_session()?;
        let display = self.daemon.display(highlight.display_id)?;

        let (rect, resolved) = match highlight.target()? {
            HighlightTarget::Rect(rect) => (rect, None),
            HighlightTarget::Query(query) => {
                let resolution = self.resolve(query, highlight.display_id).await?;
                let rect = resolution
                    .rect
                    .unwrap_or_else(|| region_around(resolution.point, UNCERTAIN_REGION));
                (rect, Some((resolution.point, resolution.confidence)))
            }
        };

        let look = self.daemon.appearance(
            None,
            highlight.display_id,
            &Footprint::Outline {
                rect,
                width: contrast::STROKE_WIDTH,
            },
            rect,
        );
        let annotation = Annotation::new(
            session,
            Anchor::new(rect, highlight.display_id),
            AnnotationKind::Highlight {
                label: highlight.label,
            },
        )
        .with_ttl(self.daemon.ttl_for(highlight.ttl_ms))
        .with_color(look.color)
        .with_fingerprint(look.fingerprint);

        let id = self.daemon.store(annotation)?;

        let mut ack = Ack::annotation(id).with_display(display);
        if let Some((point, confidence)) = resolved {
            ack = ack.with_resolution(point, confidence);
        }
        Ok(DaemonMessage::Ack(ack))
    }

    /// Capture the display and ask the resolver to ground a query against it.
    ///
    /// Captured here rather than inside the resolver, so an adapter never touches the
    /// screen itself and a fake one in a test never has to. The resolver says how much
    /// detail it needs and the daemon is what decides how to get it.
    async fn resolve(&self, query: &str, display: DisplayId) -> Result<crate::traits::Resolution> {
        let resolver = self.daemon.resolver.clone().ok_or(Error::NoResolver)?;
        let frame = self
            .daemon
            .capture
            .capture_detailed(display, resolver.detail())?;

        self.daemon.renderer.set_orb_state(OrbState::Thinking)?;
        let outcome = resolver.resolve(query, &frame).await;
        self.daemon.renderer.set_orb_state(OrbState::Idle)?;

        outcome.map_err(|e| Error::ResolveFailed {
            query: query.to_owned(),
            reason: e.to_string(),
        })
    }

    /// Clear annotations, refusing to touch another session's.
    fn clear(
        &self,
        session: &SessionId,
        clear: &arin_protocol::Clear,
    ) -> Result<Option<AnnotationId>> {
        let mut state = self.daemon.state.lock().expect("state lock");

        if clear.all {
            let doomed: Vec<AnnotationId> = state
                .annotations
                .iter()
                .filter(|(_, a)| &a.session == session)
                .map(|(id, _)| id.clone())
                .collect();
            for id in &doomed {
                state.annotations.remove(id);
            }
            drop(state);
            if !doomed.is_empty() {
                self.daemon.mark_drawn();
            }
            for id in &doomed {
                self.daemon.renderer.clear(id)?;
            }
            return Ok(None);
        }

        let id = clear
            .annotation_id
            .clone()
            .expect("validate rejects a clear with no scope");

        match state.annotations.get(&id) {
            // Not found and not yours are answered identically on purpose: a session
            // must not be able to probe for the existence of another's annotations.
            Some(a) if &a.session != session => Err(Error::NotOwner),
            None => Err(Error::NotOwner),
            Some(_) => {
                state.annotations.remove(&id);
                drop(state);
                self.daemon.mark_drawn();
                self.daemon.renderer.clear(&id)?;
                Ok(Some(id))
            }
        }
    }

    fn require_session(&self) -> Result<SessionId> {
        self.session.clone().ok_or(Error::NoSession)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // A dropped socket implies session_end. Without this, a client that crashes
        // leaves marks on the screen with nothing left to clear them.
        if let Some(session) = self.session.take() {
            self.daemon.drop_session(&session);
        }
    }
}

/// A square region centred on a point, for uncertain resolutions.
fn region_around(point: LogicalPoint, size: f64) -> LogicalRect {
    LogicalRect::new(point.x - size / 2.0, point.y - size / 2.0, size, size)
}

/// The smallest rect containing every point in a path.
fn bounding_rect(points: &[LogicalPoint]) -> LogicalRect {
    let Some(first) = points.first() else {
        return LogicalRect::new(0.0, 0.0, 1.0, 1.0);
    };
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (first.x, first.y, first.x, first.y);
    for p in points {
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
        max_x = max_x.max(p.x);
        max_y = max_y.max(p.y);
    }
    // A perfectly straight path has zero extent in one axis, which is not a drawable
    // rect. Widen it rather than emit an invalid anchor.
    LogicalRect::new(
        min_x,
        min_y,
        (max_x - min_x).max(1.0),
        (max_y - min_y).max(1.0),
    )
}
