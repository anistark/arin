//! Client to daemon messages.

use crate::anchor::Anchor;
use crate::geom::{DisplayId, LogicalPoint, LogicalRect};
use crate::ids::AnnotationId;
use crate::position::Position;
use crate::validate::{Validate, ValidationError};
use serde::{Deserialize, Serialize};

fn is_false(b: &bool) -> bool {
    !*b
}

/// Check a time to live, which every drawing message carries in the same form.
///
/// Milliseconds rather than seconds so the wire never has to carry a fraction, and an
/// integer so it never has to carry a NaN. Absent means the mark lives until it is
/// cleared or invalidated, which is what the daemon does by default.
fn validate_ttl(ttl_ms: Option<u64>) -> Result<(), ValidationError> {
    match ttl_ms {
        Some(0) => Err(ValidationError::ZeroTtl),
        _ => Ok(()),
    }
}

/// Anything a client can send.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Open a session. The daemon replies with a session id.
    SessionStart(SessionStart),
    /// Put the orb on a target.
    Point(Point),
    /// Outline a region.
    Highlight(Highlight),
    /// Place a text box. Display only, never an input widget.
    Textbox(Textbox),
    /// Draw a freehand path.
    Draw(Draw),
    /// Draw an arrow from one position to another.
    Arrow(Arrow),
    /// Remove annotations owned by this session.
    Clear(Clear),
    /// Bring an application's windows to the front.
    Focus(Focus),
    /// Wait until an application's window is showing.
    AwaitWindow(AwaitWindow),
    /// Close the session. Annotations fade shortly after.
    SessionEnd,
}

impl Validate for ClientMessage {
    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::SessionStart(m) => m.validate(),
            Self::Point(m) => m.validate(),
            Self::Highlight(m) => m.validate(),
            Self::Textbox(m) => m.validate(),
            Self::Draw(m) => m.validate(),
            Self::Arrow(m) => m.validate(),
            Self::Clear(m) => m.validate(),
            Self::Focus(m) => m.validate(),
            Self::AwaitWindow(m) => m.validate(),
            Self::SessionEnd => Ok(()),
        }
    }
}

/// Bring an application's windows to the front.
///
/// The one message that changes what is on the user's screen rather than what is drawn over
/// it, and the only one a daemon may refuse for being switched off rather than malformed.
///
/// Arin can only mark the visible desktop, since every desktop on a display shares one set
/// of coordinates, so a target the user has swiped away from has nowhere to be marked. This
/// is the ask to switch, carried out.
///
/// It names an application rather than a window: a window has no name a client could know,
/// and choosing between an application's windows needs Accessibility.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Focus {
    /// The application to bring forward, by visible name or bundle identifier.
    ///
    /// `"Slack"` and `"com.tinyspeck.slackmacgap"` both work. Matched against the
    /// applications that own windows the capture backend can see, so this never reaches an
    /// application the user does not have open, and never reads a window title to decide.
    pub app: String,
}

impl Validate for Focus {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.app.trim().is_empty() {
            return Err(ValidationError::Empty { field: "app" });
        }
        Ok(())
    }
}

/// Wait until an application's window is showing, or give up.
///
/// The other half of guiding somebody somewhere Arin cannot take them. A mark can say
/// "switch desktops", and without this nothing knows whether they did, so an agent draws an
/// instruction and carries on as though it were followed.
///
/// A request rather than a pushed event because MCP has no way for a server to interrupt a
/// model: a client driven by one could not act on an event until its next call anyway.
///
/// It never moves anything. The acting half of the handoff is the user's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AwaitWindow {
    /// The application to wait for, matched exactly as [`Focus::app`] is.
    pub app: String,
    /// How long to wait before giving up, in milliseconds.
    ///
    /// Absent leaves it to the daemon, which is what a client that has no opinion should
    /// send: the useful bound is "about as long as a person takes to find a window", and the
    /// daemon is better placed to know that than a client is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

impl Validate for AwaitWindow {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.app.trim().is_empty() {
            return Err(ValidationError::Empty { field: "app" });
        }
        // A wait of no time is a question, not a wait, and answering it as though somebody
        // had waited would report "they never arrived" the instant they were asked to move.
        if self.timeout_ms == Some(0) {
            return Err(ValidationError::ZeroTtl);
        }
        Ok(())
    }
}

