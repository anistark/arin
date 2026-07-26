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
use arin_protocol::{DisplayId, Invalidated, InvalidationReason};
use std::collections::HashMap;
use std::sync::Arc;

/// Watches displays for content movement.
pub struct ScrollWatcher {
    daemon: Arc<Daemon>,
    fingerprints: HashMap<DisplayId, u64>,
}

impl ScrollWatcher {
    /// Watch the displays a daemon is drawing on.
    pub fn new(daemon: Arc<Daemon>) -> Self {
        Self {
            daemon,
            fingerprints: HashMap::new(),
        }
    }

    /// Compare one round of captures and invalidate what moved.
    ///
    /// Returns the invalidations to broadcast. Does nothing when nothing is on screen:
    /// there is no reason to capture the display of an idle daemon, and not capturing is
    /// the difference between a background process and a suspicious one.
    pub fn tick(&mut self) -> Vec<Invalidated> {
        if self.daemon.annotation_count() == 0 {
            self.fingerprints.clear();
            return Vec::new();
        }

        let displays = match self.daemon.displays() {
            Ok(displays) => displays,
            Err(e) => {
                tracing::warn!(error = %e, "could not enumerate displays");
                return Vec::new();
            }
        };

        let mut invalidated = Vec::new();
        // Named `screen` rather than `display` so the tracing macros below do not
        // shadow their own `display` value helper.
        for screen in displays {
            let id = screen.id;
            let frame = match self.daemon.capture_backend().capture(id) {
                Ok(frame) => frame,
                Err(e) => {
                    tracing::warn!(%id, error = %e, "capture failed");
                    continue;
                }
            };

            let current = frame.fingerprint();
            match self.fingerprints.insert(id, current) {
                // First sighting of this display. Nothing to compare against yet.
                None => {}
                Some(previous) if previous != current => {
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
        self.fingerprints.clear();
    }
}
