//! Scroll detection.
//!
//! Screenshot diffing on a timer, during active sessions only. This is deliberately the
//! dumbest thing that works, because the alternative is asking for Accessibility access
//! to observe scroll events, and that permission is exactly what Arin promises not to
//! need. Screen Recording alone is the whole ask.
//!
//! In 0.1 a detected change invalidates every annotation on that display. Translating
//! annotations with the scroll instead needs the reserved anchor content hash.

use crate::daemon::Daemon;
use crate::signature::Signature;
use arin_protocol::{DisplayId, Invalidated, InvalidationReason};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// How many ticks to treat as a fresh baseline after the daemon draws.
///
/// Two is enough to cover a render dispatched to another thread landing after the tick
/// that noticed it, without blinding the watcher for long enough to miss a real scroll.
const SETTLE_TICKS: u8 = 2;

/// Watches displays for content movement.
pub struct ScrollWatcher {
    daemon: Arc<Daemon>,
    baselines: HashMap<DisplayId, Signature>,
    /// The daemon's render generation as of the last comparison.
    ///
    /// Capture includes the overlay, so a frame taken after the daemon drew differs from
    /// the one before it for reasons that have nothing to do with the user. When this
    /// moves, the next capture becomes a fresh baseline instead of a comparison.
    generation: u64,
    /// Ticks to keep re-baselining after the daemon draws.
    ///
    /// Rendering is dispatched to the platform's own thread, so the pixels change some
    /// time after the daemon records that it drew. A single re-baseline can therefore
    /// land on a half drawn frame and the next comparison blames the user for the rest.
    settling: u8,
    /// Displays already reported as failing to capture.
    ///
    /// The tick runs twice a second for as long as anything is on screen, so a backend
    /// that cannot capture would otherwise fill the log with the same line forever.
    reported: HashSet<DisplayId>,
}

impl ScrollWatcher {
    /// Watch the displays a daemon is drawing on.
    pub fn new(daemon: Arc<Daemon>) -> Self {
        Self {
            daemon,
            baselines: HashMap::new(),
            generation: 0,
            settling: 0,
            reported: HashSet::new(),
        }
    }

    /// Compare one round of captures and invalidate what moved.
    ///
    /// Returns the invalidations to broadcast. Does nothing when nothing is on screen:
    /// there is no reason to capture the display of an idle daemon, and not capturing is
    /// the difference between a background process and a suspicious one.
    pub fn tick(&mut self) -> Vec<Invalidated> {
        if self.daemon.annotation_count() == 0 {
            self.baselines.clear();
            return Vec::new();
        }

        // Anything the daemon drew since the last tick makes the previous frame useless
        // as a baseline. Re-baseline rather than blame the user for it, and keep doing
        // so briefly, because the drawing lands asynchronously.
        let generation = self.daemon.render_generation();
        if generation != self.generation {
            self.generation = generation;
            self.settling = SETTLE_TICKS;
        }
        let rebaseline = self.settling > 0;
        self.settling = self.settling.saturating_sub(1);

        let watched = self.daemon.displays_in_use();
        let displays: Vec<DisplayId> = match self.daemon.displays() {
            Ok(all) => all
                .into_iter()
                .map(|d| d.id)
                .filter(|id| watched.contains(id))
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, "could not enumerate displays");
                return Vec::new();
            }
        };

        let mut invalidated = Vec::new();
        // Named `screen` rather than `display` so the tracing macros below do not
        // shadow their own `display` value helper.
        for id in displays {
            let frame = match self.daemon.capture_backend().capture(id) {
                Ok(frame) => frame,
                Err(e) => {
                    if self.reported.insert(id) {
                        tracing::warn!(%id, error = %e, "capture failed, scroll detection is off");
                    }
                    continue;
                }
            };

            // A capture takes long enough that the daemon can draw part way through
            // one. The generation is checked again here because a frame straddling a
            // draw contains an overlay the baseline does not, and comparing the two
            // blames the user for a change the daemon made.
            let drew_during_capture = self.daemon.render_generation() != generation;

            let current = frame.signature();
            let previous = self.baselines.insert(id, current.clone());
            if rebaseline || drew_during_capture {
                if drew_during_capture {
                    self.settling = SETTLE_TICKS;
                }
                continue;
            }
            match previous {
                // First sighting of this display. Nothing to compare against yet.
                None => {}
                Some(previous) if current.moved_from(&previous) => {
                    tracing::debug!(%id, "content moved, invalidating");
                    invalidated.extend(
                        self.daemon
                            .invalidate_display(id, InvalidationReason::Scroll),
                    );
                }
                Some(_) => {}
            }
        }
        invalidated
    }

    /// Forget what every display looked like.
    ///
    /// Call after deliberately changing what is on screen, so the next tick does not
    /// read the daemon's own drawing as a scroll.
    pub fn reset(&mut self) {
        self.baselines.clear();
        self.reported.clear();
    }
}