/// Open a session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionStart {
    /// Free-form client identifier, for example `"claude-code"`. Shown in diagnostics.
    pub client_name: String,
}

impl Validate for SessionStart {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.client_name.trim().is_empty() {
            return Err(ValidationError::Empty {
                field: "client_name",
            });
        }
        Ok(())
    }
}

/// Put the orb on a target.
///
/// Two forms. Clients that ground coordinates themselves send `x` and `y`. Clients that
/// cannot send `query` instead, which needs a configured resolver.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Point {
    /// Horizontal position in logical points. Paired with `y`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    /// Vertical position in logical points. Paired with `x`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    /// A position named relative to the display, such as `top-left` or `50%,30%`.
    ///
    /// For clients that have not measured the screen and so cannot name a coordinate.
    /// Resolved by the daemon, which is the only party that knows the geometry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// Natural language description of the target, resolved by the daemon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// The display the coordinates, position, or query apply to.
    pub display_id: DisplayId,
    /// Short caption rendered next to the orb.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// How long the mark should live, in milliseconds.
    ///
    /// Omit to draw until cleared, invalidated, or the session ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
}

/// Which form of [`Point`] a client sent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointTarget<'a> {
    /// The client did its own grounding.
    Coords(LogicalPoint),
    /// A position relative to the display, which only the daemon can turn into a point.
    Named(Position),
    /// The daemon must resolve this.
    Query(&'a str),
}

impl Point {
    /// At explicit coordinates.
    pub const fn at(x: f64, y: f64, display_id: DisplayId) -> Self {
        Self {
            x: Some(x),
            y: Some(y),
            at: None,
            query: None,
            display_id,
            label: None,
            ttl_ms: None,
        }
    }

    /// From a natural language description. Requires a resolver.
    pub fn query(query: impl Into<String>, display_id: DisplayId) -> Self {
        Self {
            x: None,
            y: None,
            at: None,
            query: Some(query.into()),
            display_id,
            label: None,
            ttl_ms: None,
        }
    }

    /// At a position named relative to the display, such as `top-left` or `50%,30%`.
    pub fn named(at: impl Into<String>, display_id: DisplayId) -> Self {
        Self {
            x: None,
            y: None,
            at: Some(at.into()),
            query: None,
            display_id,
            label: None,
            ttl_ms: None,
        }
    }

    /// Attach a caption.
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set how long the mark lives, in milliseconds.
    #[must_use]
    pub fn with_ttl_ms(mut self, ttl_ms: Option<u64>) -> Self {
        self.ttl_ms = ttl_ms;
        self
    }

    /// Which form was sent.
    ///
    /// Exactly one of the three. Half a coordinate pair, or two forms at once, is an
    /// error rather than a guess: every one of those is a client bug, and drawing
    /// somewhere plausible would hide it.
    pub fn target(&self) -> Result<PointTarget<'_>, ValidationError> {
        const EXPECTED: &str = "x and y, or at, or query";

        let coords = self.x.is_some() || self.y.is_some();
        let named = self.at.is_some();
        let queried = self.query.is_some();

        match (coords, named, queried) {
            (false, false, false) => {
                return Err(ValidationError::MissingTarget {
                    message: "point",
                    expected: EXPECTED,
                });
            }
            // More than one form supplied.
            (true, true, _) | (true, _, true) | (_, true, true) => {
                return Err(ValidationError::AmbiguousTarget {
                    message: "point",
                    expected: EXPECTED,
                });
            }
            _ => {}
        }

        if let Some(at) = self.at.as_deref() {
            return Position::parse(at).map(PointTarget::Named);
        }

        if let Some(query) = self.query.as_deref() {
            return if query.trim().is_empty() {
                Err(ValidationError::Empty { field: "query" })
            } else {
                Ok(PointTarget::Query(query))
            };
        }

        match (self.x, self.y) {
            (Some(x), Some(y)) => {
                let p = LogicalPoint::new(x, y);
                if !p.is_finite() {
                    return Err(ValidationError::NonFiniteCoordinate { field: "x, y" });
                }
                Ok(PointTarget::Coords(p))
            }
            // Half a pair. Guessing the other half would put the mark somewhere the
            // client never asked for.
            _ => Err(ValidationError::AmbiguousTarget {
                message: "point",
                expected: EXPECTED,
            }),
        }
    }
}

