//! The windows a screenshot area is chosen on.
//!
//! One panel per display, above the overlay, dimming everything outside the selection.
//! These are separate from the overlay panels rather than a mode of them, for the same
//! reason the marker drops its panels under the menu bar: the overlay's windows are click
//! through and never key, and those two properties are what keep it out of the way. The
//! selector needs the opposite of both for as long as it is up, so it gets windows of its
//! own and gives them back the moment it is done.
//!
//! The bar with the buttons is the only control Arin ever puts on the screen. It belongs
//! to the person, who opened the selector from the menu bar, and it is gone again before
//! the screenshot is taken.

use super::Input;
use super::area;
use crate::display::Screen;
use crate::host;
use arin_protocol::DisplayId;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameDarkAqua, NSBackingStoreType,
    NSButton, NSColor, NSControlSize, NSCursor, NSCursorFrameResizeDirections,
    NSCursorFrameResizePosition, NSEvent, NSFont, NSFontWeightMedium, NSPanel,
    NSScreenSaverWindowLevel, NSTextAlignment, NSTextField, NSTrackingArea, NSTrackingAreaOptions,
    NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSVisualEffectView, NSWindowAnimationBehavior, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGMutablePath;
use objc2_foundation::NSString;
use objc2_quartz_core::{CALayer, CAShapeLayer};

/// How dark the screen outside the selection goes.
const DIM: f64 = 0.38;

/// Radius of a resize handle, in points.
const HANDLE: f64 = 4.5;

/// Space around and between what is in the bar, in points.
const PADDING: f64 = 8.0;

/// Width kept for the size readout, wide enough for two five digit numbers.
const READOUT: f64 = 108.0;

/// Narrowest a button in the bar is, so the two read as a pair.
const BUTTON: f64 = 88.0;

/// Above the overlay, which sits one below the screen saver, so the marks and the orb are
/// under the dimming like everything else, and the menu bar can be selected too.
fn selector_level() -> isize {
    NSScreenSaverWindowLevel
}

define_class!(
    // SAFETY: NSPanel has no subclassing requirements beyond the main thread, which the
    // thread kind enforces, and this type has no Drop.
    #[unsafe(super(NSPanel))]
    #[name = "ArinSelectorPanel"]
    #[thread_kind = MainThreadOnly]
    pub(super) struct SelectorWindow;

    impl SelectorWindow {
        /// A borderless window refuses to become key unless it says otherwise, and being
        /// key is how Return and Escape reach the bar's buttons. The panel is non
        /// activating, so taking the keyboard does not make Arin the active application.
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }
    }
);

/// What the content view carries: the display its panel covers.
struct ViewIvars {
    display: DisplayId,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements beyond the main thread, which the
    // thread kind enforces, and this type has no Drop.
    #[unsafe(super(NSView))]
    #[name = "ArinSelectorView"]
    #[thread_kind = MainThreadOnly]
    #[ivars = ViewIvars]
    struct SelectorView;

    /// The mouse, handed on to the selection in this panel's own coordinates.
    ///
    /// The view fills a borderless window and the root layer fills the view, all with the
    /// origin at the bottom left, so a location in the window is a point in the layer tree
    /// with no conversion. A drag that leaves the window keeps arriving here, with
    /// coordinates outside it, which is what lets a selection be carried to another
    /// display.
    impl SelectorView {
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.send(Input::Press(event.locationInWindow()));
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.send(Input::Drag(event.locationInWindow()));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.send(Input::Release(event.locationInWindow()));
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.send(Input::Hover(event.locationInWindow()));
        }

        #[unsafe(method(cursorUpdate:))]
        fn cursor_update(&self, event: &NSEvent) {
            self.send(Input::Hover(event.locationInWindow()));
        }

        /// A right click, or a two finger click on a trackpad, puts the selector away, as
        /// the same click wipes the marker.
        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, _event: &NSEvent) {
            self.send(Input::Cancel);
        }
    }
);

impl SelectorView {
    fn new(frame: CGRect, display: DisplayId, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ViewIvars { display });
        // SAFETY: `initWithFrame:` is NSView's designated initialiser, and the ivars are
        // set before it runs.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn send(&self, input: Input) {
        super::input(self.ivars().display, input, self.mtm());
    }
}

