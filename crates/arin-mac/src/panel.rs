//! The overlay window.
//!
//! One `NSPanel` per display, covering it entirely. Everything about how it is
//! configured exists to make it invisible to the rest of the system:
//!
//! - **Non activating.** Clicking near it never steals focus from the app underneath,
//!   and Arin never becomes the frontmost application.
//! - **Click through.** `setIgnoresMouseEvents` means events land on whatever is
//!   underneath. There are no controls in the overlay, so there is nothing to click.
//!   The one exception is the marker, which the person switches on themselves: for as
//!   long as it is on, the panel takes the mouse so they can draw on it, and drops under
//!   the menu bar and the Dock so the way to switch it off stays reachable.
//! - **All Spaces, stationary.** The overlay does not travel with a Space switch or
//!   slide during the transition, and it shows up over full screen apps.
//! - **Borderless and transparent.** No title bar, no shadow, no background.
//!
//! Not one of these needs the Accessibility permission. That is the point. Nor does
//! taking the mouse for the marker: a window receiving a click is the ordinary thing a
//! window does, and nothing here posts one.

use crate::display::Screen;
use crate::host;
use crate::marker::Input;
use arin_protocol::DisplayId;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send,
};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSEvent, NSPanel, NSScreenSaverWindowLevel, NSTrackingArea,
    NSTrackingAreaOptions, NSView, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect};
use objc2_quartz_core::CALayer;

/// How far above ordinary windows the overlay sits.
///
/// Below the screen saver and the login window, above everything a user works in,
/// including the menu bar so an agent can point at a menu item.
fn overlay_level() -> isize {
    NSScreenSaverWindowLevel - 1
}

/// Where the overlay sits while the marker is on.
///
/// One below the Dock, which is at 20, `kCGDockWindowLevel`, with the menu bar above it
/// at `NSMainMenuWindowLevel`. A panel that takes every click and covers the menu bar
/// has just covered the one place the marker can be switched off from, so for as long
/// as it is on the panel goes under both. AppKit's own name for the Dock's level is
/// deprecated, which is why the number is written here.
fn marker_level() -> isize {
    19
}

/// What the content view carries: the display its panel covers.
struct Ivars {
    display: DisplayId,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements beyond the main thread, which the
    // thread kind enforces, and this type has no Drop.
    #[unsafe(super(NSView))]
    #[name = "ArinOverlayView"]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    struct OverlayView;

    /// Mouse handling, which only ever runs while the marker is on.
    ///
    /// With the marker off, the window ignores mouse events and none of this is reached:
    /// there is no path by which the overlay takes a click the person did not ask it to.
    impl OverlayView {
        /// Every click is a first click. The panel is never key, and a view in a window
        /// that is not key is asked this before it is handed a mouse down.
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        /// The overlay takes the click itself, whatever subview is under the pointer.
        /// The only subviews are the glass of a text box, which is display only.
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> Option<Retained<NSView>> {
            Some(Retained::into_super(self.retain()))
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.input(Input::Begin(event.locationInWindow()));
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.input(Input::Extend(event.locationInWindow()));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            self.input(Input::End);
        }

        /// A right click, which is what a two finger click on a trackpad is.
        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, _event: &NSEvent) {
            self.input(Input::Clear);
        }

        /// Asked by the tracking area whenever the pointer arrives over the panel.
        #[unsafe(method(cursorUpdate:))]
        fn cursor_update(&self, _event: &NSEvent) {
            host::show_marker_cursor(self.mtm());
        }
    }
);

impl OverlayView {
    fn new(frame: NSRect, display: DisplayId, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars { display });
        // SAFETY: `initWithFrame:` is NSView's designated initialiser, and the ivars are
        // set before it runs.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    /// Hand the mouse to the marker, in this panel's own coordinates.
    ///
    /// The window's coordinates are the view's, which are the layer tree's: the view
    /// fills a borderless window and the root layer fills the view, all with the origin
    /// at the bottom left. So a location in the window is a point in the layer tree with
    /// no conversion, which is the one thing that makes drawing where the pointer is a
    /// matter of handing the number on.
    fn input(&self, input: Input) {
        host::marker_input(self.ivars().display, input, self.mtm());
    }
}

/// A full screen overlay for one display.
pub struct Panel {
    screen: Screen,
    panel: Retained<NSPanel>,
    view: Retained<OverlayView>,
    root: Retained<CALayer>,
    /// What asks the view for a cursor while the marker is on. Absent while it is off.
    tracking: Option<Retained<NSTrackingArea>>,
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
        let view = OverlayView::new(bounds, screen.info.id, mtm);

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

        // Regardless, because the panel is non activating and must appear without Arin
        // ever becoming the active application.
        panel.orderFrontRegardless();

        Self {
            screen,
            panel,
            view,
            root,
            tracking: None,
        }
    }

    /// The display this covers.
    pub fn screen(&self) -> Screen {
        self.screen
    }

    /// The layer annotations are added to.
    ///
    /// Sublayers are positioned in AppKit coordinates, not protocol ones. Convert first.
    ///
    /// The orb is not one of them. There is a single orb for the whole system and it is
    /// re-parented into whichever panel it is currently over, so a panel is somewhere the
    /// orb visits rather than something that owns one.
    pub fn root(&self) -> &CALayer {
        &self.root
    }

    /// The view annotation subviews are added to.
    ///
    /// Subviews composite above every sublayer of [`Self::root`], so anything added here
    /// draws over the marks. Today that is the blurred glass of a text box, which samples
    /// what is behind the window and so has to be a view: no layer can reach outside its
    /// own window to blur what another window is showing.
    pub fn content(&self) -> Retained<NSView> {
        self.panel
            .contentView()
            .expect("the panel was built with a content view")
    }

    /// Hand the mouse to the marker, or give it back.
    ///
    /// On, the panel stops ignoring mouse events, and that is the whole of how a click
    /// reaches [`OverlayView`]: once `setIgnoresMouseEvents` has been called with `false`,
    /// a window receives every click in its frame, transparent or not. The panel also
    /// drops to [`marker_level`], so marks under the menu bar or the Dock are hidden for
    /// as long as the marker is on and come back when it goes off. The tracking area is
    /// what turns the pointer into a marker tip: the panel is never key, so the cursor
    /// has to be asked for by an area that is active always.
    pub fn set_marker(&mut self, on: bool) {
        self.panel.setIgnoresMouseEvents(!on);
        self.panel
            .setLevel(if on { marker_level() } else { overlay_level() });
        if on {
            if self.tracking.is_none() {
                let owner: &AnyObject = &self.view;
                // SAFETY: the owner answers `cursorUpdate:`, and the panel holds both the
                // view and the area, so neither outlives the other.
                let area = unsafe {
                    NSTrackingArea::initWithRect_options_owner_userInfo(
                        NSTrackingArea::alloc(),
                        self.view.bounds(),
                        NSTrackingAreaOptions::CursorUpdate
                            | NSTrackingAreaOptions::ActiveAlways
                            | NSTrackingAreaOptions::InVisibleRect,
                        Some(owner),
                        None,
                    )
                };
                self.view.addTrackingArea(&area);
                self.tracking = Some(area);
            }
        } else if let Some(area) = self.tracking.take() {
            self.view.removeTrackingArea(&area);
        }
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        // Retaining the panel is what keeps it on screen, so dropping it has to take it
        // off explicitly rather than leaving an orphaned window behind.
        self.panel.close();
    }
}
