//! The marker: the person at the screen drawing on the overlay themselves.
//!
//! Everything else on the overlay is drawn by an agent. This is the one thing on it a
//! person draws, by switching the marker on from the menu bar and dragging across the
//! screen. While it is on, the overlay stops being click through, takes the mouse, and
//! puts ink where the pointer goes. None of that is input synthesis: the panel receives
//! clicks, and receiving one is not posting one.
//!
//! # What a stroke is not
//!
//! Not an annotation. A stroke has no session, no anchor, and no place in the daemon's
//! state, so nothing on the wire can move it, expire it, or clear it, and the scroll
//! watcher leaves it where it was drawn. It is a mark on the glass rather than on the
//! content, which is what somebody drawing over a screen expects. It is cleared by the
//! person who drew it: a right click while the marker is on, or the same Clear that takes
//! the agent's marks. Switching the marker off leaves the strokes up, so a person can
//! draw, then go and click the thing they drew around.

use crate::bitmap;
use crate::host;
use arin_core::Rgb;
use arin_protocol::DisplayId;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::NSCursor;
use objc2_core_foundation::{CFRetained, CGPoint};
use objc2_core_graphics::CGMutablePath;
use objc2_foundation::NSPoint;
use objc2_quartz_core::{CALayer, CAShapeLayer};

/// Stroke width in logical points.
///
/// Wider than an agent's path, which is drawn at the width the contrast picker samples
/// under. A marker is held in a hand and should read as one.
pub(crate) const WIDTH: f64 = 4.0;

/// What the mouse did, in the panel's own coordinates.
pub(crate) enum Input {
    /// The button went down: a stroke starts here.
    Begin(CGPoint),
    /// The pointer moved with the button held.
    Extend(CGPoint),
    /// The button came up.
    End,
    /// A right click, or a two finger click on a trackpad: wipe every stroke.
    Clear,
}

/// One stroke, from button down to button up.
struct Stroke {
    display: DisplayId,
    points: Vec<CGPoint>,
    layer: Retained<CAShapeLayer>,
}

/// The marker's state: whether it is on, what it draws in, and what it has drawn.
///
/// Lives on the main thread with the panels, because every stroke is a layer.
pub(crate) struct Marker {
    on: bool,
    color: Rgb,
    /// Built once per colour, on first use.
    cursor: Option<Retained<NSCursor>>,
    /// Finished strokes, and the display each is on.
    strokes: Vec<(DisplayId, Retained<CAShapeLayer>)>,
    /// The stroke under the button, while it is held.
    current: Option<Stroke>,
}

impl Marker {
    pub(crate) fn new() -> Self {
        Self {
            on: false,
            color: arin_core::contrast::DEFAULT,
            cursor: None,
            strokes: Vec::new(),
            current: None,
        }
    }

    pub(crate) fn is_on(&self) -> bool {
        self.on
    }

    /// Switch on or off. Off lets go of a stroke still under the button.
    pub(crate) fn set_on(&mut self, on: bool) {
        self.on = on;
        if !on {
            self.end();
        }
    }

    /// Draw in a colour from now on. Strokes already drawn keep theirs.
    pub(crate) fn set_color(&mut self, color: Rgb) {
        if self.color != color {
            self.color = color;
            self.cursor = None;
        }
    }

    /// The pointer while the marker is on: a tip in the ink's colour.
    pub(crate) fn cursor(&mut self) -> Option<Retained<NSCursor>> {
        if self.cursor.is_none() {
            self.cursor = cursor(self.color);
        }
        self.cursor.clone()
    }

    /// Start a stroke on a display, in the panel's layer tree.
    pub(crate) fn begin(&mut self, display: DisplayId, root: &CALayer, at: CGPoint) {
        // A second button down with the first still held is a lost button up.
        self.end();
        let layer = host::stroke_layer(self.color, WIDTH);
        layer.setPath(Some(&path(&[at])));
        root.addSublayer(&layer);
        self.current = Some(Stroke {
            display,
            points: vec![at],
            layer,
        });
    }

