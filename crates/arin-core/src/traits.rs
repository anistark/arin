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

    /// Grab a display with at least this many pixels along its longest edge.
    ///
    /// Two callers want very different things from a capture. Scroll detection and the
    /// contrast picker are reading coarse statistics and are happy with a thumbnail, which
    /// is why the daemon's backend is configured to downscale. A resolver grounding a
    /// query has to read the interface, and a mark placed from a 512 pixel thumbnail is
    /// off by however much that thumbnail rounded.
    ///
    /// A backend that cannot vary its resolution may ignore the request. Callers check the
    /// dimensions of what came back rather than assuming they got what they asked for, and
    /// nothing here promises more than the display itself has.
    fn capture_detailed(&self, display: DisplayId, min_long_edge: u32) -> Result<Frame> {
        let _ = min_long_edge;
        self.capture(display)
    }
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

    /// How much detail this resolver wants, in pixels along the longest edge.
    ///
    /// The daemon captures, so the resolver has to say what it needs rather than take
    /// what it is given. Larger is more accurate and more expensive, in tokens for a
    /// hosted model and in time for any of them.
    fn detail(&self) -> u32 {
        DEFAULT_DETAIL
    }

    /// Ground `query` against `frame`.
    fn resolve<'a>(&'a self, query: &'a str, frame: &'a Frame)
    -> BoxFuture<'a, Result<Resolution>>;
}

/// Pixels on the long edge a resolver gets when it does not ask for a number.
///
/// Roughly 1080p, which is enough to read interface text without paying for a Retina
/// panel's full pixel count on every resolve.
pub const DEFAULT_DETAIL: u32 = 1920;

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

/// Perceived brightness of one BGRA pixel.
///
/// Integer weights rather than floats: this runs over every pixel of every capture, and
/// the answer only has to be consistent, not colorimetrically exact.
///
/// Not the same thing as [`crate::contrast`]'s luminance, which is the gamma corrected
/// sRGB relative luminance the WCAG contrast ratio is defined against. That one models
/// what a person sees well enough to pick a readable colour. This one is a cheap, stable
/// number for telling two captures apart, and the two must not be conflated.
pub fn luminance(bgra: &[u8]) -> u8 {
    let b = u32::from(bgra[0]);
    let g = u32::from(bgra[1]);
    let r = u32::from(bgra[2]);
    ((r * 77 + g * 150 + b * 29) >> 8) as u8
}
