//! Logical geometry.
//!
//! Everything here is in logical points and is meaningless without the [`DisplayId`] it
//! travels with. Retina screenshots come back at 2x while the overlay draws in points,
//! and mixed-DPI multi-monitor setups make implicit conversion unrecoverable. Clients
//! working from a screenshot divide by the scale reported in an ack before sending.

use serde::{Deserialize, Serialize};

/// Identifies a display. Stable for the lifetime of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DisplayId(pub u32);

impl DisplayId {
    /// What a client uses when the user did not name a display.
    ///
    /// Not part of the wire contract, and the daemon never substitutes it: every
    /// positioned message still carries an explicit `display_id`, because a mixed-DPI
    /// setup makes an implicit display unrecoverable. This is only what a client fills in
    /// on the way out, so it lives here rather than being spelled `1` in each of them.
    ///
    /// The id macOS gives the main display, which is `1` in practice rather than by
    /// guarantee. `arin displays` lists what a machine actually has.
    pub const DEFAULT: Self = Self(1);
}

impl std::fmt::Display for DisplayId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A point in logical coordinates, relative to the origin of its display.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LogicalPoint {
    /// Horizontal offset in logical points.
    pub x: f64,
    /// Vertical offset in logical points.
    pub y: f64,
}

impl LogicalPoint {
    /// Construct a point.
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Whether both components are finite.
    pub fn is_finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// A rectangle in logical coordinates.
///
/// Serialized as `[x, y, width, height]`, not as an object.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(from = "[f64; 4]", into = "[f64; 4]")]
pub struct LogicalRect {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Extent along x. Must be positive.
    pub width: f64,
    /// Extent along y. Must be positive.
    pub height: f64,
}

impl LogicalRect {
    /// Construct a rectangle.
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Whether every component is finite and the extents are positive.
    ///
    /// Zero-area rects are rejected: they render as nothing, so they are always a bug in
    /// the caller rather than an intentional annotation.
    pub fn is_valid(&self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width > 0.0
            && self.height > 0.0
    }

    /// The centre of the rectangle.
    pub fn center(&self) -> LogicalPoint {
        LogicalPoint::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    /// This rectangle if it is drawable, `None` if it is not.
    ///
    /// For the callers that treat a region as an optional refinement rather than the
    /// answer. Something derived from a measurement, a model's guess, or arithmetic on
    /// either can come out degenerate, and dropping it is better than propagating a rect
    /// that renders as nothing.
    pub fn into_valid(self) -> Option<Self> {
        self.is_valid().then_some(self)
    }
}

impl From<[f64; 4]> for LogicalRect {
    fn from([x, y, width, height]: [f64; 4]) -> Self {
        Self::new(x, y, width, height)
    }
}

impl From<LogicalRect> for [f64; 4] {
    fn from(r: LogicalRect) -> Self {
        [r.x, r.y, r.width, r.height]
    }
}

/// Display metadata, returned on acks so clients can convert screenshot pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DisplayInfo {
    /// The display this describes.
    pub id: DisplayId,
    /// Backing scale factor. 2.0 on a Retina panel.
    pub scale: f64,
    /// Display size in logical points, as `[width, height]`.
    pub logical_size: [f64; 2],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rects_are_arrays_on_the_wire() {
        let rect = LogicalRect::new(100.0, 200.0, 340.0, 90.0);
        assert_eq!(
            serde_json::to_string(&rect).unwrap(),
            "[100.0,200.0,340.0,90.0]"
        );
        assert_eq!(
            serde_json::from_str::<LogicalRect>("[100,200,340,90]").unwrap(),
            rect
        );
    }

    #[test]
    fn zero_area_rects_are_invalid() {
        assert!(LogicalRect::new(0.0, 0.0, 10.0, 10.0).is_valid());
        assert!(!LogicalRect::new(0.0, 0.0, 0.0, 10.0).is_valid());
        assert!(!LogicalRect::new(0.0, 0.0, 10.0, -1.0).is_valid());
        assert!(!LogicalRect::new(f64::NAN, 0.0, 10.0, 10.0).is_valid());
    }

    #[test]
    fn center_is_the_midpoint() {
        let rect = LogicalRect::new(100.0, 200.0, 340.0, 90.0);
        assert_eq!(rect.center(), LogicalPoint::new(270.0, 245.0));
    }
}