    /// Carry the stroke under the button to where the pointer is now.
    pub(crate) fn extend(&mut self, at: CGPoint) {
        let Some(stroke) = self.current.as_mut() else {
            return;
        };
        stroke.points.push(at);
        // A fresh path each time rather than the old one mutated: a shape layer handed the
        // same path object it already holds sees no change and draws nothing new.
        stroke.layer.setPath(Some(&path(&stroke.points)));
    }

    /// Finish the stroke under the button, if there is one.
    pub(crate) fn end(&mut self) {
        if let Some(stroke) = self.current.take() {
            self.strokes.push((stroke.display, stroke.layer));
        }
    }

    /// Take every stroke off every display. Returns how many there were.
    pub(crate) fn clear(&mut self) -> usize {
        self.end();
        let count = self.strokes.len();
        for (_, layer) in self.strokes.drain(..) {
            layer.removeFromSuperlayer();
        }
        count
    }

    /// Forget the strokes on a display whose panel is gone. They went with it.
    pub(crate) fn forget_display(&mut self, display: DisplayId) {
        if self.current.as_ref().is_some_and(|s| s.display == display) {
            self.current = None;
        }
        self.strokes.retain(|(on, _)| *on != display);
    }
}

/// One piece of a stroke's outline.
#[derive(Debug, Clone, Copy)]
enum Segment {
    Move(CGPoint),
    Line(CGPoint),
    /// A quadratic curve, bent towards `control`.
    Curve {
        control: CGPoint,
        to: CGPoint,
    },
}

/// The outline of a stroke through `points`, smoothed.
///
/// Mouse events arrive as a polyline, and a polyline drawn straight shows a corner at
/// every event. So each inner vertex becomes the control point of a curve running
/// between the midpoints either side of it, which passes close to every sample and bends
/// smoothly through all of them. The ends are straight pieces, so the ink starts and
/// stops exactly where the pointer did. A single point is a zero length line, which
/// round caps draw as a dot the width of the stroke: a click leaves a mark.
fn segments(points: &[CGPoint]) -> Vec<Segment> {
    let mut out = Vec::with_capacity(points.len() + 2);
    let Some((&first, rest)) = points.split_first() else {
        return out;
    };
    out.push(Segment::Move(first));
    match rest {
        [] => out.push(Segment::Line(first)),
        [only] => out.push(Segment::Line(*only)),
        _ => {
            out.push(Segment::Line(midpoint(points[0], points[1])));
            for window in points.windows(3) {
                out.push(Segment::Curve {
                    control: window[1],
                    to: midpoint(window[1], window[2]),
                });
            }
            out.push(Segment::Line(points[points.len() - 1]));
        }
    }
    out
}

fn midpoint(a: CGPoint, b: CGPoint) -> CGPoint {
    CGPoint::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0)
}

/// The Core Graphics path for a stroke through `points`.
fn path(points: &[CGPoint]) -> CFRetained<CGMutablePath> {
    let path = CGMutablePath::new();
    for segment in segments(points) {
        // SAFETY: the path is live, and a null transform means none.
        unsafe {
            match segment {
                Segment::Move(p) => {
                    CGMutablePath::move_to_point(Some(&path), std::ptr::null(), p.x, p.y);
                }
                Segment::Line(p) => {
                    CGMutablePath::add_line_to_point(Some(&path), std::ptr::null(), p.x, p.y);
                }
                Segment::Curve { control, to } => {
                    CGMutablePath::add_quad_curve_to_point(
                        Some(&path),
                        std::ptr::null(),
                        control.x,
                        control.y,
                        to.x,
                        to.y,
                    );
                }
            }
        }
    }
    path
}

