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
use objc2::runtime::ProtocolObject;
use objc2::runtime::{AnyObject, Sel};
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSEventModifierFlags, NSImage, NSMenu, NSMenuDelegate, NSMenuItem, NSStatusBar,
    NSStatusItem, NSVariableStatusItemLength,
};
use objc2_core_foundation::{CFData, CGSize};
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString};
use std::sync::OnceLock;
use std::time::Duration;

/// What the menu bar calls when the user asks for a clear.
///
/// Registered by whoever owns the daemon, which is not this crate. The menu is built
/// before the daemon exists, so the handler arrives afterwards rather than being passed
/// in at construction.
static CLEAR: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// What the menu bar calls when the user asks to quit.
///
/// Sending `terminate:` straight to the application is the obvious wiring and it is
/// wrong: it ends the process without unwinding the daemon, so the socket file survives
/// and the next start has to notice and clear it. The daemon is asked to stop instead,
/// and it exits once it has let go of the socket.
static QUIT: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// What the menu bar asks for the status line when the menu opens.
///
/// Returns a line of text and nothing more. The menu has no business knowing what a
/// session or an annotation is, and the daemon has no business knowing about menus.
static STATUS: OnceLock<Box<dyn Fn() -> String + Send + Sync>> = OnceLock::new();

/// What the menu bar asks for a newer released version, when the user wanted checking.
static UPDATE: OnceLock<Box<dyn Fn() -> Option<String> + Send + Sync>> = OnceLock::new();

/// What the menu bar asks for the state of the grounding permission.
///
/// `None` means nothing is granted. A duration means a grant is running down, and the item
/// becomes a way to take it back.
static GROUNDING: OnceLock<Box<dyn Fn() -> Option<Duration> + Send + Sync>> = OnceLock::new();

/// What the menu bar calls to take a grounding grant back.
///
/// The promise the consent prompt makes. A grant nobody can revoke is one people will not
/// give in the first place, so this is part of the prompt working rather than a nicety.
static REVOKE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// Where the live lines sit in the menu, so the delegate can find them again.
///
/// Positions rather than retained handles, because `menuNeedsUpdate:` is handed the menu
/// and nothing else. Keep these in step with `MenuBar::install`.
const TITLE_INDEX: isize = 0;
const STATUS_INDEX: isize = 1;
const PERMISSION_INDEX: isize = 2;
const GROUNDING_INDEX: isize = 3;

/// Retitle a menu item, doing nothing if the menu has been rearranged underneath us.
fn set_title(menu: &NSMenu, index: isize, title: &str) {
    // An out of range index returns nil rather than trapping.
    if let Some(item) = menu.itemAtIndex(index) {
        item.setTitle(&NSString::from_str(title));
    }
}

/// Register the action behind the Clear menu item.
///
/// Only the first call takes effect, which is what we want: the daemon is created once.
pub fn on_clear(handler: impl Fn() + Send + Sync + 'static) {
    if CLEAR.set(Box::new(handler)).is_err() {
        tracing::warn!("a clear handler was already registered");
    }
}

/// Register the action behind the Quit menu item.
pub fn on_quit(handler: impl Fn() + Send + Sync + 'static) {
    if QUIT.set(Box::new(handler)).is_err() {
        tracing::warn!("a quit handler was already registered");
    }
}

/// Register how to read and revoke the grounding permission.
///
/// Two halves of one item: it reports what is granted and, when something is, clicking it
/// takes that back.
pub fn on_grounding(
    state: impl Fn() -> Option<Duration> + Send + Sync + 'static,
    revoke: impl Fn() + Send + Sync + 'static,
) {
    if GROUNDING.set(Box::new(state)).is_err() || REVOKE.set(Box::new(revoke)).is_err() {
        tracing::warn!("a grounding handler was already registered");
    }
}

/// Register what the status line should say.
///
/// Called on the main thread each time the menu is opened, so it must be quick and must
/// not block. Reading a couple of counters is fine; taking a screenshot is not.
pub fn on_status(handler: impl Fn() -> String + Send + Sync + 'static) {
    if STATUS.set(Box::new(handler)).is_err() {
        tracing::warn!("a status handler was already registered");
    }
}