/// The selector on one display.
pub(super) struct SelectorPanel {
    screen: Screen,
    window: Retained<SelectorWindow>,
    view: Retained<SelectorView>,
    /// The dark wash, with a hole where the selection is.
    dim: Retained<CAShapeLayer>,
    /// The selection's outline.
    outline: Retained<CAShapeLayer>,
    /// The resize handles.
    handles: Retained<CAShapeLayer>,
    /// What asks the view for a pointer and tells it where the mouse is.
    _tracking: Retained<NSTrackingArea>,
}

impl SelectorPanel {
    /// Cover a display, dimmed.
    pub(super) fn new(screen: Screen, mtm: MainThreadMarker) -> Self {
        let mask = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
        // SAFETY: NSPanel's designated initialiser, on a subclass that adds no ivars.
        let window: Retained<SelectorWindow> = unsafe {
            msg_send![
                SelectorWindow::alloc(mtm),
                initWithContentRect: screen.frame,
                styleMask: mask,
                backing: NSBackingStoreType::Buffered,
                defer: false
            ]
        };
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(false);
        window.setLevel(selector_level());
        window.setHidesOnDeactivate(false);
        // Gone at once when it is closed. A window fading out could still be on screen
        // as the capture is taken.
        window.setAnimationBehavior(NSWindowAnimationBehavior::None);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        // SAFETY: the panel is held for as long as it is open and closed in `Drop`.
        unsafe { window.setReleasedWhenClosed(false) };

        let bounds = CGRect::new(CGPoint::new(0.0, 0.0), screen.frame.size);
        let view = SelectorView::new(bounds, screen.info.id, mtm);
        let root = CALayer::new();
        root.setFrame(bounds);
        // A layer hosting view, as the overlay's is, so the sublayers are ours to keep.
        view.setLayer(Some(&root));
        view.setWantsLayer(true);

        let dim = CAShapeLayer::new();
        dim.setFillColor(Some(&host::srgb(0.0, 0.0, 0.0, DIM)));
        dim.setFillRule(unsafe { objc2_quartz_core::kCAFillRuleEvenOdd });
        root.addSublayer(&dim);

        let outline = CAShapeLayer::new();
        outline.setFillColor(None);
        outline.setStrokeColor(Some(&host::srgb(1.0, 1.0, 1.0, 0.95)));
        outline.setLineWidth(1.0);
        // A shadow keeps the white outline visible over white content.
        outline.setShadowColor(Some(&host::srgb(0.0, 0.0, 0.0, 1.0)));
        outline.setShadowOpacity(0.45);
        outline.setShadowRadius(1.5);
        outline.setShadowOffset(CGSize::new(0.0, 0.0));
        root.addSublayer(&outline);

        let handles = CAShapeLayer::new();
        handles.setFillColor(Some(&host::srgb(1.0, 1.0, 1.0, 1.0)));
        handles.setStrokeColor(Some(&host::srgb(0.0, 0.0, 0.0, 0.35)));
        handles.setLineWidth(0.5);
        root.addSublayer(&handles);

        window.setContentView(Some(&view));

        let owner: &AnyObject = &view;
        // SAFETY: the owner answers `mouseMoved:` and `cursorUpdate:`, and the panel holds
        // both the view and the area, so neither outlives the other.
        let tracking = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                bounds,
                NSTrackingAreaOptions::MouseMoved
                    | NSTrackingAreaOptions::CursorUpdate
                    | NSTrackingAreaOptions::ActiveAlways
                    | NSTrackingAreaOptions::InVisibleRect,
                Some(owner),
                None,
            )
        };
        view.addTrackingArea(&tracking);

        window.orderFrontRegardless();

        Self {
            screen,
            window,
            view,
            dim,
            outline,
            handles,
            _tracking: tracking,
        }
    }

    pub(super) fn screen(&self) -> Screen {
        self.screen
    }

    /// The whole display, in this panel's coordinates.
    pub(super) fn bounds(&self) -> CGRect {
        CGRect::new(CGPoint::new(0.0, 0.0), self.screen.frame.size)
    }

    /// The window server's id for this panel, which is what ScreenCaptureKit is told to
    /// leave out of the picture.
    pub(super) fn window_id(&self) -> Option<u32> {
        u32::try_from(self.window.windowNumber()).ok()
    }

    /// The view the bar is placed in, when the selection is on this display.
    pub(super) fn content(&self) -> &NSView {
        &self.view
    }

    /// Take the keyboard, so Return and Escape reach the bar.
    pub(super) fn make_key(&self) {
        self.window.makeKeyWindow();
    }

    /// Dim the display, leaving `area` clear and outlined, or dim all of it.
    pub(super) fn show(&self, area: Option<CGRect>) {
        let dim = CGMutablePath::new();
        add_rect(&dim, self.bounds());
        if let Some(area) = area {
            add_rect(&dim, area);
        }
        self.dim.setPath(Some(&dim));

        match area {
            Some(area) => {
                let outline = CGMutablePath::new();
                // Half a point in, so the one point line is crisp and inside the area.
                add_rect(&outline, inset(area, 0.5));
                self.outline.setPath(Some(&outline));
                self.handles.setPath(Some(&handles(area)));
            }
            None => {
                self.outline.setPath(None);
                self.handles.setPath(None);
            }
        }
    }
}

