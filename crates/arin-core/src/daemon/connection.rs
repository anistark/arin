//! One client's connection: a session, and the messages it sends.

use super::Daemon;
use crate::annotation::{Annotation, AnnotationKind};
use crate::contrast::{self, Footprint};
use crate::error::{Error, Result};
use crate::policy::{OrbState, Rendering};
use crate::session::Session;
use arin_protocol::{
    Ack, Anchor, AnnotationId, ArrowEnd, ArrowTarget, ClientMessage, DaemonMessage, DisplayId,
    Envelope, Highlight, HighlightTarget, LogicalPoint, LogicalRect, Point, PointTarget, SessionId,
    Validate, ValidationError,
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
                    AnnotationKind::Textbox {
                        text: textbox.text,
                        style: textbox.style.unwrap_or_default(),
                    },
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

            ClientMessage::Arrow(arrow) => {
                let session = self.require_session()?;
                let display = self.daemon.display(arrow.display_id)?;
                let resolve = |end: &ArrowEnd| -> Result<LogicalPoint> {
                    Ok(match end.target()? {
                        ArrowTarget::Coords(at) => at,
                        // Resolved here because only the daemon knows the display's
                        // size, exactly as a point's named form is.
                        ArrowTarget::Named(position) => position.resolve(display.logical_size),
                    })
                };
                let from = resolve(&arrow.from)?;
                let to = resolve(&arrow.to)?;
                // Validation catches two literal ends spelled the same. Two different
                // spellings of one place only meet after resolution, which is here.
                if from == to {
                    return Err(ValidationError::ZeroLengthArrow.into());
                }
                let bow = arrow.bow.unwrap_or(crate::arrow::NATURAL_BOW);
                let points = crate::arrow::path(from, to, bow);
                let bounds = bounding_rect(&points);
                let stroke_width = arrow
                    .style
                    .as_ref()
                    .and_then(|s| s.width)
                    .unwrap_or(contrast::STROKE_WIDTH);
                let asked = arrow.style.as_ref().and_then(|s| s.color.clone());
                let look = self.daemon.appearance(
                    asked.as_deref(),
                    arrow.display_id,
                    &Footprint::Path {
                        points: points.clone(),
                        width: stroke_width,
                    },
                    bounds,
                );
                // An ordinary path from here on, so an arrow scrolls, expires, and is
                // cleared exactly as one, and renderers never learn arrows exist.
                let annotation = Annotation::new(
                    session,
                    Anchor::new(bounds, arrow.display_id),
                    AnnotationKind::Path {
                        points,
                        style: arrow.style,
                    },
                )
                .with_ttl(self.daemon.ttl_for(arrow.ttl_ms))
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

            ClientMessage::Focus(focus) => self.focus(&focus.app).await,

            ClientMessage::AwaitWindow(wait) => self.await_window(&wait.app, wait.timeout_ms).await,

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

    /// Bring an application's windows to the front.
    ///
    /// The one request that changes the user's screen instead of what is drawn over it, and
    /// so the one with a switch of its own. See [`crate::Config::allow_activation`] for why
    /// that switch is a setting rather than a prompt.
    ///
    /// Requires a session like every other message. Not for permission, which activation
    /// does not have, but because a client that never introduced itself should not be
    /// rearranging the desk, and because the log line is worth a name.
    async fn focus(&mut self, app: &str) -> Result<DaemonMessage> {
        let session = self.require_session()?;

        if !self.daemon.config.allow_activation {
            return Err(Error::ActivationRefused(
                "this daemon will not bring applications forward. Restart it with \
                 --allow-activation if you want it to"
                    .into(),
            ));
        }
        let Some(focus) = self.daemon.focus.clone() else {
            return Err(Error::ActivationRefused(
                "this build has no way to bring an application forward".into(),
            ));
        };

        // Before it happens, and naming the client. Activation is the only thing Arin does
        // that a user could mistake for their machine acting on its own, so the log has to
        // be able to answer "what just took my focus" afterwards.
        tracing::info!(
            client = %self.daemon.client_name(&session),
            app,
            "bringing an application forward"
        );

        let activated = focus.activate(app).map_err(|e| Error::ActivationFailed {
            app: app.to_owned(),
            reason: e.to_string(),
        })?;

        // The request was accepted, not completed. Everything a caller would do next
        // involves looking at the screen, and until this is over the screen is a transition.
        tokio::time::sleep(super::ACTIVATION_SETTLE).await;

        // Nothing is invalidated here on purpose. Activation changes the whole screen, so
        // every mark on it is about to be wrong, and the scroll watcher is what notices
        // that on its next tick. Doing it here as well would race that and report the same
        // invalidation twice.
        Ok(DaemonMessage::Ack(Ack::activated(activated)))
    }

    /// Wait until an application's window is showing, or give up.
    ///
    /// The waiting half of a handoff: a mark says "switch desktops", the user does it, and
    /// this is what notices. Without it an agent draws an instruction and then acts as
    /// though it were followed, which is how a second mark lands on a desktop nobody moved
    /// to.
    ///
    /// Gated on [`crate::Config::allow_activation`] like `focus`, and for the same reason
    /// rather than a weaker one. Answering it needs the same window list, and a daemon that
    /// will not raise an application should not be answering questions about which of them
    /// the user is looking at either.
    ///
    /// Polls rather than subscribing. macOS reports that the active desktop changed only
    /// after it has, and says nothing about which one, so there is no event worth waiting on
    /// that is better than looking.
    async fn await_window(&mut self, app: &str, timeout_ms: Option<u64>) -> Result<DaemonMessage> {
        let session = self.require_session()?;

        if !self.daemon.config.allow_activation {
            return Err(Error::ActivationRefused(
                "this daemon was not started with --allow-activation, so it does not answer \
                 questions about which window you are looking at"
                    .into(),
            ));
        }
        let Some(focus) = self.daemon.focus.clone() else {
            return Err(Error::ActivationRefused(
                "this build cannot tell whether a window is showing".into(),
            ));
        };

        let timeout = timeout_ms
            .map(std::time::Duration::from_millis)
            .unwrap_or(super::ARRIVAL_TIMEOUT);

        tracing::info!(
            client = %self.daemon.client_name(&session),
            app,
            timeout_ms = timeout.as_millis(),
            "waiting for a window to show"
        );

        // Checked before the first sleep, so a client that waits for something already in
        // front of the user returns at once rather than after a tick.
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match focus.is_showing(app) {
                Ok(true) => {
                    tracing::info!(app, "the window turned up");
                    return Ok(DaemonMessage::Ack(Ack::appeared(true)));
                }
                Ok(false) => {}
                // A backend that cannot answer is a different thing from a window that has
                // not appeared, and waiting thirty seconds to report the difference as a
                // timeout would hide it.
                Err(e) => {
                    return Err(Error::ActivationFailed {
                        app: app.to_owned(),
                        reason: e.to_string(),
                    });
                }
            }
            if std::time::Instant::now() >= deadline {
                tracing::info!(app, "gave up waiting for a window");
                return Ok(DaemonMessage::Ack(Ack::appeared(false)));
            }
            tokio::time::sleep(super::ARRIVAL_TICK).await;
        }
    }

    /// Capture the display and ask the resolver to ground a query against it.
    ///
    /// Captured here rather than inside the resolver, so an adapter never touches the
    /// screen itself and a fake one in a test never has to. The resolver says how much
    /// detail it needs and the daemon is what decides how to get it.
    async fn resolve(&self, query: &str, display: DisplayId) -> Result<crate::traits::Resolution> {
        let resolver = self.daemon.resolver.clone().ok_or(Error::NoResolver)?;

        // Before the capture, not after. The whole finding is that grounding makes Arin
        // look at the screen on a client's behalf, so a refused request must not have taken
        // a frame first. Asking afterwards would leave the screen already read by the time
        // the user says no.
        let session = self.require_session()?;
        self.daemon
            .permit_grounding(&session, query, resolver.as_ref())
            .await?;

        let frame = self
            .daemon
            .capture
            .capture_detailed(display, resolver.detail())?;

        // At the moment it happens, not only at startup. The startup line says a resolver
        // that leaves the machine is configured, which is a different claim from a
        // screenshot of this display going out right now, and it is the second one somebody
        // scrolling back through a log needs to find.
        if resolver.is_remote() {
            // Bound out here because `tracing` brings its own `display` into scope inside
            // the macro, which shadows the parameter.
            let display_id = display.0;
            tracing::warn!(
                resolver = resolver.name(),
                display_id,
                "sending a screenshot of this display off the machine"
            );
        }

        self.daemon.renderer.set_orb_state(OrbState::Thinking)?;
        let outcome = resolver.resolve(query, &frame).await;
        self.daemon.renderer.set_orb_state(OrbState::Idle)?;

        outcome.map_err(|e| Error::ResolveFailed {
            query: query.to_owned(),
            reason: somewhere_else(
                &e.to_string(),
                self.daemon.capture_backend().windows_off_desktop(),
            ),
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

/// Add where else to look to a resolver failure, when there is anywhere else.
///
/// A resolver that finds nothing cannot say whether the thing is absent or on a desktop
/// nobody is looking at, and only the second has an action attached.
///
/// The count decides whether to speak and is never quoted. It rests on heuristics that
/// differ per application, and an agent does the same thing whether the answer is one or
/// nine. Worded loosely for the same reason: what is counted is windows not on screen,
/// which takes in minimised and fully covered ones as well as other desktops.
fn somewhere_else(reason: &str, off_desktop: Option<usize>) -> String {
    match off_desktop {
        Some(count) if count > 0 => format!(
            "{reason}. Arin only sees the desktop that is showing, and there are windows it \
             is not showing. If the target is on another desktop, ask the user to switch to \
             it and try again."
        ),
        _ => reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::somewhere_else;

    #[test]
    fn a_failure_with_windows_elsewhere_says_where_else_to_look() {
        let said = somewhere_else("no match for \"the Submit button\"", Some(3));

        assert!(
            said.starts_with("no match"),
            "the resolver's own words go first"
        );
        assert!(
            said.contains("another desktop") && said.contains("ask the user to switch"),
            "knowing there is somewhere else is only useful with what to do about it: {said}"
        );
    }

    /// A digit here would be false precision a client could act on.
    #[test]
    fn the_number_of_windows_elsewhere_is_never_quoted() {
        for count in [1, 3, 9, 274] {
            let said = somewhere_else("no match", Some(count));
            assert!(
                !said.chars().any(|c| c.is_ascii_digit()),
                "the count reached the client for {count}: {said}"
            );
        }
    }

    /// Nothing elsewhere and no way to tell both leave the resolver's own words alone.
    #[test]
    fn a_failure_with_nowhere_else_is_left_exactly_as_it_was() {
        let plain = "no match for \"the Submit button\"";
        assert_eq!(somewhere_else(plain, Some(0)), plain);
        assert_eq!(somewhere_else(plain, None), plain);
    }
}
