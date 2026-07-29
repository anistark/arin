//! The session and annotation state machine.
//!
//! Transport-free on purpose: [`Connection::handle`] takes a parsed message and returns
//! a reply, so the whole state machine is testable over a `Vec` of messages with no
//! socket and no display.

use crate::annotation::Annotation;
use crate::config::Config;
use crate::contrast::Rgb;
use crate::error::Result;
use crate::fingerprint::Fingerprint;
use crate::session::Session;
use crate::traits::{Capture, Renderer, Resolver};
use arin_protocol::{AnnotationId, DisplayId, Invalidated, InvalidationReason, SessionId};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

/// What the daemon learned from one look at the region it is about to mark.
struct Appearance {
    /// The colour to draw in.
    color: Rgb,
    /// What is under the mark, for checking it later. `None` when nothing was captured.
    fingerprint: Option<Fingerprint>,
}

mod connection;
mod displays;
mod movement;

pub use connection::Connection;
pub use movement::Followed;

/// How many pending announcements a connection may fall behind by.
///
/// Generous: an invalidation is a handful of bytes and a connection that is not reading
/// is already in trouble. A receiver that overruns this loses the oldest and is told, so
/// the daemon is never blocked by a client that stopped listening.
const ANNOUNCEMENT_BACKLOG: usize = 256;

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

    /// The capture backend, for the scroll watcher.
    pub fn capture_backend(&self) -> &Arc<dyn Capture> {
        &self.capture
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
