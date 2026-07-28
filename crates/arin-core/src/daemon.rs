//! The session and annotation state machine.
//!
//! Transport-free on purpose: [`Connection::handle`] takes a parsed message and returns
//! a reply, so the whole state machine is testable over a `Vec` of messages with no
//! socket and no display.

use crate::annotation::{Annotation, AnnotationKind};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::policy::{OrbState, Rendering};
use crate::session::Session;
use crate::traits::{Capture, Renderer, Resolver};
use arin_protocol::{
    Ack, Anchor, AnnotationId, ClientMessage, DaemonMessage, DisplayId, DisplayInfo, Envelope,
    Highlight, HighlightTarget, Invalidated, InvalidationReason, LogicalPoint, LogicalRect, Point,
    PointTarget, SessionId, Validate,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// How large a region to highlight around a low-confidence resolution, in logical points.
///
/// Deliberately generous. A slightly large highlight reads as intentional, while a
/// confident mark in the wrong place reads as broken.
const UNCERTAIN_REGION: f64 = 120.0;

/// Shared daemon state and the platform seams it drives.
pub struct Daemon {
    config: Config,
    renderer: Arc<dyn Renderer>,
    capture: Arc<dyn Capture>,
    resolver: Option<Arc<dyn Resolver>>,
    state: Mutex<State>,
    /// Bumped every time the daemon changes what is on screen.
    ///
    /// Scroll detection compares one capture against the last. The overlay is part of
    /// what gets captured, so the daemon's own drawing would otherwise read as the page
    /// moving and invalidate the very annotation that just appeared. This lets the
    /// watcher tell the two apart.
    drawn: AtomicU64,
}

#[derive(Default)]
struct State {
    sessions: HashMap<SessionId, Session>,
    annotations: HashMap<AnnotationId, Annotation>,
}

impl Daemon {
    /// Build a daemon over a renderer and a capture backend.
    pub fn new(config: Config, renderer: Arc<dyn Renderer>, capture: Arc<dyn Capture>) -> Self {
        Self {
            config,
            renderer,
            capture,
            resolver: None,
            state: Mutex::new(State::default()),
            drawn: AtomicU64::new(0),
        }
    }

    /// Attach a resolver, enabling the query form of `point` and `highlight`.
    #[must_use]
    pub fn with_resolver(mut self, resolver: Arc<dyn Resolver>) -> Self {
        self.resolver = Some(resolver);
        self
    }

    /// The active configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The configured resolver, if any.
    pub fn resolver(&self) -> Option<&Arc<dyn Resolver>> {
        self.resolver.as_ref()
    }

    /// How many annotations are currently on screen.
    pub fn annotation_count(&self) -> usize {
        self.state.lock().expect("state lock").annotations.len()
    }

    /// The displays that currently have something drawn on them.
    ///
    /// Scroll detection only needs to watch these. Capturing a display with no
    /// annotation on it is work that can change nothing, and on a machine with virtual
    /// displays attached it is most of the work.
    pub fn displays_in_use(&self) -> Vec<DisplayId> {
        let state = self.state.lock().expect("state lock");
        let mut seen: Vec<DisplayId> = state.annotations.values().map(|a| a.display_id()).collect();
        seen.sort_unstable();
        seen.dedup();
        seen
    }

    /// How many sessions are open.
    pub fn session_count(&self) -> usize {
        self.state.lock().expect("state lock").sessions.len()
    }

    /// How many times the daemon has changed what is on screen.
    ///
    /// Compare across ticks: a change means the last capture cannot be trusted as a
    /// baseline, because the difference is the daemon's own doing.
    pub fn render_generation(&self) -> u64 {
        self.drawn.load(Ordering::Relaxed)
    }

    fn mark_drawn(&self) {
        self.drawn.fetch_add(1, Ordering::Relaxed);
    }

    /// Every connected display.
    pub fn displays(&self) -> Result<Vec<DisplayInfo>> {
        self.renderer.displays()
    }

    /// The capture backend, for the scroll watcher.
    pub fn capture_backend(&self) -> &Arc<dyn Capture> {
        &self.capture
    }

    /// Look up a display, or fail with the code the client expects.
    fn display(&self, id: DisplayId) -> Result<DisplayInfo> {
        self.displays()?
            .into_iter()
            .find(|d| d.id == id)
            .ok_or(Error::UnknownDisplay(id))
    }

    /// Drop every annotation owned by a session and tell the renderer.
    fn drop_session(&self, session: &SessionId) -> Vec<AnnotationId> {
        let mut state = self.state.lock().expect("state lock");
        state.sessions.remove(session);
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
            self.mark_drawn();
        }
        for id in &doomed {
            if let Err(e) = self.renderer.clear(id) {
                tracing::warn!(%id, error = %e, "renderer refused a clear");
            }
        }
        doomed
    }

    /// Invalidate every annotation on a display.
    ///
    /// In 0.1 any scroll on a display takes out everything on it. Translating
    /// annotations with the scroll instead needs the reserved anchor content hash.
    pub fn invalidate_display(
        &self,
        display: DisplayId,
        reason: InvalidationReason,
    ) -> Vec<Invalidated> {
        let mut state = self.state.lock().expect("state lock");
        let doomed: Vec<AnnotationId> = state
            .annotations
            .iter()
            .filter(|(_, a)| a.display_id() == display)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &doomed {
            state.annotations.remove(id);
        }
        drop(state);

        if !doomed.is_empty() {
            self.mark_drawn();
        }
        doomed
            .into_iter()
            .map(|id| {
                if let Err(e) = self.renderer.clear(&id) {
                    tracing::warn!(%id, error = %e, "renderer refused a clear");
                }
                Invalidated::one(id, reason.clone())
            })
            .collect()
    }

    /// Remove every annotation, whoever owns it.
    ///
    /// This is the user's escape hatch rather than a client operation. A session can
    /// only clear its own marks, by design, so nothing a client can send will take down
    /// another client's. The person looking at the screen needs a way to say enough, and
    /// this is it.
    pub fn clear_everything(&self) -> Vec<Invalidated> {
        let mut state = self.state.lock().expect("state lock");
        let doomed: Vec<AnnotationId> = state.annotations.keys().cloned().collect();
        state.annotations.clear();
        drop(state);

        if doomed.is_empty() {
            return Vec::new();
        }
        self.mark_drawn();

        if let Err(e) = self.renderer.clear_all() {
            tracing::warn!(error = %e, "renderer refused a clear");
        }
        doomed
            .into_iter()
            .map(|id| Invalidated::one(id, InvalidationReason::Cleared))
            .collect()
    }

    /// Drop annotations whose time to live has run out.
    pub fn expire_annotations(&self) -> Vec<Invalidated> {
        let now = std::time::Instant::now();
        let mut state = self.state.lock().expect("state lock");
        let doomed: Vec<AnnotationId> = state
            .annotations
            .iter()
            .filter(|(_, a)| a.is_expired(now))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &doomed {
            state.annotations.remove(id);
        }
        drop(state);

        doomed
            .into_iter()
            .map(|id| {
                let _ = self.renderer.clear(&id);
                Invalidated::one(id, InvalidationReason::Ttl)
            })
            .collect()
    }

    fn store(&self, annotation: Annotation) -> Result<AnnotationId> {
        let id = annotation.id.clone();
        self.renderer.draw(&annotation)?;
        self.mark_drawn();
        self.state
            .lock()
            .expect("state lock")
            .annotations
            .insert(id.clone(), annotation);
        Ok(id)
    }
}

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
                let annotation = Annotation::new(
                    session,
                    anchor,
                    AnnotationKind::Textbox { text: textbox.text },
                )
                .with_ttl(self.daemon.config.default_ttl);
                let id = self.daemon.store(annotation)?;
                Ok(DaemonMessage::Ack(
                    Ack::annotation(id).with_display(display),
                ))
            }

            ClientMessage::Draw(draw) => {
                let session = self.require_session()?;
                let display = self.daemon.display(draw.display_id)?;
                let points: Vec<LogicalPoint> = draw.points().collect();
                let anchor = Anchor::new(bounding_rect(&points), draw.display_id);
                let annotation = Annotation::new(
                    session,
                    anchor,
                    AnnotationKind::Path {
                        points,
                        style: draw.style,
                    },
                )
                .with_ttl(self.daemon.config.default_ttl);
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
                Ok(DaemonMessage::Invalidated(Invalidated {
                    annotation_id: None,
                    reason: InvalidationReason::SessionEnd,
                }))
            }
        }
    }

    async fn point(&mut self, point: Point) -> Result<DaemonMessage> {
        let session = self.require_session()?;
        let display = self.daemon.display(point.display_id)?;

        let (at, rect, confidence) = match point.target()? {
            PointTarget::Coords(at) => (at, None, None),
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
        let anchor = Anchor::new(
            rect.unwrap_or_else(|| region_around(at, UNCERTAIN_REGION)),
            point.display_id,
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
        .with_ttl(self.daemon.config.default_ttl);

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

        let annotation = Annotation::new(
            session,
            Anchor::new(rect, highlight.display_id),
            AnnotationKind::Highlight {
                label: highlight.label,
            },
        )
        .with_ttl(self.daemon.config.default_ttl);

        let id = self.daemon.store(annotation)?;

        let mut ack = Ack::annotation(id).with_display(display);
        if let Some((point, confidence)) = resolved {
            ack = ack.with_resolution(point, confidence);
        }
        Ok(DaemonMessage::Ack(ack))
    }

    /// Capture the display and ask the resolver to ground a query against it.
    async fn resolve(&self, query: &str, display: DisplayId) -> Result<crate::traits::Resolution> {
        let resolver = self.daemon.resolver.clone().ok_or(Error::NoResolver)?;
        let frame = self.daemon.capture.capture(display)?;

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
