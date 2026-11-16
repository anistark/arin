//! Screenshots of an area of the screen, taken by the person from the menu bar.
//!
//! **Screenshot** opens a selector over every display: a clear rectangle with handles,
//! the rest of the screen dimmed, and a bar under the rectangle with its size and two
//! buttons. The rectangle is moved by dragging its middle, resized by its edges and
//! corners, drawn afresh by dragging anywhere outside it, and carried to another display
//! by dragging it there. **Capture**, or Return, takes the picture. **Cancel**, Escape, or
//! a right click puts the selector away.
//!
//! # What this is not
//!
//! Not something a client can do. Nothing on the wire starts it and nothing on the wire
//! receives the image, so it is the person using Arin's Screen Recording grant on their
//! own screen, never a way for an agent to read the screen around the grounding gate. It
//! lives entirely in this crate, like the marker, and the daemon never learns it ran.
//!
//! # What the picture shows
//!
//! What was on the screen, Arin's own marks, orb and marker ink included, since those are
//! on the screen the person is taking a picture of. The selector is left out. Its panels
//! are closed before the capture is asked for, and ScreenCaptureKit is told to exclude
//! them as well, in case the window server has not caught up.
//!
//! One display per picture. A capture is made from one display's pixels, and two displays
//! can differ in scale, so a selection belongs to the display it is on and moves between
//! them whole.

mod area;
mod save;
mod selector;

use crate::display::{Screen, screens};
use crate::host;
use area::Grip;
use arin_protocol::DisplayId;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSAlert, NSAlertStyle, NSApplication, NSEvent};
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_foundation::{NSObject, NSString};
use objc2_quartz_core::CATransaction;
use selector::{SelectorPanel, Toolbar};
use std::cell::{Cell, RefCell};

thread_local! {
    /// The selector, while it is open. Main thread only.
    static SHOOTER: RefCell<Option<Shooter>> = const { RefCell::new(None) };

    /// Where the last selection was, so the selector opens where it was left.
    ///
    /// Kept for the life of the process only. A rectangle remembered across restarts would
    /// be one remembered across display arrangements too.
    static LAST: Cell<Option<(DisplayId, CGRect)>> = const { Cell::new(None) };
}

/// What the mouse did on a selector panel, in that panel's own coordinates.
pub(crate) enum Input {
    Press(CGPoint),
    Drag(CGPoint),
    Release(CGPoint),
    Hover(CGPoint),
    Cancel,
}

/// What a press took hold of.
#[derive(Debug, Clone, Copy)]
enum Hold {
    Move,
    Resize(area::Edges),
    /// A new selection, from a point on a display.
    Draw {
        display: DisplayId,
        anchor: CGPoint,
    },
}

/// A press and where it has got to.
#[derive(Debug, Clone, Copy)]
struct Drag {
    hold: Hold,
    /// Where the press was, in AppKit's global space.
    from: CGPoint,
    /// The selection as it was when the press happened.
    start_on: DisplayId,
    start: CGRect,
    /// Whether the pointer has gone far enough for this to be a drag.
    moving: bool,
}

/// The selector, open.
///
/// Fields drop in order: the bar leaves the panel it is in before the panels close, and
/// the buttons' target outlives the buttons.
struct Shooter {
    toolbar: Toolbar,
    panels: Vec<SelectorPanel>,
    /// The display the selection is on.
    on: DisplayId,
    /// The selection, in that display's own points.
    area: CGRect,
    drag: Option<Drag>,
    /// What the bar's buttons send their actions to. A button does not keep its target
    /// alive, so this does.
    _actions: Retained<Actions>,
}

impl Shooter {
    fn panel(&self, display: DisplayId) -> Option<&SelectorPanel> {
        self.panels.iter().find(|p| p.screen().info.id == display)
    }

    fn screen(&self, display: DisplayId) -> Option<Screen> {
        self.panel(display).map(SelectorPanel::screen)
    }

    /// The display a point in global space is on.
    fn screen_at(&self, global: CGPoint) -> Option<Screen> {
        self.panels
            .iter()
            .map(SelectorPanel::screen)
            .find(|screen| screen.contains(global))
    }

