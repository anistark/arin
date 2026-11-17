//! Where the selection is, and how a drag reshapes it.
//!
//! Pure geometry in one display's own points, in AppKit's orientation: origin at the
//! bottom left of that display, y growing upward. That is the space a selector panel's
//! layers and views are already in, so nothing here converts on the way to the screen.
//! The one conversion is to ScreenCaptureKit's top left origin, in [`capture_rect`].

use objc2_core_foundation::{CGPoint, CGRect, CGSize};

/// The smallest side a selection can have, in points.
pub(crate) const MINIMUM: f64 = 12.0;

/// How far outside an edge the pointer can be and still take hold of it, in points.
const REACH: f64 = 8.0;

/// How far the pointer has to travel before a press on the dimmed screen draws a new
/// selection, rather than being a stray click that throws the old one away.
pub(crate) const SLOP: f64 = 4.0;

/// Space between the selection and the bar with the buttons, in points.
const GAP: f64 = 10.0;

/// How close the bar may come to the edge of its display, in points.
const MARGIN: f64 = 8.0;

/// Which edges of the selection a drag moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Edges {
    pub left: bool,
    pub right: bool,
    pub bottom: bool,
    pub top: bool,
}

/// What a press near the selection takes hold of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Grip {
    /// The whole selection, to move it.
    Body,
    /// One edge, or the two that meet at a corner, to resize it.
    Edges(Edges),
}

/// The pointer to show for what is under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pointer {
    /// Outside the selection, where a drag draws a new one.
    Crosshair,
    /// Inside it, where a drag moves it.
    Hand,
    /// On an edge or a corner, where a drag resizes it.
    Resize(Edges),
}

impl Grip {
    /// The pointer that says what a drag from here would do.
    pub(crate) fn pointer(grip: Option<Self>) -> Pointer {
        match grip {
            None => Pointer::Crosshair,
            Some(Self::Body) => Pointer::Hand,
            Some(Self::Edges(edges)) => Pointer::Resize(edges),
        }
    }
}

/// The four sides of a rect: left, right, bottom, top.
fn sides(rect: CGRect) -> (f64, f64, f64, f64) {
    (
        rect.origin.x,
        rect.origin.x + rect.size.width,
        rect.origin.y,
        rect.origin.y + rect.size.height,
    )
}

fn from_sides(left: f64, right: f64, bottom: f64, top: f64) -> CGRect {
    CGRect::new(
        CGPoint::new(left, bottom),
        CGSize::new(right - left, top - bottom),
    )
}

/// `value` held between `low` and `high`, preferring `high` if the two have crossed.
///
/// `f64::clamp` panics when they cross, and a mouse handler is the wrong place to find out
/// that some display was narrower than a selection's minimum.
fn within(value: f64, low: f64, high: f64) -> f64 {
    value.max(low).min(high)
}

/// What a press at `at` takes hold of, if anything.
///
/// The edges reach a few points outside the selection so a thin border is easy to catch,
/// and less far inside a small selection so its middle can still be grabbed to move it.
pub(crate) fn grip_at(area: CGRect, at: CGPoint) -> Option<Grip> {
    let (left, right, bottom, top) = sides(area);
    if at.x < left - REACH || at.x > right + REACH || at.y < bottom - REACH || at.y > top + REACH {
        return None;
    }
    let inside_x = REACH.min(area.size.width / 4.0);
    let inside_y = REACH.min(area.size.height / 4.0);
    let edges = Edges {
        left: at.x <= left + inside_x,
        right: at.x >= right - inside_x,
        bottom: at.y <= bottom + inside_y,
        top: at.y >= top - inside_y,
    };
    if edges == Edges::default() {
        Some(Grip::Body)
    } else {
        Some(Grip::Edges(edges))
    }
}