impl Drop for SelectorPanel {
    fn drop(&mut self) {
        self.window.close();
    }
}

fn add_rect(path: &CFRetained<CGMutablePath>, rect: CGRect) {
    // SAFETY: the path is live, and a null transform means none.
    unsafe { CGMutablePath::add_rect(Some(path), std::ptr::null(), rect) };
}

fn inset(rect: CGRect, by: f64) -> CGRect {
    CGRect::new(
        CGPoint::new(rect.origin.x + by, rect.origin.y + by),
        CGSize::new(
            (rect.size.width - by * 2.0).max(0.0),
            (rect.size.height - by * 2.0).max(0.0),
        ),
    )
}

/// A dot at each corner and the middle of each side.
fn handles(area: CGRect) -> CFRetained<CGMutablePath> {
    let path = CGMutablePath::new();
    let xs = [
        area.origin.x,
        area.origin.x + area.size.width / 2.0,
        area.origin.x + area.size.width,
    ];
    let ys = [
        area.origin.y,
        area.origin.y + area.size.height / 2.0,
        area.origin.y + area.size.height,
    ];
    for (i, &x) in xs.iter().enumerate() {
        for (j, &y) in ys.iter().enumerate() {
            if i == 1 && j == 1 {
                continue;
            }
            let dot = CGRect::new(
                CGPoint::new(x - HANDLE, y - HANDLE),
                CGSize::new(HANDLE * 2.0, HANDLE * 2.0),
            );
            // SAFETY: as in `add_rect`.
            unsafe { CGMutablePath::add_ellipse_in_rect(Some(&path), std::ptr::null(), dot) };
        }
    }
    path
}

/// The floating bar under the selection: its size, and the two buttons.
pub(super) struct Toolbar {
    root: Retained<NSVisualEffectView>,
    readout: Retained<NSTextField>,
    _cancel: Retained<NSButton>,
    _capture: Retained<NSButton>,
}

impl Toolbar {
    /// Build the bar, with its buttons sending `cancel` and `capture` to `target`.
    ///
    /// A button does not keep its target alive, so whoever owns the bar owns the target.
    pub(super) fn new(
        target: &AnyObject,
        cancel: Sel,
        capture: Sel,
        mtm: MainThreadMarker,
    ) -> Self {
        let cancel = button("Cancel", target, cancel, "\u{1b}", mtm);
        // Return, which also makes it the default button and draws it in the accent colour.
        let capture = button("Capture", target, capture, "\r", mtm);

        let readout = NSTextField::labelWithString(&NSString::from_str("0 × 0"), mtm);
        // SAFETY: an extern constant, read once.
        let weight = unsafe { NSFontWeightMedium };
        readout.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
            13.0, weight,
        )));
        readout.setAlignment(NSTextAlignment::Center);
        readout.sizeToFit();
        let readout_height = readout.frame().size.height;

        let tallest = cancel
            .frame()
            .size
            .height
            .max(capture.frame().size.height)
            .max(readout_height);
        let height = tallest + PADDING * 2.0;
        let middle = |h: f64| ((height - h) / 2.0).round();

        let mut x = PADDING;
        readout.setFrame(CGRect::new(
            CGPoint::new(x, middle(readout_height)),
            CGSize::new(READOUT, readout_height),
        ));
        x += READOUT + PADDING;
        for item in [&cancel, &capture] {
            let size = item.frame().size;
            item.setFrame(CGRect::new(CGPoint::new(x, middle(size.height)), size));
            x += size.width + PADDING;
        }

        let frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(x, height));
        let root = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), frame);
        root.setMaterial(NSVisualEffectMaterial::HUDWindow);
        root.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        root.setState(NSVisualEffectState::Active);
        // Dark in light mode too, as the system's own screenshot bar is: it sits over a
        // dimmed screen, where a light bar would be the brightest thing on it.
        if let Some(dark) = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }) {
            root.setAppearance(Some(&dark));
        }
        root.setWantsLayer(true);
        let layer = root.layer().expect("a view asked for a layer has a layer");
        layer.setCornerRadius(12.0);
        unsafe { layer.setCornerCurve(objc2_quartz_core::kCACornerCurveContinuous) };
        layer.setMasksToBounds(true);
        layer.setBorderWidth(0.5);
        layer.setBorderColor(Some(&host::srgb(1.0, 1.0, 1.0, 0.18)));

        root.addSubview(&readout);
        root.addSubview(&cancel);
        root.addSubview(&capture);

        Self {
            root,
            readout,
            _cancel: cancel,
            _capture: capture,
        }
    }

    pub(super) fn size(&self) -> CGSize {
        self.root.frame().size
    }

    /// Where the bar is, in the coordinates of the view it is in.
    pub(super) fn frame(&self) -> CGRect {
        self.root.frame()
    }

    /// Put the bar in `view` at `origin`, moving it there from another panel if need be.
    pub(super) fn place(&self, view: &NSView, origin: CGPoint) {
        // SAFETY: the superview is a selector panel's content view, which the panel holds
        // for as long as the bar can be in it.
        let already_there =
            unsafe { self.root.superview() }.is_some_and(|parent| std::ptr::eq(&*parent, view));
        if !already_there {
            self.root.removeFromSuperview();
            view.addSubview(&self.root);
        }
        self.root.setFrameOrigin(origin);
    }

    pub(super) fn set_readout(&self, text: &str) {
        self.readout.setStringValue(&NSString::from_str(text));
    }
}