    /// Move the selection, redraw every panel, and bring the bar along.
    fn select(&mut self, display: DisplayId, area: CGRect) {
        self.on = display;
        self.area = area;
        LAST.set(Some((display, area)));

        for panel in &self.panels {
            let here = panel.screen().info.id == display;
            panel.show(here.then_some(area));
        }
        if let Some(panel) = self.panel(display) {
            self.toolbar.set_readout(&area::describe(area.size));
            let origin = area::toolbar_origin(area, self.toolbar.size(), panel.bounds());
            self.toolbar.place(panel.content(), origin);
        }
    }

    /// Hand the keyboard to the panel the bar is on.
    ///
    /// The bar's buttons answer Return and Escape only in the key window. Done when a drag
    /// ends rather than as the selection crosses a display, so the window the drag started
    /// in keeps it until the button comes up.
    fn take_keyboard(&self) {
        if let Some(panel) = self.panel(self.on) {
            panel.make_key();
        }
    }

    fn input(&mut self, display: DisplayId, input: Input) {
        match input {
            Input::Press(at) => self.press(display, at),
            Input::Drag(at) => self.drag(display, at),
            Input::Release(at) => {
                self.drag = None;
                self.take_keyboard();
                self.hover(display, at);
            }
            Input::Hover(at) => self.hover(display, at),
            Input::Cancel => later(cancel),
        }
    }

    fn press(&mut self, display: DisplayId, at: CGPoint) {
        let Some(screen) = self.screen(display) else {
            return;
        };
        // The bar's buttons take their own clicks. One on the space around them would
        // otherwise reach here and start drawing a selection under the bar.
        if display == self.on && self.toolbar.frame().contains(at) {
            return;
        }
        let grip = (display == self.on)
            .then(|| area::grip_at(self.area, at))
            .flatten();
        let hold = match grip {
            Some(Grip::Body) => Hold::Move,
            Some(Grip::Edges(edges)) => Hold::Resize(edges),
            None => Hold::Draw {
                display,
                anchor: at,
            },
        };
        if matches!(hold, Hold::Move) {
            selector::show_grabbing();
        }
        self.drag = Some(Drag {
            hold,
            from: screen.layer_to_global(at),
            start_on: self.on,
            start: self.area,
            moving: false,
        });
    }

    fn drag(&mut self, display: DisplayId, at: CGPoint) {
        let (Some(mut drag), Some(screen)) = (self.drag, self.screen(display)) else {
            return;
        };
        let global = screen.layer_to_global(at);
        let by = CGPoint::new(global.x - drag.from.x, global.y - drag.from.y);
        if !drag.moving {
            // A press outside the selection replaces it, so a click that wobbles a point
            // or two must not. Moving and resizing have no such cost and start at once.
            if matches!(drag.hold, Hold::Draw { .. }) && by.x.hypot(by.y) < area::SLOP {
                return;
            }
            drag.moving = true;
            self.drag = Some(drag);
        }

        match drag.hold {
            Hold::Move => {
                let Some(start) = self.screen(drag.start_on) else {
                    return;
                };
                let origin = start.layer_to_global(drag.start.origin);
                let carried = CGPoint::new(origin.x + by.x, origin.y + by.y);
                // The display under the pointer takes the selection, which is how it is
                // carried from one screen to another.
                let target = self.screen_at(global).unwrap_or(start);
                let bounds = CGRect::new(CGPoint::new(0.0, 0.0), target.frame.size);
                let area = area::fitted(
                    CGRect::new(target.global_to_layer(carried), drag.start.size),
                    bounds,
                );
                self.select(target.info.id, area);
            }
            Hold::Resize(edges) => {
                let Some(bounds) = self.panel(drag.start_on).map(SelectorPanel::bounds) else {
                    return;
                };
                let area = area::resized(drag.start, edges, by, bounds);
                self.select(drag.start_on, area);
            }
            Hold::Draw { display, anchor } => {
                let Some(panel) = self.panel(display) else {
                    return;
                };
                let at = panel.screen().global_to_layer(global);
                let area = area::spanning(anchor, at, panel.bounds());
                self.select(display, area);
            }
        }
    }

