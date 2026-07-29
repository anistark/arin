//! Everything the daemon knows about which screens exist, and what to do when that changes.

use super::Daemon;
use crate::annotation::Annotation;
use crate::error::{Error, Result};
use arin_protocol::{
    AnnotationId, DisplayId, DisplayInfo, Invalidated, InvalidationReason, SessionId,
};

impl Daemon {
    /// Every connected display.
    pub fn displays(&self) -> Result<Vec<DisplayInfo>> {
        self.renderer.displays()
    }

    /// Look up a display, or fail with the code the client expects.
    pub(super) fn display(&self, id: DisplayId) -> Result<DisplayInfo> {
        self.displays()?
            .into_iter()
            .find(|d| d.id == id)
            .ok_or(Error::UnknownDisplay(id))
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
    /// changes, and the arrangement a mark was placed against is not the one on the desk.
    /// Two things follow:
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
}