impl Validate for Point {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_ttl(self.ttl_ms)?;
        self.target().map(|_| ())
    }
}

/// Outline a region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Highlight {
    /// The region in logical points, as `[x, y, width, height]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rect: Option<LogicalRect>,
    /// Natural language description of the region, resolved by the daemon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// The display the rect or query applies to.
    pub display_id: DisplayId,
    /// Short caption rendered against the region.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// How long the mark should live, in milliseconds.
    ///
    /// Omit to draw until cleared, invalidated, or the session ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
}

/// Which form of [`Highlight`] a client sent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HighlightTarget<'a> {
    /// The client did its own grounding.
    Rect(LogicalRect),
    /// The daemon must resolve this.
    Query(&'a str),
}

impl Highlight {
    /// Over an explicit rect.
    pub const fn over(rect: LogicalRect, display_id: DisplayId) -> Self {
        Self {
            rect: Some(rect),
            query: None,
            display_id,
            label: None,
            ttl_ms: None,
        }
    }

    /// From a natural language description. Requires a resolver.
    pub fn query(query: impl Into<String>, display_id: DisplayId) -> Self {
        Self {
            rect: None,
            query: Some(query.into()),
            display_id,
            label: None,
            ttl_ms: None,
        }
    }

    /// Attach a caption.
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set how long the mark lives, in milliseconds.
    #[must_use]
    pub fn with_ttl_ms(mut self, ttl_ms: Option<u64>) -> Self {
        self.ttl_ms = ttl_ms;
        self
    }

    /// Which form was sent.
    pub fn target(&self) -> Result<HighlightTarget<'_>, ValidationError> {
        const EXPECTED: &str = "rect or query";
        match (self.rect, self.query.as_deref()) {
            (Some(rect), None) => {
                if !rect.is_valid() {
                    return Err(ValidationError::InvalidRect { field: "rect" });
                }
                Ok(HighlightTarget::Rect(rect))
            }
            (None, Some(q)) if !q.trim().is_empty() => Ok(HighlightTarget::Query(q)),
            (None, Some(_)) => Err(ValidationError::Empty { field: "query" }),
            (None, None) => Err(ValidationError::MissingTarget {
                message: "highlight",
                expected: EXPECTED,
            }),
            (Some(_), Some(_)) => Err(ValidationError::AmbiguousTarget {
                message: "highlight",
                expected: EXPECTED,
            }),
        }
    }
}

impl Validate for Highlight {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_ttl(self.ttl_ms)?;
        self.target().map(|_| ())
    }
}

/// Place a text box.
///
/// Display only. Arin never renders an editable input, and the overlay is fully click
/// through. This is not negotiable within 0.x.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Textbox {
    /// Where to pin the box. Preferred form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Anchor>,
    /// Shorthand for an anchor, used with `display_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rect: Option<LogicalRect>,
    /// Required when `rect` is used instead of `anchor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_id: Option<DisplayId>,
    /// The text to render.
    pub text: String,
    /// What the text is for, which decides how loudly it is drawn.
    ///
    /// Omit for [`TextboxStyle::Note`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<TextboxStyle>,
    /// How long the box should live, in milliseconds.
    ///
    /// Omit to draw until cleared, invalidated, or the session ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
}

/// What a text box is for.
///
/// Semantic rather than typographic on purpose. A client says what kind of thing it is
/// writing, and the renderer decides what that looks like, the same split that keeps
/// colour out of clients' hands. Raw font sizes on the wire would make every client a
/// typographer and every renderer a chance to drift.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextboxStyle {
    /// An explanation read alongside something else. Quiet, the default.
    #[default]
    Note,
    /// An instruction read at a glance by somebody about to act. Larger and centred,
    /// because it competes with everything else on a screen mid-action.
    Guide,
}

impl Textbox {
    /// Pin text to a region.
    pub fn new(anchor: Anchor, text: impl Into<String>) -> Self {
        Self {
            anchor: Some(anchor),
            rect: None,
            display_id: None,
            text: text.into(),
            style: None,
            ttl_ms: None,
        }
    }