/// `start` with the chosen edges carried by `by`, kept inside `bounds`.
///
/// An edge stops short of the one opposite rather than crossing it, so a selection never
/// turns inside out or shrinks below [`MINIMUM`].
pub(crate) fn resized(start: CGRect, edges: Edges, by: CGPoint, bounds: CGRect) -> CGRect {
    let (mut left, mut right, mut bottom, mut top) = sides(start);
    let (low_x, high_x, low_y, high_y) = sides(bounds);
    if edges.left {
        left = within(left + by.x, low_x, right - MINIMUM);
    }
    if edges.right {
        right = within(right + by.x, left + MINIMUM, high_x);
    }
    if edges.bottom {
        bottom = within(bottom + by.y, low_y, top - MINIMUM);
    }
    if edges.top {
        top = within(top + by.y, bottom + MINIMUM, high_y);
    }
    snapped(from_sides(left, right, bottom, top))
}

/// `area` moved as little as it takes to sit inside `bounds`, and shrunk if it is larger.
pub(crate) fn fitted(area: CGRect, bounds: CGRect) -> CGRect {
    let width = area.size.width.min(bounds.size.width);
    let height = area.size.height.min(bounds.size.height);
    let (low_x, high_x, low_y, high_y) = sides(bounds);
    let x = within(area.origin.x, low_x, high_x - width);
    let y = within(area.origin.y, low_y, high_y - height);
    snapped(CGRect::new(CGPoint::new(x, y), CGSize::new(width, height)))
}

/// A new selection drawn from `anchor` to `at`, kept inside `bounds`.
///
/// Grown away from the anchor to at least [`MINIMUM`] on each side, in the direction the
/// pointer went, so the corner under the pointer stays under it.
pub(crate) fn spanning(anchor: CGPoint, at: CGPoint, bounds: CGRect) -> CGRect {
    let (low_x, high_x, low_y, high_y) = sides(bounds);
    let anchor = CGPoint::new(
        within(anchor.x, low_x, high_x),
        within(anchor.y, low_y, high_y),
    );
    let at = CGPoint::new(within(at.x, low_x, high_x), within(at.y, low_y, high_y));
    let width = (at.x - anchor.x).abs().max(MINIMUM);
    let height = (at.y - anchor.y).abs().max(MINIMUM);
    let x = if at.x < anchor.x {
        anchor.x - width
    } else {
        anchor.x
    };
    let y = if at.y < anchor.y {
        anchor.y - height
    } else {
        anchor.y
    };
    fitted(
        CGRect::new(CGPoint::new(x, y), CGSize::new(width, height)),
        bounds,
    )
}

/// Where a selection starts on a display nobody has selected on yet: centred, half of it
/// each way.
pub(crate) fn initial(bounds: CGRect) -> CGRect {
    let width = (bounds.size.width / 2.0).max(MINIMUM);
    let height = (bounds.size.height / 2.0).max(MINIMUM);
    fitted(
        CGRect::new(
            CGPoint::new(
                bounds.origin.x + (bounds.size.width - width) / 2.0,
                bounds.origin.y + (bounds.size.height - height) / 2.0,
            ),
            CGSize::new(width, height),
        ),
        bounds,
    )
}

/// `area` on whole points.
///
/// The sides are rounded rather than the origin and size, so a side resting on the edge of
/// the display stays on it. Whole points are what the size readout promises, and they
/// keep a capture from starting halfway through a pixel row.
pub(crate) fn snapped(area: CGRect) -> CGRect {
    let (left, right, bottom, top) = sides(area);
    from_sides(left.round(), right.round(), bottom.round(), top.round())
}

/// Where the bar with the buttons goes for a selection.
///
/// Under it and centred, which is where the eye already is after drawing one. Above it
/// when there is no room under it, and inside its bottom edge when there is room for
/// neither, which only happens to a selection that is most of the display tall.
pub(crate) fn toolbar_origin(area: CGRect, toolbar: CGSize, bounds: CGRect) -> CGPoint {
    let (left, right, bottom, top) = sides(area);
    let (low_x, high_x, low_y, high_y) = sides(bounds);
    let x = within(
        (left + right - toolbar.width) / 2.0,
        low_x + MARGIN,
        high_x - MARGIN - toolbar.width,
    );
    let below = bottom - GAP - toolbar.height;
    let above = top + GAP;
    let y = if below >= low_y + MARGIN {
        below
    } else if above + toolbar.height <= high_y - MARGIN {
        above
    } else {
        within(
            bottom + GAP,
            low_y + MARGIN,
            high_y - MARGIN - toolbar.height,
        )
    };
    CGPoint::new(x.round(), y.round())
}