    fn hover(&self, display: DisplayId, at: CGPoint) {
        if self.drag.is_some() {
            return;
        }
        if display == self.on && self.toolbar.frame().contains(at) {
            selector::show_pointer(None);
            return;
        }
        let grip = (display == self.on)
            .then(|| area::grip_at(self.area, at))
            .flatten();
        selector::show_pointer(Some(Grip::pointer(grip)));
    }

    /// The window ids of every selector panel, for ScreenCaptureKit to leave out.
    fn window_ids(&self) -> Vec<u32> {
        self.panels
            .iter()
            .filter_map(SelectorPanel::window_id)
            .collect()
    }
}

/// `CGRect::contains`, which the bindings do not have.
trait Contains {
    fn contains(&self, point: CGPoint) -> bool;
}

impl Contains for CGRect {
    fn contains(&self, point: CGPoint) -> bool {
        point.x >= self.origin.x
            && point.x < self.origin.x + self.size.width
            && point.y >= self.origin.y
            && point.y < self.origin.y + self.size.height
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this type has no Drop.
    #[unsafe(super(NSObject))]
    #[name = "ArinScreenshotActions"]
    #[thread_kind = MainThreadOnly]
    struct Actions;

    /// The bar's buttons.
    ///
    /// Both hand their work to the next turn of the run loop. Each tears the selector
    /// down, and doing that from inside the click would close the window whose button is
    /// still handling it.
    impl Actions {
        #[unsafe(method(captureArea:))]
        fn capture_area(&self, _sender: Option<&AnyObject>) {
            later(capture);
        }

        #[unsafe(method(cancelArea:))]
        fn cancel_area(&self, _sender: Option<&AnyObject>) {
            later(cancel);
        }
    }
);

impl Actions {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

/// Run something on the main thread on a later turn of the run loop.
fn later(work: fn(MainThreadMarker)) {
    DispatchQueue::main().exec_async(move || {
        if let Some(mtm) = MainThreadMarker::new() {
            work(mtm);
        }
    });
}

/// Hand a panel's mouse to the selection.
///
/// Borrowed rather than taken, so an event that arrives while the selector is being
/// rearranged is dropped rather than panicking. The next one carries the pointer anyway.
fn input(display: DisplayId, input: Input, _mtm: MainThreadMarker) {
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    SHOOTER.with(|slot| match slot.try_borrow_mut() {
        Ok(mut shooter) => {
            if let Some(shooter) = shooter.as_mut() {
                shooter.input(display, input);
            }
        }
        Err(_) => tracing::debug!("screenshot input arrived while the selector was busy"),
    });
    CATransaction::commit();
}

/// Open the screenshot selector.
///
/// From any thread. The work happens on a later turn of the main run loop: the menu calls
/// this while it is still tracking, and a window taking the keyboard from inside menu
/// tracking comes up with the menu's own event loop still on the stack.
pub(crate) fn begin() {
    later(open);
}

/// Whether the selector is open. Asked by the menu, on the main thread.
pub(crate) fn is_open(_mtm: MainThreadMarker) -> bool {
    SHOOTER.with(|slot| slot.try_borrow().map_or(true, |shooter| shooter.is_some()))
}

/// Put the selector away if the displays change underneath it.
///
/// Its panels were sized for the old arrangement, and a selection on a display that went
/// away has nothing left to be a picture of. Opening it again is one click.
pub(crate) fn displays_changed(mtm: MainThreadMarker) {
    if is_open(mtm) {
        tracing::info!("displays changed, screenshot selector closed");
        later(cancel);
    }
}

fn open(mtm: MainThreadMarker) {
    if is_open(mtm) {
        return;
    }
    let attached = screens(mtm);
    let Some(first) = attached.first().copied() else {
        tracing::warn!("no display to take a screenshot of");
        return;
    };

    // Where it was left, if that display is still here. Otherwise the display the pointer
    // is on, which is the one the person is looking at.
    let bounds_of = |screen: &Screen| CGRect::new(CGPoint::new(0.0, 0.0), screen.frame.size);
    let remembered = LAST.get().and_then(|(display, area)| {
        let screen = attached.iter().find(|s| s.info.id == display)?;
        Some((display, area::fitted(area, bounds_of(screen))))
    });
    let (on, area) = remembered.unwrap_or_else(|| {
        let pointer = NSEvent::mouseLocation();
        let screen = attached
            .iter()
            .find(|s| s.contains(pointer))
            .copied()
            .unwrap_or(first);
        (screen.info.id, area::initial(bounds_of(&screen)))
    });

    let actions = Actions::new(mtm);
    let toolbar = Toolbar::new(&actions, sel!(cancelArea:), sel!(captureArea:), mtm);
    let panels = attached
        .into_iter()
        .map(|screen| SelectorPanel::new(screen, mtm))
        .collect();
    let mut shooter = Shooter {
        toolbar,
        panels,
        on,
        area,
        drag: None,
        _actions: actions,
    };

    CATransaction::begin();
    CATransaction::setDisableActions(true);
    shooter.select(on, area);
    CATransaction::commit();
    shooter.take_keyboard();
    selector::show_pointer(Some(area::Pointer::Crosshair));

    SHOOTER.set(Some(shooter));
    tracing::info!(display = %on, "screenshot selector open");
}

/// Take the selector off the screen and give the pointer back.
fn close(mtm: MainThreadMarker) -> Option<Shooter> {
    let shooter = SHOOTER.with(|slot| slot.try_borrow_mut().ok()?.take())?;
    selector::show_pointer(None);
    // The marker's tip, if the marker is still on underneath.
    host::show_marker_cursor(mtm);
    Some(shooter)
}

fn cancel(mtm: MainThreadMarker) {
    if close(mtm).is_some() {
        tracing::info!("screenshot cancelled");
    }
}

/// Take the picture of the selection and write it out.
///
/// The selector goes first, then the capture runs on a thread of its own. Capture blocks
/// until ScreenCaptureKit answers, and its completion handlers need the main run loop
/// free, so it can never run here.
fn capture(mtm: MainThreadMarker) {
    let Some(shooter) = close(mtm) else {
        return;
    };
    let Some(screen) = shooter.screen(shooter.on) else {
        return;
    };
    let area = shooter.area;
    let leave_out = shooter.window_ids();
    let stamp = save::timestamp();
    // Closes every panel.
    drop(shooter);

    let display = screen.info.id;
    let scale = screen.info.scale;
    let rect = area::capture_rect(area, screen.frame.size.height);
    let spawned = std::thread::Builder::new()
        .name("arin-screenshot".into())
        .spawn(move || {
            let saved = crate::capture::capture_area(display, rect, scale, &leave_out)
                .map_err(|e| e.to_string())
                .and_then(|image| save::write(&image, rect.size, &stamp));
            DispatchQueue::main().exec_async(move || {
                let Some(mtm) = MainThreadMarker::new() else {
                    return;
                };
                match saved {
                    Ok(path) => {
                        tracing::info!(path = %path.display(), "screenshot saved");
                        host::flash(display, area);
                    }
                    Err(message) => {
                        tracing::warn!(error = %message, "screenshot failed");
                        report_failure(mtm, &message);
                    }
                }
            });
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "could not start the screenshot thread");
        report_failure(mtm, &e.to_string());
    }
}

/// Say that a screenshot was not taken, and why.
///
/// An alert rather than a log line. The person clicked Capture and is waiting for a file,
/// and the most likely reason there is none is a missing permission they can fix.
fn report_failure(mtm: MainThreadMarker, message: &str) {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str("The screenshot was not taken"));
    alert.setInformativeText(&NSString::from_str(message));
    alert.setAlertStyle(NSAlertStyle::Warning);
    // As the other alerts do: with no windows of its own, Arin's alert can come up behind
    // whatever the person is looking at.
    NSApplication::sharedApplication(mtm).activate();
    alert.runModal();
}