/// Register how to ask whether a newer version has been released.
///
/// Returns the version when there is one and `None` otherwise, and it is asked only as the
/// menu opens, so the answer is something already known rather than something fetched.
/// Nothing registers this unless the user asked for update checks, and an unregistered
/// handler simply leaves the title alone.
pub fn on_update_available(handler: impl Fn() -> Option<String> + Send + Sync + 'static) {
    if UPDATE.set(Box::new(handler)).is_err() {
        tracing::warn!("an update handler was already registered");
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this type has no Drop.
    #[unsafe(super(NSObject))]
    #[name = "ArinMenuActions"]
    #[thread_kind = MainThreadOnly]
    struct Actions;

    // Required by `NSMenuDelegate`, and free: `NSObject` already answers all of it.
    unsafe impl NSObjectProtocol for Actions {}

    /// Refreshes the live lines when the menu opens.
    ///
    /// A status shown once at install would be a status from startup, which is exactly
    /// when nothing has happened yet and the permission has most likely not been granted.
    unsafe impl NSMenuDelegate for Actions {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            let status = STATUS
                .get()
                .map_or_else(|| "starting up".to_owned(), |status| status());
            set_title(menu, STATUS_INDEX, &status);

            // On the name rather than in a line of its own. A menu item that appears and
            // disappears moves everything under it, and the indices the rest of this
            // delegate writes to are positional.
            let title = match UPDATE.get().and_then(|available| available()) {
                Some(version) => format!("Arin  ·  {version} available"),
                None => "Arin".to_owned(),
            };
            set_title(menu, TITLE_INDEX, &title);

            // Cheap: reads the TCC answer rather than proving it with a frame, which is
            // what `arin permissions` is for and is far too slow for a menu opening.
            let granted = crate::permission::screen_recording_granted();
            set_title(
                menu,
                PERMISSION_INDEX,
                if granted {
                    "Screen Recording: granted"
                } else {
                    "Screen Recording: needed, open Settings"
                },
            );
            if let Some(item) = menu.itemAtIndex(PERMISSION_INDEX) {
                // Only actionable when there is something to fix.
                item.setEnabled(!granted);
            }

            // The other half of the consent prompt's promise. It says a grant can be taken
            // back from here, so here has to show one and take it back.
            let grant = GROUNDING.get().and_then(|state| state());
            set_title(
                menu,
                GROUNDING_INDEX,
                &match grant {
                    Some(left) => format!(
                        "Screen access granted, {} left. Revoke",
                        remaining(left)
                    ),
                    None => "Screen access: asks each time".to_owned(),
                },
            );
            if let Some(item) = menu.itemAtIndex(GROUNDING_INDEX) {
                item.setEnabled(grant.is_some());
            }
        }
    }

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

        #[unsafe(method(revokeGrounding:))]
        fn revoke_grounding(&self, _sender: Option<&AnyObject>) {
            match REVOKE.get() {
                Some(revoke) => revoke(),
                None => tracing::warn!("revoke requested before the daemon was ready"),
            }
        }

        #[unsafe(method(openScreenRecording:))]
        fn open_screen_recording(&self, _sender: Option<&AnyObject>) {
            if !crate::permission::open_screen_recording_settings() {
                tracing::warn!("could not open System Settings");
            }
        }

        #[unsafe(method(quitArin:))]
        fn quit_arin(&self, _sender: Option<&AnyObject>) {
            match QUIT.get() {
                Some(quit) => quit(),
                // No daemon to unwind, so there is no socket to let go of and nothing to
                // wait for. Terminating directly is the right answer rather than leaving
                // the user with a Quit item that does nothing.
                None => {
                    tracing::warn!("quit requested before the daemon was ready");
                    if let Some(mtm) = MainThreadMarker::new() {
                        NSApplication::sharedApplication(mtm).terminate(None);
                    }
                }
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

        // Index 0. A label rather than a control: there is nothing to configure here.
        let title = menu_item(mtm, "Arin", None, "");
        title.setEnabled(false);
        menu.addItem(&title);

        // Index 1 and 2, both rewritten by the delegate every time the menu opens. The
        // placeholder text is only ever seen if that fails to fire.
        let status = menu_item(mtm, "starting up", None, "");
        status.setEnabled(false);
        menu.addItem(&status);

        let permission = menu_item(
            mtm,
            "Screen Recording",
            Some(sel!(openScreenRecording:)),
            "",
        );
        unsafe { permission.setTarget(Some(&actions)) };
        menu.addItem(&permission);

        // Index 3, also rewritten every time the menu opens.
        let grounding = menu_item(mtm, "Screen access", Some(sel!(revokeGrounding:)), "");
        unsafe { grounding.setTarget(Some(&actions)) };
        menu.addItem(&grounding);

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

        // Deliberately not `terminate:`. See `QUIT`.
        let quit = menu_item(mtm, "Quit Arin", Some(sel!(quitArin:)), "q");
        unsafe { quit.setTarget(Some(&actions)) };
        menu.addItem(&quit);

        // The delegate is what keeps the two lines above from being a snapshot of
        // startup, which is the one moment when nothing has happened and the permission
        // is most likely still missing.
        menu.setDelegate(Some(ProtocolObject::from_ref(&*actions)));

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

/// How long a grant has left, in words rather than seconds.
///
/// A menu item is read at a glance, and "3512s" is not something anyone converts in their
/// head while deciding whether to revoke something.
fn remaining(left: Duration) -> String {
    let seconds = left.as_secs();
    if seconds >= 90 {
        format!("{} min", seconds.div_ceil(60))
    } else {
        format!("{seconds} sec")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grant_is_described_in_units_somebody_can_read_at_a_glance() {
        assert_eq!(remaining(Duration::from_secs(3600)), "60 min");
        assert_eq!(remaining(Duration::from_secs(90)), "2 min");
        assert_eq!(remaining(Duration::from_secs(89)), "89 sec");
        assert_eq!(remaining(Duration::from_secs(1)), "1 sec");
    }
}
