//! Renderer and capture backends that do nothing.
//!
//! Used to run the daemon headless: on platforms with no renderer yet, and in tests
//! that exercise the protocol and state machine without a display.

use crate::annotation::Annotation;
use crate::error::Result;
use crate::policy::OrbState;
use crate::traits::{Capture, Frame, Renderer};
use arin_protocol::{AnnotationId, DisplayId, DisplayInfo};
use std::sync::Mutex;

/// A renderer that accepts everything and draws nothing.
#[derive(Debug, Default)]
pub struct NoopRenderer {
    displays: Vec<DisplayInfo>,
    state: Mutex<OrbState>,
}

impl NoopRenderer {
    /// A single 1728x1117 logical display at 2x, matching a 14 inch Mac.
    pub fn new() -> Self {
        Self::with_displays(vec![DisplayInfo {
            id: DisplayId(1),
            scale: 2.0,
            logical_size: [1728.0, 1117.0],
        }])
    }

    /// A renderer reporting a specific set of displays.
    pub fn with_displays(displays: Vec<DisplayInfo>) -> Self {
        Self {
            displays,
            state: Mutex::new(OrbState::Idle),
        }
    }

    /// The last orb state that was set.
    pub fn orb_state(&self) -> OrbState {
        *self.state.lock().expect("orb state lock")
    }
}

impl Renderer for NoopRenderer {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        Ok(self.displays.clone())
    }

    fn draw(&self, annotation: &Annotation) -> Result<()> {
        tracing::debug!(id = %annotation.id, "noop renderer: draw");
        Ok(())
    }

    fn clear(&self, id: &AnnotationId) -> Result<()> {
        tracing::debug!(%id, "noop renderer: clear");
        Ok(())
    }

    fn clear_all(&self) -> Result<()> {
        Ok(())
    }

    fn set_orb_state(&self, state: OrbState) -> Result<()> {
        *self.state.lock().expect("orb state lock") = state;
        Ok(())
    }
}

/// A capture backend that returns blank frames.
///
/// Scroll detection against these never fires, which is the correct behaviour: nothing
/// is being captured, so nothing can be observed to move.
#[derive(Debug, Default)]
pub struct NoopCapture;

impl Capture for NoopCapture {
    fn capture(&self, display: DisplayId) -> Result<Frame> {
        Ok(Frame {
            display,
            scale: 1.0,
            logical_size: [0.0, 0.0],
            width: 0,
            height: 0,
            pixels: Vec::new().into(),
        })
    }
}
