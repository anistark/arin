//! Display enumeration.
//!
//! `DisplayId` is the `CGDirectDisplayID` that AppKit reports, so it stays stable across
//! daemon restarts for the same physical arrangement. Clients should read the id out of
//! an ack rather than assume one.

use arin_protocol::{DisplayId, DisplayInfo};
use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;
use objc2_foundation::{NSPoint, NSRect, NSString};

/// A display, with what the renderer needs on top of what clients are told.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Screen {
    /// What the protocol reports.
    pub info: DisplayInfo,
    /// The screen's frame in AppKit's global space, origin bottom left of the primary.
    pub frame: NSRect,
}

impl Screen {
    /// Convert a point in this panel's layer space into AppKit's global space.
    ///
    /// A panel's root layer is left in AppKit's orientation, y growing upward, with its
    /// origin at the bottom left of its own display. So this differs from global space by
    /// the display's origin and nothing else.
    ///
    /// The protocol's own inversion, y growing downward from the top left, is applied in
    /// `host` on the way in and is already gone by the time anything reaches here.
    ///
    /// Needed because the orb's flight is computed once across the whole desktop and then
    /// drawn in segments, one per panel it crosses. A panel cannot draw outside its own
    /// window, so the arc has to be cut at the screen boundary and each piece expressed in
    /// the coordinates of the panel that draws it.
    pub fn layer_to_global(&self, point: NSPoint) -> NSPoint {
        NSPoint::new(self.frame.origin.x + point.x, self.frame.origin.y + point.y)
    }

    /// Convert a point in AppKit's global space into this panel's layer space.
    ///
    /// The inverse of [`Screen::layer_to_global`], and deliberately not clamped: a sample
    /// on the flight path that has left this display converts to coordinates outside it,
    /// which is what makes it possible to ask where the arc crossed.
    pub fn global_to_layer(&self, point: NSPoint) -> NSPoint {
        NSPoint::new(point.x - self.frame.origin.x, point.y - self.frame.origin.y)
    }

    /// Whether a point in AppKit's global space falls on this display.
    ///
    /// Half open, so a point on the shared edge of two abutting displays belongs to
    /// exactly one of them. Without that a flight crossing the boundary would find two
    /// answers at the crossing and stitch a zero length segment between them.
    pub fn contains(&self, point: NSPoint) -> bool {
        let origin = self.frame.origin;
        let size = self.frame.size;
        point.x >= origin.x
            && point.x < origin.x + size.width
            && point.y >= origin.y
            && point.y < origin.y + size.height
    }
}

/// Every connected display, in AppKit's order.
pub fn screens(mtm: MainThreadMarker) -> Vec<Screen> {
    NSScreen::screens(mtm)
        .iter()
        .filter_map(|screen| {
            let frame = screen.frame();
            Some(Screen {
                info: DisplayInfo {
                    id: DisplayId(display_id(&screen)?),
                    scale: screen.backingScaleFactor(),
                    logical_size: [frame.size.width, frame.size.height],
                },
                frame,
            })
        })
        .collect()
}

/// Read `CGDirectDisplayID` out of a screen's device description.
///
/// Returns `None` for a screen that does not report one, which should not happen on a
/// real display but is not worth panicking over on a timer.
fn display_id(screen: &NSScreen) -> Option<u32> {
    let key = NSString::from_str("NSScreenNumber");
    let description = screen.deviceDescription();
    let value = description.objectForKey(&key)?;
    let number = value.downcast::<objc2_foundation::NSNumber>().ok()?;
    Some(number.unsignedIntValue())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arin_protocol::DisplayInfo;
    use objc2_foundation::{NSRect, NSSize};

    fn screen(origin_x: f64, origin_y: f64, w: f64, h: f64) -> Screen {
        Screen {
            info: DisplayInfo {
                id: DisplayId(1),
                scale: 2.0,
                logical_size: [w, h],
            },
            frame: NSRect::new(NSPoint::new(origin_x, origin_y), NSSize::new(w, h)),
        }
    }

    #[test]
    fn the_primary_display_needs_no_translation() {
        // AppKit's origin is the primary's bottom left, and so is its panel's.
        let s = screen(0.0, 0.0, 1512.0, 982.0);
        assert_eq!(
            s.layer_to_global(NSPoint::new(756.0, 200.0)),
            NSPoint::new(756.0, 200.0)
        );
    }

    #[test]
    fn a_secondary_display_is_offset_by_its_origin() {
        // A second screen to the right of the primary.
        let s = screen(1512.0, 0.0, 1000.0, 800.0);
        assert_eq!(
            s.layer_to_global(NSPoint::new(0.0, 0.0)),
            NSPoint::new(1512.0, 0.0)
        );
        assert_eq!(
            s.layer_to_global(NSPoint::new(100.0, 800.0)),
            NSPoint::new(1612.0, 800.0)
        );
    }

    #[test]
    fn a_display_below_the_primary_has_a_negative_origin() {
        // AppKit puts screens below the primary at negative y.
        let s = screen(0.0, -800.0, 1000.0, 800.0);
        assert_eq!(
            s.layer_to_global(NSPoint::new(0.0, 0.0)),
            NSPoint::new(0.0, -800.0)
        );
        assert_eq!(
            s.global_to_layer(NSPoint::new(0.0, 0.0)),
            NSPoint::new(0.0, 800.0)
        );
    }

    #[test]
    fn layer_and_global_round_trip_on_every_arrangement() {
        for s in [
            screen(0.0, 0.0, 1512.0, 982.0),
            screen(1512.0, 0.0, 1920.0, 1080.0),
            screen(0.0, -800.0, 1000.0, 800.0),
            screen(-1920.0, 240.0, 1920.0, 1080.0),
        ] {
            for (x, y) in [(0.0, 0.0), (10.0, 700.0), (411.0, 88.0)] {
                let start = NSPoint::new(x, y);
                let there_and_back = s.global_to_layer(s.layer_to_global(start));
                assert_eq!(there_and_back, start, "{:?}", s.frame);
            }
        }
    }

    #[test]
    fn a_display_holds_its_own_origin_and_not_the_far_edges() {
        let s = screen(1512.0, 0.0, 1920.0, 1080.0);
        assert!(s.contains(NSPoint::new(1512.0, 0.0)));
        assert!(s.contains(NSPoint::new(3431.0, 1079.0)));
        // Half open: the far edges belong to whatever display starts there.
        assert!(!s.contains(NSPoint::new(3432.0, 500.0)));
        assert!(!s.contains(NSPoint::new(2000.0, 1080.0)));
        assert!(!s.contains(NSPoint::new(1511.0, 500.0)));
    }

    #[test]
    fn abutting_displays_do_not_both_claim_the_shared_edge() {
        let left = screen(0.0, 0.0, 1512.0, 982.0);
        let right = screen(1512.0, 0.0, 1920.0, 1080.0);
        let seam = NSPoint::new(1512.0, 400.0);
        assert!(!left.contains(seam));
        assert!(right.contains(seam));
    }
}
