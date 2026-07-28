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
    /// Convert a logical point on this display into AppKit's global space.
    ///
    /// The protocol measures from the top left of the display and grows downward, which
    /// is what a screenshot looks like. AppKit measures from the bottom left of the
    /// primary display and grows upward. Everything above this function is in protocol
    /// coordinates and everything below it is in AppKit's.
    ///
    /// Within a single panel this is not needed, because the root layer is flipped and
    /// protocol coordinates land directly. It is for the cases that cross displays, such
    /// as flying the orb from one screen to another.
    pub fn to_global(&self, x: f64, y: f64) -> NSPoint {
        NSPoint::new(
            self.frame.origin.x + x,
            self.frame.origin.y + self.frame.size.height - y,
        )
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
    fn the_protocol_origin_is_the_top_left() {
        let s = screen(0.0, 0.0, 1512.0, 982.0);
        // Protocol (0, 0) is the top left, which in AppKit is the full height up.
        assert_eq!(s.to_global(0.0, 0.0), NSPoint::new(0.0, 982.0));
        // Protocol y grows downward, so the bottom edge is AppKit's zero.
        assert_eq!(s.to_global(0.0, 982.0), NSPoint::new(0.0, 0.0));
    }

    #[test]
    fn x_is_unchanged_and_y_is_inverted() {
        let s = screen(0.0, 0.0, 1512.0, 982.0);
        assert_eq!(s.to_global(756.0, 200.0), NSPoint::new(756.0, 782.0));
    }

    #[test]
    fn a_secondary_display_is_offset_by_its_origin() {
        // A second screen to the right of the primary.
        let s = screen(1512.0, 0.0, 1000.0, 800.0);
        assert_eq!(s.to_global(0.0, 0.0), NSPoint::new(1512.0, 800.0));
        assert_eq!(s.to_global(100.0, 800.0), NSPoint::new(1612.0, 0.0));
    }

    #[test]
    fn a_display_below_the_primary_has_a_negative_origin() {
        // AppKit puts screens below the primary at negative y.
        let s = screen(0.0, -800.0, 1000.0, 800.0);
        assert_eq!(s.to_global(0.0, 0.0), NSPoint::new(0.0, 0.0));
        assert_eq!(s.to_global(0.0, 800.0), NSPoint::new(0.0, -800.0));
    }
}
