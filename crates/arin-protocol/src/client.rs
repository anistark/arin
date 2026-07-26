//! Client to daemon messages.

use crate::anchor::Anchor;
use crate::geom::{DisplayId, LogicalPoint, LogicalRect};
use crate::ids::AnnotationId;
use crate::validate::{Validate, ValidationError};
use serde::{Deserialize, Serialize};

fn is_false(b: &bool) -> bool {
    !*b
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
    /// Remove annotations owned by this session.
    Clear(Clear),
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
            Self::Clear(m) => m.validate(),
            Self::SessionEnd => Ok(()),
        }
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
    /// Natural language description of the target, resolved by the daemon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// The display the coordinates or query apply to.
    pub display_id: DisplayId,
    /// Short caption rendered next to the orb.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Which form of [`Point`] a client sent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointTarget<'a> {
    /// The client did its own grounding.
    Coords(LogicalPoint),
    /// The daemon must resolve this.
    Query(&'a str),
}

impl Point {
    /// At explicit coordinates.
    pub const fn at(x: f64, y: f64, display_id: DisplayId) -> Self {
        Self {
            x: Some(x),
            y: Some(y),
            query: None,
            display_id,
            label: None,
        }
    }

    /// From a natural language description. Requires a resolver.
    pub fn query(query: impl Into<String>, display_id: DisplayId) -> Self {
        Self {
            x: None,
            y: None,
            query: Some(query.into()),
            display_id,
            label: None,
        }
    }

    /// Attach a caption.
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Which form was sent.
    pub fn target(&self) -> Result<PointTarget<'_>, ValidationError> {
        const EXPECTED: &str = "x and y, or query";
        match (self.x, self.y, self.query.as_deref()) {
            (Some(x), Some(y), None) => {
                let p = LogicalPoint::new(x, y);
                if !p.is_finite() {
                    return Err(ValidationError::NonFiniteCoordinate { field: "x, y" });
                }
                Ok(PointTarget::Coords(p))
            }
            (None, None, Some(q)) if !q.trim().is_empty() => Ok(PointTarget::Query(q)),
            (None, None, Some(_)) => Err(ValidationError::Empty { field: "query" }),
            (None, None, None) => Err(ValidationError::MissingTarget {
                message: "point",
                expected: EXPECTED,
            }),
            _ => Err(ValidationError::AmbiguousTarget {
                message: "point",
                expected: EXPECTED,
            }),
        }
    }
}

impl Validate for Point {
    fn validate(&self) -> Result<(), ValidationError> {
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
        }
    }

    /// From a natural language description. Requires a resolver.
    pub fn query(query: impl Into<String>, display_id: DisplayId) -> Self {
        Self {
            rect: None,
            query: Some(query.into()),
            display_id,
            label: None,
        }
    }

    /// Attach a caption.
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
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
}

impl Textbox {
    /// Pin text to a region.
    pub fn new(anchor: Anchor, text: impl Into<String>) -> Self {
        Self {
            anchor: Some(anchor),
            rect: None,
            display_id: None,
            text: text.into(),
        }
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
}

impl Draw {
    /// A path on a display.
    pub fn new(display_id: DisplayId, path: Vec<[f64; 2]>) -> Self {
        Self {
            display_id,
            path,
            style: None,
        }
    }

    /// The path as logical points.
    pub fn points(&self) -> impl Iterator<Item = LogicalPoint> + '_ {
        self.path.iter().map(|&[x, y]| LogicalPoint::new(x, y))
    }
}

impl Validate for Draw {
    fn validate(&self) -> Result<(), ValidationError> {
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
