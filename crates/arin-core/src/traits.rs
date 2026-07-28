//! The seams platform crates plug into.
//!
//! Core depends on these traits and never on a concrete implementation. Binaries wire
//! the real ones up at startup. Tests wire up fakes and run with no display at all.

use crate::annotation::Annotation;
use crate::error::Result;
use crate::policy::OrbState;
use arin_protocol::{AnnotationId, DisplayId, DisplayInfo, LogicalPoint, LogicalRect};
use futures::future::BoxFuture;
use std::sync::Arc;

/// Draws on the screen.
///
/// Implementations are expected to be cheap to call and to do their real work on
/// whatever thread the platform demands. Methods are synchronous because they hand off
/// to a platform queue rather than waiting on one.
pub trait Renderer: Send + Sync + 'static {
    /// Every connected display, with scale and logical size.
    fn displays(&self) -> Result<Vec<DisplayInfo>>;

    /// Draw or redraw an annotation.
    fn draw(&self, annotation: &Annotation) -> Result<()>;

    /// Remove one annotation.
    fn clear(&self, id: &AnnotationId) -> Result<()>;

    /// Remove everything this renderer is showing.
    fn clear_all(&self) -> Result<()>;

    /// Move the orb to a state.
    ///
    /// Clients never request these. They follow from what the daemon is doing.
    fn set_orb_state(&self, state: OrbState) -> Result<()>;
}

/// Takes screenshots.
///
/// Used for scroll detection and to give resolvers something to ground against. Capture
/// is the only permission Arin asks for.
pub trait Capture: Send + Sync + 'static {
    /// Grab the current contents of a display.
    fn capture(&self, display: DisplayId) -> Result<Frame>;
}

/// A captured display.
#[derive(Clone)]
pub struct Frame {
    /// Which display this came from.
    pub display: DisplayId,
    /// Backing scale factor. `width` is `logical_size[0] * scale`.
    pub scale: f64,
    /// Size in logical points.
    pub logical_size: [f64; 2],
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
    /// Packed BGRA pixels, `width * height * 4` bytes.
    pub pixels: Arc<[u8]>,
}

impl Frame {
    /// Summarise the frame for change detection.
    ///
    /// See [`crate::signature`] for why this is a tolerant summary rather than a hash.
    pub fn signature(&self) -> crate::signature::Signature {
        crate::signature::Signature::of(self)
    }
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("display", &self.display)
            .field("scale", &self.scale)
            .field("logical_size", &self.logical_size)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("pixels", &format_args!("{} bytes", self.pixels.len()))
            .finish()
    }
}

/// Turns a natural language target into coordinates.
///
/// Registered on the daemon, never configured per message. Implementations live in
/// `arin-resolve`. Any implementation that reaches the network is an egress point and
/// must be explicit and off by default.
pub trait Resolver: Send + Sync + 'static {
    /// Identifier used in configuration and diagnostics.
    fn name(&self) -> &str;

    /// Whether this resolver sends the frame off the machine.
    ///
    /// Surfaced to the user before anything leaves. A local model returns `false`.
    fn is_remote(&self) -> bool;

    /// Ground `query` against `frame`.
    fn resolve<'a>(&'a self, query: &'a str, frame: &'a Frame)
    -> BoxFuture<'a, Result<Resolution>>;
}

/// What a resolver produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    /// Best guess at the target, in logical points.
    pub point: LogicalPoint,
    /// The region the target occupies, when the model reports one.
    ///
    /// Low-confidence resolutions render as this region rather than as a point.
    pub rect: Option<LogicalRect>,
    /// How sure the model was, in `0.0..=1.0`.
    pub confidence: f64,
}