/// The pointer while the marker is on.
///
/// A disc in the ink's colour, so the pointer says what it will draw, inside a dark ring
/// and a light one, so it stays visible over any content the way the system's own
/// pointer keeps an outline. The hotspot is the centre: ink lands where the disc is.
fn cursor(color: Rgb) -> Option<Retained<NSCursor>> {
    const PIXELS: usize = 32;
    const POINTS: f64 = 16.0;
    // Radii in pixels at 2x, so the disc is the stroke's own width on screen.
    const DISC: f64 = WIDTH;
    const DARK: f64 = DISC + 1.5;
    const LIGHT: f64 = DARK + 1.5;

    let (r, g, b) = color.as_unit();
    let image = bitmap::raster(PIXELS, POINTS, |dx, dy| {
        let distance = (dx * dx + dy * dy).sqrt();
        let light = [1.0, 1.0, 1.0, 1.0].map(|c| c * bitmap::coverage(distance, LIGHT));
        let dark = [0.1, 0.1, 0.1, 1.0].map(|c| c * bitmap::coverage(distance, DARK));
        let disc = [r, g, b, 1.0].map(|c| c * bitmap::coverage(distance, DISC));
        over(disc, over(dark, light)).map(|c| (c * 255.0).round() as u8)
    })?;
    Some(NSCursor::initWithImage_hotSpot(
        NSCursor::alloc(),
        &image,
        NSPoint::new(POINTS / 2.0, POINTS / 2.0),
    ))
}

/// Composite one premultiplied colour over another.
fn over(top: [f64; 4], under: [f64; 4]) -> [f64; 4] {
    let mut out = [0.0; 4];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = top[i] + under[i] * (1.0 - top[3]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xy(point: CGPoint) -> (f64, f64) {
        (point.x, point.y)
    }

    #[test]
    fn a_click_is_a_dot() {
        let dot = segments(&[CGPoint::new(10.0, 20.0)]);
        assert_eq!(dot.len(), 2);
        let Segment::Move(from) = dot[0] else {
            panic!("a stroke starts with a move, got {:?}", dot[0]);
        };
        let Segment::Line(to) = dot[1] else {
            panic!(
                "a single point ends in a zero length line, got {:?}",
                dot[1]
            );
        };
        assert_eq!(xy(from), xy(to));
    }

    #[test]
    fn two_points_are_a_straight_line() {
        let line = segments(&[CGPoint::new(0.0, 0.0), CGPoint::new(10.0, 0.0)]);
        assert_eq!(line.len(), 2);
        let Segment::Line(to) = line[1] else {
            panic!("got {:?}", line[1]);
        };
        assert_eq!(xy(to), (10.0, 0.0));
    }

    /// The property the smoothing exists for: the ink starts and ends where the pointer
    /// did, and every sample in between steers a curve rather than making a corner.
    #[test]
    fn a_polyline_bends_through_its_midpoints_and_ends_where_the_pointer_did() {
        let points = [
            CGPoint::new(0.0, 0.0),
            CGPoint::new(10.0, 0.0),
            CGPoint::new(10.0, 10.0),
            CGPoint::new(20.0, 10.0),
        ];
        let stroke = segments(&points);
        assert_eq!(stroke.len(), 5);

        let Segment::Move(start) = stroke[0] else {
            panic!("got {:?}", stroke[0]);
        };
        assert_eq!(xy(start), (0.0, 0.0));
        let Segment::Line(first_mid) = stroke[1] else {
            panic!("got {:?}", stroke[1]);
        };
        assert_eq!(xy(first_mid), (5.0, 0.0));
        let Segment::Curve { control, to } = stroke[2] else {
            panic!("got {:?}", stroke[2]);
        };
        assert_eq!(xy(control), (10.0, 0.0));
        assert_eq!(xy(to), (10.0, 5.0));
        let Segment::Curve { control, to } = stroke[3] else {
            panic!("got {:?}", stroke[3]);
        };
        assert_eq!(xy(control), (10.0, 10.0));
        assert_eq!(xy(to), (15.0, 10.0));
        let Segment::Line(end) = stroke[4] else {
            panic!("got {:?}", stroke[4]);
        };
        assert_eq!(xy(end), (20.0, 10.0));
    }

    #[test]
    fn nothing_draws_nothing() {
        assert!(segments(&[]).is_empty());
    }

    #[test]
    fn compositing_keeps_the_top_colour_where_it_is_solid() {
        let disc = over([1.0, 0.5, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(disc, [1.0, 0.5, 0.0, 1.0]);
        let edge = over([0.5, 0.25, 0.0, 0.5], [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(edge, [1.0, 0.75, 0.5, 1.0]);
    }
}
