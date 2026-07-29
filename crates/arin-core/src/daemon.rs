//! The session and annotation state machine.
//!
//! Transport-free on purpose: [`Connection::handle`] takes a parsed message and returns
//! a reply, so the whole state machine is testable over a `Vec` of messages with no
//! socket and no display.

use crate::annotation::{Annotation, AnnotationKind};
use crate::config::Config;
use crate::contrast::{self, Footprint, Rgb};
use crate::error::{Error, Result};
use crate::fingerprint::Fingerprint;
use crate::policy::{OrbState, Rendering};
use crate::session::Session;
use crate::traits::{Capture, Frame, Renderer, Resolver};
use arin_protocol::{
    Ack, Anchor, AnnotationId, ClientMessage, DaemonMessage, DisplayId, DisplayInfo, Envelope,
    Highlight, HighlightTarget, Invalidated, InvalidationReason, LogicalPoint, LogicalRect, Point,
    PointTarget, SessionId, Validate,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

/// How many pending announcements a connection may fall behind by.
///
/// Generous: an invalidation is a handful of bytes and a connection that is not reading
/// is already in trouble. A receiver that overruns this loses the oldest and is told, so
/// the daemon is never blocked by a client that stopped listening.
const ANNOUNCEMENT_BACKLOG: usize = 256;

/// How large a region to highlight around a low-confidence resolution, in logical points.
///
/// Deliberately generous. A slightly large highlight reads as intentional, while a
/// confident mark in the wrong place reads as broken.
const UNCERTAIN_REGION: f64 = 120.0;

/// How far around a mark to look when measuring what moved under it, in logical points.
///
/// Two things pull against each other. Too tight and there is not enough content to
/// correlate against. Too wide and the region takes in the toolbars and neighbouring
/// windows that made the display-wide measurement useless in the first place, which is
/// the failure being fixed and so the one to lean away from.
///
/// The wrong reading of "not enough content" is what makes this tempting to raise. A
/// region with no structure in it is not a failure: it reports no movement, the mark stays
/// where it is, and a plain background is exactly the kind of thing that does not visibly
/// move. Widening buys correlation the mark did not need, at the price of measuring a
/// window it is not in.
///
/// It also bounds how far a scroll can jump and still be followed, since the search only
/// reaches half the region. A flick that throws the page further than that reads as
/// unexplainable and the mark goes, which is the right answer to content that is no longer
/// anywhere near where it was.
const CONTEXT: f64 = 120.0;

/// The patch of screen a mark's movement is measured against.
///
/// The mark's own ink is inside it and is left there rather than masked out. The overlay
/// is in the frame, so between two ticks the mark has not moved and votes for an offset of
/// zero against content that may have scrolled. Masking it would leave holes in the profile
/// that vote for zero exactly as loudly, so it is cheaper to let the surrounding content
/// outvote it and let the residual carry the disagreement.
fn neighbourhood(anchor: LogicalRect, display: [f64; 2]) -> LogicalRect {
    // A fixed margin, not one that grows with the mark. Scaling it by the anchor was the
    // obvious way to keep the mark a small share of what is being correlated, and it
    // quietly undid the whole change: a highlight six hundred points wide got a margin of
    // six hundred, which on a laptop display is the entire screen and therefore the same
    // display-wide answer this replaced.
    //
    // Letting a large mark occupy most of its own region is the better trade. A highlight
    // is an outline, so the content inside it is visible and is precisely the content the
    // mark is about. Only a text box is opaque over its own anchor, and it is the one kind
    // that has no target underneath to keep up with.
    let left = (anchor.x - CONTEXT).max(0.0);
    let top = (anchor.y - CONTEXT).max(0.0);
    let right = (anchor.x + anchor.width + CONTEXT).min(display[0]);
    let bottom = (anchor.y + anchor.height + CONTEXT).min(display[1]);

    LogicalRect::new(left, top, (right - left).max(1.0), (bottom - top).max(1.0))
}

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
    /// Invalidations nobody asked for, on their way to whoever owns them.
    announcements: broadcast::Sender<Announcement>,
}

/// An invalidation, and the session whose annotation it concerns.
///
/// The session travels alongside rather than inside [`Invalidated`], because the wire
/// message must not carry it: a client learning that some *other* session's annotation
/// went away would be told something it has no business knowing. Connections filter on
/// this before writing anything out.
#[derive(Debug, Clone)]
pub struct Announcement {
    /// Who owns the annotation this concerns.
    pub session: SessionId,
    /// What to tell them.
    pub event: Invalidated,
}