    /// Say what the text is for.
    #[must_use]
    pub fn with_style(mut self, style: TextboxStyle) -> Self {
        self.style = Some(style);
        self
    }

    /// Set how long the box lives, in milliseconds.
    #[must_use]
    pub fn with_ttl_ms(mut self, ttl_ms: Option<u64>) -> Self {
        self.ttl_ms = ttl_ms;
        self
    }

    /// The anchor, whichever form was sent.
    pub fn resolved_anchor(&self) -> Result<Anchor, ValidationError> {
        const EXPECTED: &str = "anchor, or rect with display_id";
        match (&self.anchor, self.rect, self.display_id) {
            (Some(anchor), None, None) => {
                if !anchor.screen_rect.is_valid() {
                    return Err(ValidationError::InvalidRect {
                        field: "anchor.screen_rect",
                    });
                }
                Ok(anchor.clone())
            }
            (None, Some(rect), Some(display_id)) => {
                if !rect.is_valid() {
                    return Err(ValidationError::InvalidRect { field: "rect" });
                }
                Ok(Anchor::new(rect, display_id))
            }
            (None, None, _) => Err(ValidationError::MissingTarget {
                message: "textbox",
                expected: EXPECTED,
            }),
            _ => Err(ValidationError::AmbiguousTarget {
                message: "textbox",
                expected: EXPECTED,
            }),
        }
    }
}

impl Validate for Textbox {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_ttl(self.ttl_ms)?;
        if self.text.trim().is_empty() {
            return Err(ValidationError::Empty { field: "text" });
        }
        self.resolved_anchor().map(|_| ())
    }
}

/// Draw a freehand path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Draw {
    /// The display the path is drawn on.
    pub display_id: DisplayId,
    /// Ordered vertices in logical points, each as `[x, y]`.
    pub path: Vec<[f64; 2]>,
    /// Stroke appearance. Daemon defaults apply when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<StrokeStyle>,
    /// How long the path should live, in milliseconds.
    ///
    /// Omit to draw until cleared, invalidated, or the session ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
}

impl Draw {
    /// A path on a display.
    pub fn new(display_id: DisplayId, path: Vec<[f64; 2]>) -> Self {
        Self {
            display_id,
            path,
            style: None,
            ttl_ms: None,
        }
    }

    /// Set how long the path lives, in milliseconds.
    #[must_use]
    pub fn with_ttl_ms(mut self, ttl_ms: Option<u64>) -> Self {
        self.ttl_ms = ttl_ms;
        self
    }

    /// The path as logical points.
    pub fn points(&self) -> impl Iterator<Item = LogicalPoint> + '_ {
        self.path.iter().map(|&[x, y]| LogicalPoint::new(x, y))
    }
}

impl Validate for Draw {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_ttl(self.ttl_ms)?;
        if self.path.len() < 2 {
            return Err(ValidationError::PathTooShort {
                got: self.path.len(),
            });
        }
        if !self.points().all(|p| p.is_finite()) {
            return Err(ValidationError::NonFiniteCoordinate { field: "path" });
        }
        Ok(())
    }
}

/// Stroke appearance for a freehand path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrokeStyle {
    /// Stroke width in logical points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    /// Stroke colour as `#RRGGBB`.
    ///
    /// Omit to let the contrast picker choose against the target region. Blue is
    /// reserved for the orb and is excluded from automatic selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Draw an arrow from one position to another. The head sits at `to`.
///
/// Curved by default. A gentle bow is how a person draws an arrow, and it keeps the
/// shaft out of whatever sits on the straight line between two points, which is often
/// exactly the content being talked about. `bow: 0` asks for a straight one.
///
/// The daemon turns the curve into an ordinary path annotation, so an arrow scrolls,
/// expires, and picks its colour exactly as a freehand path does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Arrow {
    /// The display both ends sit on.
    pub display_id: DisplayId,
    /// Where the arrow starts.
    pub from: ArrowEnd,
    /// Where the arrow points.
    pub to: ArrowEnd,
    /// How far the shaft bows off the straight line, as a signed fraction of its length.
    ///
    /// `0` is straight, positive bows to the right of travel, negative to the left, and
    /// past `1` the shape stops reading as an arrow, so that is where validation stops
    /// accepting it. Omit for the daemon's natural curve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bow: Option<f64>,
    /// Stroke appearance. Daemon defaults apply when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<StrokeStyle>,
    /// How long the arrow should live, in milliseconds.
    ///
    /// Omit to draw until cleared, invalidated, or the session ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
}

