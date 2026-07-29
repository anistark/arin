//! The overlay window.
//!
//! One `NSPanel` per display, covering it entirely. Everything about how it is
//! configured exists to make it invisible to the rest of the system:
//!
//! - **Non activating.** Clicking near it never steals focus from the app underneath,
//!   and Arin never becomes the frontmost application.
//! - **Click through.** `setIgnoresMouseEvents` means events land on whatever is
//!   underneath. There are no controls in the overlay, so there is nothing to click.
//! - **All Spaces, stationary.** The overlay does not travel with a Space switch or
//!   slide during the transition, and it shows up over full screen apps.
//! - **Borderless and transparent.** No title bar, no shadow, no background.
//!
//! Not one of these needs the Accessibility permission. That is the point.

use crate::display::Screen;
use crate::orb::Orb;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSPanel, NSScreenSaverWindowLevel, NSView,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::NSRect;
use objc2_quartz_core::CALayer;

/// How far above ordinary windows the overlay sits.
///
/// Below the screen saver and the login window, above everything a user works in,
/// including the menu bar so an agent can point at a menu item.
fn overlay_level() -> isize {
    NSScreenSaverWindowLevel - 1
}

/// A full screen overlay for one display.
pub struct Panel {
    screen: Screen,
    panel: Retained<NSPanel>,
    root: Retained<CALayer>,
    orb: Orb,
}

impl Panel {
    /// Create and show the overlay for a screen.
    pub fn new(screen: Screen, mtm: MainThreadMarker) -> Self {
        let panel = {
            NSPanel::initWithContentRect_styleMask_backing_defer(
                NSPanel::alloc(mtm),
                screen.frame,
                NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                NSBackingStoreType::Buffered,
                false,
            )
        };

        panel.setOpaque(false);
        let clear = NSColor::clearColor();
        panel.setBackgroundColor(Some(&clear));
        panel.setHasShadow(false);
        panel.setLevel(overlay_level());
        panel.setIgnoresMouseEvents(true);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        // The daemon owns this panel's lifetime. Without this, closing it would free it
        // underneath us.
        unsafe { panel.setReleasedWhenClosed(false) };

        let bounds = NSRect::new(objc2_foundation::NSPoint::new(0.0, 0.0), screen.frame.size);
        let view = NSView::initWithFrame(NSView::alloc(mtm), bounds);

        let root = CALayer::new();
        root.setFrame(bounds);

        // Assigning the layer before asking for one makes this a layer *hosting* view,
        // so the layer is ours to keep sublayers on rather than one AppKit may replace.
        view.setLayer(Some(&root));
        view.setWantsLayer(true);

        // Left in AppKit's orientation, y growing upward from the bottom left.
        // `setGeometryFlipped(true)` does not take on this view. Inverting y is
        // arithmetic in `host`, where it can be tested.
        panel.setContentView(Some(&view));

        let orb = Orb::new();
        orb.set_visible(false);
        root.addSublayer(orb.layer());

        // Regardless, because the panel is non activating and must appear without Arin
        // ever becoming the active application.
        panel.orderFrontRegardless();

        Self {
            screen,
            panel,
            root,
            orb,
        }
    }

    /// The display this covers.
    pub fn screen(&self) -> Screen {
        self.screen
    }

    /// The layer annotations are added to.
    ///
    /// Sublayers are positioned in AppKit coordinates, not protocol ones. Convert first.
    pub fn root(&self) -> &CALayer {
        &self.root
    }

    /// The orb living in this panel.
    pub fn orb_mut(&mut self) -> &mut Orb {
        &mut self.orb
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        // Retaining the panel is what keeps it on screen, so dropping it has to take it
        // off explicitly rather than leaving an orphaned window behind.
        self.panel.close();
    }
}