impl Drop for Toolbar {
    fn drop(&mut self) {
        self.root.removeFromSuperview();
    }
}

fn button(
    title: &str,
    target: &AnyObject,
    action: Sel,
    key: &str,
    mtm: MainThreadMarker,
) -> Retained<NSButton> {
    // SAFETY: the target answers the action, and outlives the button. See `Toolbar::new`.
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target),
            Some(action),
            mtm,
        )
    };
    button.setControlSize(NSControlSize::Large);
    button.setKeyEquivalent(&NSString::from_str(key));
    button.sizeToFit();
    let size = button.frame().size;
    button.setFrameSize(CGSize::new(size.width.max(BUTTON), size.height));
    button
}

/// Show the pointer for what a drag would do, or the arrow over the bar.
pub(super) fn show_pointer(pointer: Option<area::Pointer>) {
    let cursor = match pointer {
        None => NSCursor::arrowCursor(),
        Some(area::Pointer::Crosshair) => NSCursor::crosshairCursor(),
        Some(area::Pointer::Hand) => NSCursor::openHandCursor(),
        Some(area::Pointer::Resize(edges)) => resize_cursor(edges),
    };
    cursor.set();
}

/// The pointer over an edge or a corner of the selection.
///
/// macOS 15 has a cursor for each edge and corner of a frame. On 14 there are only the
/// side to side and up and down ones, which 15 deprecates, and a corner keeps the
/// crosshair.
fn resize_cursor(edges: area::Edges) -> Retained<NSCursor> {
    if objc2::available!(macos = 15.0)
        && let Some(position) = frame_position(edges)
    {
        return NSCursor::frameResizeCursorFromPosition_inDirections(
            position,
            NSCursorFrameResizeDirections::All,
        );
    }
    #[allow(deprecated)]
    match (edges.left || edges.right, edges.top || edges.bottom) {
        (true, false) => NSCursor::resizeLeftRightCursor(),
        (false, true) => NSCursor::resizeUpDownCursor(),
        _ => NSCursor::crosshairCursor(),
    }
}

/// Which edge or corner of a frame some edges of the selection are.
fn frame_position(edges: area::Edges) -> Option<NSCursorFrameResizePosition> {
    use NSCursorFrameResizePosition as Position;
    Some(match (edges.left, edges.right, edges.top, edges.bottom) {
        (true, false, true, false) => Position::TopLeft,
        (false, true, true, false) => Position::TopRight,
        (true, false, false, true) => Position::BottomLeft,
        (false, true, false, true) => Position::BottomRight,
        (true, false, false, false) => Position::Left,
        (false, true, false, false) => Position::Right,
        (false, false, true, false) => Position::Top,
        (false, false, false, true) => Position::Bottom,
        _ => return None,
    })
}

/// The pointer while a selection is being carried.
pub(super) fn show_grabbing() {
    NSCursor::closedHandCursor().set();
}