/// One end of an arrow.
///
/// Either form [`Point`] takes, minus the query: coordinates for a client that measured
/// the screen, or a named position for one that has not. The forms are different JSON
/// shapes, a pair against a string, so an end can never be two things at once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    untagged,
    expecting = "an [x, y] pair, or a position like `top-left` or `50%,30%`"
)]
pub enum ArrowEnd {
    /// Logical coordinates, as `[x, y]`.
    Coords([f64; 2]),
    /// A position named relative to the display, such as `top-left` or `50%,30%`.
    Named(String),
}

/// An [`ArrowEnd`], checked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ArrowTarget {
    /// The client did its own grounding.
    Coords(LogicalPoint),
    /// A position relative to the display, which only the daemon can turn into a point.
    Named(Position),
}

impl ArrowEnd {
    /// Coordinates in logical points.
    pub const fn coords(x: f64, y: f64) -> Self {
        Self::Coords([x, y])
    }

    /// A position named relative to the display.
    pub fn named(at: impl Into<String>) -> Self {
        Self::Named(at.into())
    }

    /// Which form this end carries.
    pub fn target(&self) -> Result<ArrowTarget, ValidationError> {
        match self {
            Self::Coords([x, y]) => {
                let p = LogicalPoint::new(*x, *y);
                if !p.is_finite() {
                    return Err(ValidationError::NonFiniteCoordinate { field: "from, to" });
                }
                Ok(ArrowTarget::Coords(p))
            }
            Self::Named(at) => Position::parse(at).map(ArrowTarget::Named),
        }
    }
}

impl Arrow {
    /// From one end to the other.
    pub fn new(display_id: DisplayId, from: ArrowEnd, to: ArrowEnd) -> Self {
        Self {
            display_id,
            from,
            to,
            bow: None,
            style: None,
            ttl_ms: None,
        }
    }

    /// Set how far the shaft bows off the straight line.
    #[must_use]
    pub fn with_bow(mut self, bow: f64) -> Self {
        self.bow = Some(bow);
        self
    }

    /// Set how long the arrow lives, in milliseconds.
    #[must_use]
    pub fn with_ttl_ms(mut self, ttl_ms: Option<u64>) -> Self {
        self.ttl_ms = ttl_ms;
        self
    }
}

impl Validate for Arrow {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_ttl(self.ttl_ms)?;
        let from = self.from.target()?;
        let to = self.to.target()?;
        // Two identical ends can only be caught here when they are literal. Named ends
        // resolve against a display this crate has never seen, so the daemon repeats the
        // check after resolving them.
        if from == to {
            return Err(ValidationError::ZeroLengthArrow);
        }
        if let Some(bow) = self.bow {
            if !bow.is_finite() || bow.abs() > 1.0 {
                return Err(ValidationError::BowOutOfRange {
                    got: bow.to_string(),
                });
            }
        }
        Ok(())
    }
}

/// Remove annotations. Only ever affects the calling session's own annotations.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Clear {
    /// Clear one annotation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotation_id: Option<AnnotationId>,
    /// Clear every annotation in this session.
    #[serde(default, skip_serializing_if = "is_false")]
    pub all: bool,
}

impl Clear {
    /// Clear every annotation in the session.
    pub const fn all() -> Self {
        Self {
            annotation_id: None,
            all: true,
        }
    }

    /// Clear one annotation.
    pub const fn one(annotation_id: AnnotationId) -> Self {
        Self {
            annotation_id: Some(annotation_id),
            all: false,
        }
    }
}

impl Validate for Clear {
    fn validate(&self) -> Result<(), ValidationError> {
        const EXPECTED: &str = "annotation_id or all";
        match (&self.annotation_id, self.all) {
            (Some(_), false) | (None, true) => Ok(()),
            (None, false) => Err(ValidationError::MissingTarget {
                message: "clear",
                expected: EXPECTED,
            }),
            (Some(_), true) => Err(ValidationError::AmbiguousTarget {
                message: "clear",
                expected: EXPECTED,
            }),
        }
    }
}