/// What the daemon learned from one look at the region it is about to mark.
struct Appearance {
    /// The colour to draw in.
    color: Rgb,
    /// What is under the mark, for checking it later. `None` when nothing was captured.
    fingerprint: Option<Fingerprint>,
}

/// What became of a display's annotations when its content moved.
#[derive(Debug, Default)]
pub struct Followed {
    /// Marks that kept up with the content and were redrawn.
    pub moved: Vec<AnnotationId>,
    /// Marks that could not be followed, and the invalidations sent for them.
    pub invalidated: Vec<Invalidated>,
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
            announcements: broadcast::channel(ANNOUNCEMENT_BACKLOG).0,
        }
    }

    /// Listen for invalidations the daemon raises on its own.
    ///
    /// One per connection. Everything sent here is unsolicited: a scroll, a time to live
    /// running out, or the user clearing the screen. Replies to requests never come this
    /// way, which is what keeps the two apart on the socket.
    pub fn subscribe(&self) -> broadcast::Receiver<Announcement> {
        self.announcements.subscribe()
    }

    /// Tell whoever owns these annotations that they went away.
    ///
    /// Failure means nothing is listening, which is the normal state for a daemon with no
    /// clients connected and not worth reporting.
    fn announce(&self, owners: Vec<(AnnotationId, SessionId)>, reason: &InvalidationReason) {
        for (id, session) in owners {
            let _ = self.announcements.send(Announcement {
                session,
                event: Invalidated::one(id, reason.clone()),
            });
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
        let doomed: Vec<(AnnotationId, SessionId)> = state
            .annotations
            .iter()
            .filter(|(_, a)| a.display_id() == display)
            .map(|(id, a)| (id.clone(), a.session.clone()))
            .collect();
        for (id, _) in &doomed {
            state.annotations.remove(id);
        }
        drop(state);

        if !doomed.is_empty() {
            self.mark_drawn();
        }
        for (id, _) in &doomed {
            if let Err(e) = self.renderer.clear(id) {
                tracing::warn!(%id, error = %e, "renderer refused a clear");
            }
        }
        self.announce(doomed.clone(), &reason);
        doomed
            .into_iter()
            .map(|(id, _)| Invalidated::one(id, reason.clone()))
            .collect()
    }

    /// Drop marks whose display went away or moved out from under them.
    ///
    /// Displays are not fixed. A monitor is unplugged, a laptop lid closes, a resolution
    /// changes, and the arrangement a mark was placed against is no longer the one on the
    /// desk. Two things follow from that, and neither used to be handled:
    ///
    /// A mark on a display that is gone cannot be seen, cannot be cleared from the menu
    /// bar because there is no panel to clear it from, and is never invalidated on its
    /// own. It sits in the daemon's state for the life of the session, keeping the scroll
    /// watcher asking a backend for a display that does not exist.
    ///
    /// A mark on a display that shrank may now be outside it, which is the same failure a
    /// scroll causes and gets the same answer.
    ///
    /// `display_change` has been on the wire since 0.1 with nothing emitting it. This is
    /// what emits it.
    pub fn reconcile_displays(&self) -> Vec<Invalidated> {
        let Ok(present) = self.displays() else {
            // No idea what is connected. Guessing here would either drop every mark on
            // screen or none, and doing nothing is the recoverable half of that.
            tracing::warn!("could not enumerate displays, leaving marks alone");
            return Vec::new();
        };

        let mut state = self.state.lock().expect("state lock");
        let doomed: Vec<(AnnotationId, SessionId)> = state
            .annotations
            .iter()
            .filter(|(_, annotation)| {
                match present.iter().find(|d| d.id == annotation.display_id()) {
                    None => true,
                    Some(display) => !annotation.is_on_screen(display.logical_size),
                }
            })
            .map(|(id, annotation)| (id.clone(), annotation.session.clone()))
            .collect();
        for (id, _) in &doomed {
            state.annotations.remove(id);
        }
        drop(state);

        if doomed.is_empty() {
            return Vec::new();
        }
        self.mark_drawn();

        for (id, _) in &doomed {
            // A clear for a display that is gone is expected to fail, so this is quieter
            // than the other clear paths.
            if let Err(e) = self.renderer.clear(id) {
                tracing::debug!(%id, error = %e, "clearing a mark from a display that changed");
            }
        }
        self.announce(doomed.clone(), &InvalidationReason::DisplayChange);
        tracing::info!(
            count = doomed.len(),
            "displays changed, dropped marks that went with them"
        );

        doomed
            .into_iter()
            .map(|(id, _)| Invalidated::one(id, InvalidationReason::DisplayChange))
            .collect()
    }

    /// Draw every surviving mark again.
    ///
    /// For after something outside the daemon threw away what was on screen. A platform
    /// renderer rebuilding its overlay for a display that changed cannot repopulate it,
    /// because the annotations live here and it has never been told what they are.
    ///
    /// Not a general refresh. Call it when the screen has been emptied behind the daemon's
    /// back, and reconcile first so nothing is redrawn onto a display that is gone.
    pub fn redraw_all(&self) -> usize {
        let annotations: Vec<Annotation> = self
            .state
            .lock()
            .expect("state lock")
            .annotations
            .values()
            .cloned()
            .collect();

        if annotations.is_empty() {
            return 0;
        }
        self.mark_drawn();

        let mut drawn = 0;
        for annotation in &annotations {
            match self.renderer.draw(annotation) {
                Ok(()) => drawn += 1,
                Err(e) => {
                    tracing::warn!(id = %annotation.id, error = %e, "renderer refused a redraw")
                }
            }
        }
        drawn
    }

    /// Remove every annotation, whoever owns it.
    ///
    /// This is the user's escape hatch rather than a client operation. A session can
    /// only clear its own marks, by design, so nothing a client can send will take down
    /// another client's. The person looking at the screen needs a way to say enough, and
    /// this is it.
    pub fn clear_everything(&self) -> Vec<Invalidated> {
        let mut state = self.state.lock().expect("state lock");
        let doomed: Vec<(AnnotationId, SessionId)> = state
            .annotations
            .iter()
            .map(|(id, a)| (id.clone(), a.session.clone()))
            .collect();
        state.annotations.clear();
        drop(state);

        if doomed.is_empty() {
            return Vec::new();
        }
        self.mark_drawn();

        if let Err(e) = self.renderer.clear_all() {
            tracing::warn!(error = %e, "renderer refused a clear");
        }
        self.announce(doomed.clone(), &InvalidationReason::Cleared);
        doomed
            .into_iter()
            .map(|(id, _)| Invalidated::one(id, InvalidationReason::Cleared))
            .collect()
    }

    /// Drop annotations whose time to live has run out.
    ///
    /// Called on a timer. Sweeping rather than arming one timer per annotation, because
    /// the alternative is a task per mark that has to be cancelled whenever a clear, a
    /// scroll, or a session end gets there first.
    pub fn expire_annotations(&self) -> Vec<Invalidated> {
        let now = std::time::Instant::now();
        let mut state = self.state.lock().expect("state lock");
        let doomed: Vec<(AnnotationId, SessionId)> = state
            .annotations
            .iter()
            .filter(|(_, a)| a.is_expired(now))
            .map(|(id, a)| (id.clone(), a.session.clone()))
            .collect();
        for (id, _) in &doomed {
            state.annotations.remove(id);
        }
        drop(state);

        if doomed.is_empty() {
            return Vec::new();
        }
        // An expiry changes what is on screen just as much as a draw does, so scroll
        // detection has to re-baseline. Without this the disappearing mark reads as the
        // page moving and takes every other annotation on that display with it.
        self.mark_drawn();

        for (id, _) in &doomed {
            if let Err(e) = self.renderer.clear(id) {
                tracing::warn!(%id, error = %e, "renderer refused a clear");
            }
        }
        self.announce(doomed.clone(), &InvalidationReason::Ttl);
        doomed
            .into_iter()
            .map(|(id, _)| Invalidated::one(id, InvalidationReason::Ttl))
            .collect()
    }

    /// What one look at the screen tells the daemon about a mark it is about to draw.
    ///
    /// Two answers from one capture: the colour to draw in, and a record of the content
    /// being marked so a later scroll can check the mark stayed on it. The capture used
    /// to buy only the colour, which made it hard to justify at around 100ms per
    /// positioned annotation. It now also buys the only per-annotation evidence the
    /// daemon has that following a scroll put the mark somewhere sensible.
    fn appearance(
        &self,
        asked: Option<&str>,
        screen: DisplayId,
        footprint: &Footprint,
        region: LogicalRect,
    ) -> Appearance {
        // A colour the client named wins outright: it asked for something specific and
        // second-guessing it would make the field a suggestion. An unparseable one falls
        // through to the adaptive pick rather than to the default, since a client that
        // cared enough to name a colour is better served by a legible one than by
        // whatever the palette starts with.
        let named = asked.and_then(|name| {
            let parsed = Rgb::parse(name);
            if parsed.is_none() {
                tracing::debug!(color = name, "unparseable colour, choosing one");
            }
            parsed
        });

        if !self.config.adaptive_color {
            return Appearance {
                color: named.unwrap_or(contrast::DEFAULT),
                fingerprint: None,
            };
        }

        // A capture that fails is not an error. Screen Recording may not be granted yet,
        // and a mark in the default colour is a far better outcome than a refused
        // request, so this falls back quietly and says so once at debug level.
        match self.capture.capture(screen) {
            Ok(frame) => Appearance {
                color: named.unwrap_or_else(|| {
                    let picked = contrast::pick(&frame, footprint);
                    // Logged because it is otherwise unobservable. A screenshot goes
                    // through colour management on the way out, so the pixels that come
                    // back are not the ones asked for and cannot be compared against the
                    // palette.
                    tracing::debug!(
                        color = %picked,
                        adapted = picked != contrast::DEFAULT,
                        "chose an annotation colour"
                    );
                    picked
                }),
                fingerprint: Fingerprint::of(&frame, region),
            },
            Err(e) => {
                tracing::debug!(%screen, error = %e, "no frame to sample, using the default colour");
                Appearance {
                    color: named.unwrap_or(contrast::DEFAULT),
                    fingerprint: None,
                }
            }
        }
    }

    /// Follow content that moved, and drop whatever could not be followed.
    ///
    /// Two things have to hold for a mark to survive. It has to still be on the display,
    /// and where it landed has to still look like what it was pointing at. The second is
    /// the one that matters: a display-wide shift is right for most of the screen and
    /// wrong for any part that did not move with the rest, and the anchor's fingerprint is
    /// the only evidence the daemon has about which of those a given mark is.
    ///
    /// `frame` is the capture the movement was detected from. Without it, and without a
    /// fingerprint recorded when the mark was made, a mark is followed on the display-wide
    /// estimate alone.
    /// The patches of screen that would be correlated for the marks on a display.
    ///
    /// For recording alongside a capture pair, so a scorer tried against that pair offline
    /// is looking at the same regions the daemon was.
    pub fn measured_regions(&self, display: DisplayId) -> Vec<LogicalRect> {
        let Ok(info) = self.display(display) else {
            return Vec::new();
        };
        let state = self.state.lock().expect("state lock");
        state
            .annotations
            .values()
            .filter(|a| a.display_id() == display)
            .map(|a| neighbourhood(a.anchor.screen_rect, info.logical_size))
            .collect()
    }

    /// Follow whatever moved under each mark on a display, one mark at a time.
    ///
    /// The display-wide version of this was wrong, and wrong in a way only a real screen
    /// showed. A scroll happens inside a window. Correlated across the whole display the
    /// answer comes back as *nothing moved*, because the menu bar, the dock, the desktop
    /// and every other window did not, and they outvote the one region that did. Measured
    /// while scrolling a text window: best offset zero, residual 4.6, with a fifth of the
    /// screen's samples changed. Both answers were true at once, and the display-wide one
    /// was no use to a mark sitting inside the window.
    ///
    /// So each mark is measured against its own surroundings. A mark in the scrolling pane
    /// sees the scroll. A mark on a toolbar beside it sees nothing and stays put, which is
    /// correct rather than a special case, and is what the display-wide code needed the
    /// fingerprint to patch up after the fact.
    pub fn follow_movement(&self, display: DisplayId, before: &Frame, after: &Frame) -> Followed {
        let Ok(info) = self.display(display) else {
            return Followed::default();
        };

        let watched: Vec<Annotation> = {
            let state = self.state.lock().expect("state lock");
            state
                .annotations
                .values()
                .filter(|a| a.display_id() == display)
                .cloned()
                .collect()
        };

        let mut moving: Vec<Annotation> = Vec::new();
        let mut doomed: Vec<(AnnotationId, SessionId)> = Vec::new();
        for annotation in watched {
            let around = neighbourhood(annotation.anchor.screen_rect, info.logical_size);
            tracing::debug!(id = %annotation.id, ?around, "measuring what moved under a mark");
            let Some(shift) = crate::signature::shift_within(before, after, around) else {
                // Something changed here that no movement explains, so there is nowhere
                // honest to put this mark. The rest of the display is unaffected.
                doomed.push((annotation.id.clone(), annotation.session.clone()));
                continue;
            };
            let mut candidate = annotation.clone();
            candidate.translate(shift);

            let on_screen = candidate.is_on_screen(info.logical_size);
            let recognised = match candidate.fingerprint() {
                Some(recorded) => Fingerprint::of(after, candidate.anchor.screen_rect)
                    .is_none_or(|now| recorded.matches(&now)),
                None => true,
            };

            // Checked even when the measurement says nothing moved, which it did not use
            // to be. A region split between a still part and a scrolling one has two
            // explanations and settles on "no movement", so the mark that most needs
            // catching is precisely the one that reported a zero and skipped the check.
            // A mark sitting on content that is no longer its content has to go whether
            // or not anyone managed to measure where that content went.
            if !recognised {
                tracing::debug!(
                    id = %annotation.id,
                    dx = shift.dx,
                    dy = shift.dy,
                    "the content under a mark is not the content it was put on"
                );
                doomed.push((annotation.id.clone(), annotation.session.clone()));
                continue;
            }
            if shift.is_negligible() {
                continue;
            }

            if on_screen {
                tracing::debug!(
                    id = %annotation.id,
                    dx = shift.dx,
                    dy = shift.dy,
                    "following a mark"
                );
                moving.push(candidate);
            } else {
                tracing::debug!(
                    id = %annotation.id,
                    dx = shift.dx,
                    dy = shift.dy,
                    on_screen,
                    recognised,
                    "measured a movement but could not follow it"
                );
                doomed.push((annotation.id.clone(), annotation.session.clone()));
            }
        }

        if moving.is_empty() && doomed.is_empty() {
            return Followed::default();
        }

        {
            let mut state = self.state.lock().expect("state lock");
            for annotation in &moving {
                // Only if it is still there. A clear or an expiry may have got here first.
                if let Some(slot) = state.annotations.get_mut(&annotation.id) {
                    *slot = annotation.clone();
                }
            }
            for (id, _) in &doomed {
                state.annotations.remove(id);
            }
        }
        self.mark_drawn();

        // Redrawing in place rather than clearing first: a clear would fade the orb out
        // and fly it back in, which reads as the mark leaving and a new one arriving
        // rather than as one mark keeping up with the page.
        for annotation in &moving {
            if let Err(e) = self.renderer.draw(annotation) {
                tracing::warn!(id = %annotation.id, error = %e, "renderer refused a redraw");
            }
        }
        for (id, _) in &doomed {
            if let Err(e) = self.renderer.clear(id) {
                tracing::warn!(%id, error = %e, "renderer refused a clear");
            }
        }
        self.announce(doomed.clone(), &InvalidationReason::Scroll);

        Followed {
            moved: moving.into_iter().map(|a| a.id).collect(),
            invalidated: doomed
                .into_iter()
                .map(|(id, _)| Invalidated::one(id, InvalidationReason::Scroll))
                .collect(),
        }
    }

    /// The time to live a message asked for, or the daemon's default.
    ///
    /// A client's `ttl_ms` wins over the configured default, including when the default
    /// is set and the client wants something shorter. There is deliberately no way to ask
    /// for "no expiry" against a configured default: the default exists so an operator
    /// can guarantee marks do not pile up, and a client opting out would defeat it.
    fn ttl_for(&self, ttl_ms: Option<u64>) -> Option<std::time::Duration> {
        ttl_ms
            .map(std::time::Duration::from_millis)
            .or(self.config.default_ttl)
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
                // Acked rather than answered with an `invalidated`. Every request gets an
                // ack or an error, which leaves `invalidated` to mean one thing only:
                // something happened that the client did not ask for. A reply that shared
                // a type with a push could not be told apart from one.
                Ok(DaemonMessage::Ack(Ack::default()))
            }
        }
    }

    async fn point(&mut self, point: Point) -> Result<DaemonMessage> {
        let session = self.require_session()?;
        let display = self.daemon.display(point.display_id)?;

        let (at, rect, confidence) = match point.target()? {
            PointTarget::Coords(at) => (at, None, None),
            // Resolved here rather than by the client, because the display's size is
            // something only the daemon knows. That is the whole point of the form: a
            // client that has not measured the screen can still say where it means.
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