/// `area` in ScreenCaptureKit's terms: points on the same display, origin at the top left.
pub(crate) fn capture_rect(area: CGRect, display_height: f64) -> CGRect {
    CGRect::new(
        CGPoint::new(
            area.origin.x,
            display_height - area.origin.y - area.size.height,
        ),
        area.size,
    )
}

/// The size readout under the selection, in points, which is what a person resizing it
/// sees and what every other measurement on a Mac is given in.
pub(crate) fn describe(size: CGSize) -> String {
    format!("{} × {}", size.width.round(), size.height.round())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> CGRect {
        CGRect::new(CGPoint::new(x, y), CGSize::new(width, height))
    }

    fn parts(rect: CGRect) -> (f64, f64, f64, f64) {
        (
            rect.origin.x,
            rect.origin.y,
            rect.size.width,
            rect.size.height,
        )
    }

    /// A 14 inch panel at its default scaling.
    fn display() -> CGRect {
        rect(0.0, 0.0, 1512.0, 982.0)
    }

    #[test]
    fn the_middle_moves_and_the_edges_resize() {
        let area = rect(100.0, 100.0, 200.0, 100.0);
        assert_eq!(grip_at(area, CGPoint::new(200.0, 150.0)), Some(Grip::Body));
        assert_eq!(
            grip_at(area, CGPoint::new(97.0, 150.0)),
            Some(Grip::Edges(Edges {
                left: true,
                ..Edges::default()
            }))
        );
        assert_eq!(
            grip_at(area, CGPoint::new(303.0, 203.0)),
            Some(Grip::Edges(Edges {
                right: true,
                top: true,
                ..Edges::default()
            }))
        );
        assert_eq!(grip_at(area, CGPoint::new(50.0, 150.0)), None);
    }

    #[test]
    fn a_small_selection_can_still_be_moved_from_its_middle() {
        let area = rect(100.0, 100.0, MINIMUM, MINIMUM);
        let middle = CGPoint::new(100.0 + MINIMUM / 2.0, 100.0 + MINIMUM / 2.0);
        assert_eq!(grip_at(area, middle), Some(Grip::Body));
    }

    #[test]
    fn the_pointer_says_what_a_drag_would_do() {
        let corner = Edges {
            left: true,
            top: true,
            ..Edges::default()
        };
        assert_eq!(Grip::pointer(None), Pointer::Crosshair);
        assert_eq!(Grip::pointer(Some(Grip::Body)), Pointer::Hand);
        assert_eq!(
            Grip::pointer(Some(Grip::Edges(corner))),
            Pointer::Resize(corner)
        );
    }

    #[test]
    fn an_edge_stops_short_of_the_one_opposite() {
        let area = rect(100.0, 100.0, 200.0, 100.0);
        let left = Edges {
            left: true,
            ..Edges::default()
        };
        let shrunk = resized(area, left, CGPoint::new(500.0, 0.0), display());
        assert_eq!(parts(shrunk), (300.0 - MINIMUM, 100.0, MINIMUM, 100.0));
    }

    #[test]
    fn a_resize_stays_on_the_display() {
        let area = rect(100.0, 100.0, 200.0, 100.0);
        let corner = Edges {
            right: true,
            top: true,
            ..Edges::default()
        };
        let grown = resized(area, corner, CGPoint::new(5000.0, 5000.0), display());
        assert_eq!(parts(grown), (100.0, 100.0, 1412.0, 882.0));
    }

    #[test]
    fn a_selection_moved_off_the_edge_stops_at_it() {
        let moved = fitted(rect(1400.0, -50.0, 200.0, 100.0), display());
        assert_eq!(parts(moved), (1312.0, 0.0, 200.0, 100.0));
    }

    #[test]
    fn a_selection_too_big_for_a_smaller_display_shrinks_to_fit() {
        let small = rect(0.0, 0.0, 1280.0, 800.0);
        let moved = fitted(rect(10.0, 10.0, 1500.0, 900.0), small);
        assert_eq!(parts(moved), (0.0, 0.0, 1280.0, 800.0));
    }

    #[test]
    fn a_new_selection_can_be_drawn_in_any_direction() {
        let anchor = CGPoint::new(400.0, 400.0);
        let down_left = spanning(anchor, CGPoint::new(300.0, 350.0), display());
        assert_eq!(parts(down_left), (300.0, 350.0, 100.0, 50.0));
        let up_right = spanning(anchor, CGPoint::new(500.0, 450.0), display());
        assert_eq!(parts(up_right), (400.0, 400.0, 100.0, 50.0));
    }

    #[test]
    fn a_new_selection_is_never_smaller_than_the_minimum() {
        let anchor = CGPoint::new(400.0, 400.0);
        let tiny = spanning(anchor, CGPoint::new(398.0, 401.0), display());
        assert_eq!(parts(tiny), (400.0 - MINIMUM, 400.0, MINIMUM, MINIMUM));
    }

    #[test]
    fn a_new_selection_dragged_off_the_display_ends_at_its_edge() {
        let drawn = spanning(
            CGPoint::new(1400.0, 100.0),
            CGPoint::new(2000.0, -300.0),
            display(),
        );
        assert_eq!(parts(drawn), (1400.0, 0.0, 112.0, 100.0));
    }

    #[test]
    fn the_first_selection_is_centred_on_its_display() {
        assert_eq!(parts(initial(display())), (378.0, 246.0, 756.0, 491.0));
    }

    #[test]
    fn sides_land_on_whole_points() {
        let area = snapped(rect(10.4, 20.6, 100.2, 50.3));
        assert_eq!(parts(area), (10.0, 21.0, 101.0, 50.0));
    }

    #[test]
    fn the_bar_sits_under_the_selection_when_there_is_room() {
        let bar = CGSize::new(240.0, 40.0);
        let origin = toolbar_origin(rect(400.0, 300.0, 400.0, 200.0), bar, display());
        assert_eq!((origin.x, origin.y), (480.0, 250.0));
    }

    #[test]
    fn the_bar_goes_above_a_selection_at_the_bottom_of_the_display() {
        let bar = CGSize::new(240.0, 40.0);
        let origin = toolbar_origin(rect(400.0, 20.0, 400.0, 200.0), bar, display());
        assert_eq!((origin.x, origin.y), (480.0, 230.0));
    }

    #[test]
    fn the_bar_goes_inside_a_selection_that_fills_the_display() {
        let bar = CGSize::new(240.0, 40.0);
        let origin = toolbar_origin(display(), bar, display());
        assert_eq!((origin.x, origin.y), (636.0, 10.0));
    }

    #[test]
    fn the_bar_stays_on_the_display_beside_a_selection_at_its_edge() {
        let bar = CGSize::new(240.0, 40.0);
        let origin = toolbar_origin(rect(0.0, 300.0, 100.0, 100.0), bar, display());
        assert_eq!(origin.x, 8.0);
    }

    #[test]
    fn a_capture_is_measured_from_the_top_of_the_display() {
        let area = rect(100.0, 82.0, 300.0, 100.0);
        assert_eq!(
            parts(capture_rect(area, 982.0)),
            (100.0, 800.0, 300.0, 100.0)
        );
    }

    #[test]
    fn the_readout_is_in_whole_points() {
        assert_eq!(describe(CGSize::new(640.0, 400.0)), "640 × 400");
    }
}
