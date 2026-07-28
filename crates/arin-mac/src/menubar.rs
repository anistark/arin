//! The menu bar item.
//!
//! The clear affordance lives here and on the global hotkey, never in the overlay. The
//! overlay is click through by design, so it cannot hold a button, and a person who
//! wants the marks gone needs somewhere to go that is not the agent that drew them.
//!
//! # The icon
//!
//! Drawn rather than loaded. It is the orb primitive with its features disabled: no
//! embers, a tighter halo, and no colour, because a template image is black plus alpha
//! and the system tints it. A blue orb in the menu bar looks wrong in dark mode, and an
//! image file would be one more thing to keep in step with the palette.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSEventModifierFlags, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_core_foundation::{CFData, CGSize};
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo,
};
use objc2_foundation::{NSObject, NSString};
use std::sync::OnceLock;

/// What the menu bar calls when the user asks for a clear.
///
/// Registered by whoever owns the daemon, which is not this crate. The menu is built
/// before the daemon exists, so the handler arrives afterwards rather than being passed
/// in at construction.
static CLEAR: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// Register the action behind the Clear menu item.
///
/// Only the first call takes effect, which is what we want: the daemon is created once.
pub fn on_clear(handler: impl Fn() + Send + Sync + 'static) {
    if CLEAR.set(Box::new(handler)).is_err() {
        tracing::warn!("a clear handler was already registered");
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this type has no Drop.
    #[unsafe(super(NSObject))]
    #[name = "ArinMenuActions"]
    #[thread_kind = MainThreadOnly]
    struct Actions;

    impl Actions {
        #[unsafe(method(clearAnnotations:))]
        fn clear_annotations(&self, _sender: Option<&AnyObject>) {
            match CLEAR.get() {
                Some(clear) => clear(),
                // The menu is built before the daemon, so this is reachable only if the
                // daemon failed to start. Saying so beats a menu item that does nothing.
                None => tracing::warn!("clear requested before the daemon was ready"),
            }
        }
    }
);

impl Actions {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

/// The menu bar item, held for as long as the daemon runs.
///
/// Dropping this takes the icon out of the menu bar, so it lives as long as the overlay.
pub struct MenuBar {
    _item: Retained<NSStatusItem>,
    _actions: Retained<Actions>,
}

impl MenuBar {
    /// Put Arin in the menu bar.
    pub fn install(mtm: MainThreadMarker) -> Self {
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);

        if let Some(button) = item.button(mtm) {
            if let Some(image) = template_icon() {
                image.setTemplate(true);
                button.setImage(Some(&image));
            }
            button.setToolTip(Some(&NSString::from_str("Arin")));
        }

        let actions = Actions::new(mtm);
        let menu = NSMenu::new(mtm);

        // A label rather than a control. There is nothing to configure here yet, and a
        // menu that only clears should say what it belongs to.
        let title = menu_item(mtm, "Arin", None, "");
        title.setEnabled(false);
        menu.addItem(&title);
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        let clear = menu_item(mtm, "Clear annotations", Some(sel!(clearAnnotations:)), "k");
        // Shown so the menu teaches the global chord. It will not fire from here: the
        // activation policy keeps Arin from ever being frontmost, which is the whole
        // reason the clear is also a global hotkey.
        clear.setKeyEquivalentModifierMask(
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        );
        unsafe { clear.setTarget(Some(&actions)) };
        menu.addItem(&clear);

        menu.addItem(&NSMenuItem::separatorItem(mtm));

        // Quit goes to the application itself, which already knows how to stop.
        let quit = menu_item(mtm, "Quit Arin", Some(sel!(terminate:)), "q");
        unsafe { quit.setTarget(Some(&NSApplication::sharedApplication(mtm))) };
        menu.addItem(&quit);

        item.setMenu(Some(&menu));
        tracing::info!("menu bar item installed");

        Self {
            _item: item,
            _actions: actions,
        }
    }
}

fn menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    key: &str,
) -> Retained<NSMenuItem> {
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    }
}

/// The orb primitive at menu bar size, monochrome, with its features disabled.
///
/// Black with a soft alpha edge. A template image carries no colour of its own: the
/// system tints it to match the menu bar, which is what makes it look right in both
/// light and dark mode.
fn template_icon() -> Option<Retained<NSImage>> {
    const SIZE: usize = 36;
    const POINTS: f64 = 18.0;
    let centre = (SIZE as f64 - 1.0) / 2.0;
    // Tighter than the on screen orb: below the featured size the halo pulls in, which
    // at this scale is the difference between a dot and a smudge.
    let radius = SIZE as f64 * 0.36;

    let mut pixels = vec![0u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f64 - centre;
            let dy = y as f64 - centre;
            let distance = (dx * dx + dy * dy).sqrt() / radius;
            // Solid core with a short falloff, rather than the wide halo the full size
            // orb carries.
            let alpha = if distance <= 0.62 {
                1.0
            } else {
                ((1.0 - distance) / 0.38).clamp(0.0, 1.0)
            };
            let idx = (y * SIZE + x) * 4;
            // Premultiplied black: only the alpha channel carries the shape.
            pixels[idx + 3] = (alpha * 255.0) as u8;
        }
    }

    // SAFETY: the pointer and length describe `pixels`, which CFData copies out of.
    let data = unsafe { CFData::new(None, pixels.as_ptr(), pixels.len() as isize) }?;
    let provider = CGDataProvider::with_cf_data(Some(&data))?;
    let space = CGColorSpace::new_device_rgb()?;
    // SAFETY: the dimensions, stride, and bitmap info describe the buffer above.
    let cg = unsafe {
        CGImage::new(
            SIZE,
            SIZE,
            8,
            32,
            SIZE * 4,
            Some(&space),
            CGBitmapInfo(CGImageAlphaInfo::PremultipliedLast.0),
            Some(&provider),
            std::ptr::null(),
            true,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }?;

    // Sized in points rather than pixels, so the image is drawn at 2x on a Retina panel
    // instead of being scaled up from a smaller one.
    Some(NSImage::initWithCGImage_size(
        NSImage::alloc(),
        &cg,
        CGSize::new(POINTS, POINTS),
    ))
}
