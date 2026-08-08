//! The About box.
//!
//! An `NSAlert`, for the same reason `approval` is one: Arin is `LSUIElement` and owns no
//! windows, so a panel would need a window controller, a nib, and a way to be brought back
//! to the front. An alert is a modal a menu bar app already knows how to raise.
//!
//! # Why the links are buttons
//!
//! `NSAlert` shows plain text, and a clickable URL inside one means an accessory view
//! holding an `NSTextView` with an attributed string, sized by hand. Buttons carry the same
//! two destinations with none of that, and they say where they go rather than showing a URL
//! somebody has to read. Clicking one dismisses the box, which is why neither is the default:
//! return closes it, and a link is a deliberate click.
//!
//! The box does not re-present itself after a link opens. It could, and it would put Arin
//! back in front of the browser the user just asked for, which is worse than making them
//! open the menu twice.

use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSAlert, NSAlertSecondButtonReturn, NSAlertStyle, NSAlertThirdButtonReturn, NSApplication,
    NSImage, NSWorkspace,
};
use objc2_core_foundation::CGSize;
use objc2_foundation::{NSData, NSString, NSURL};

/// The phoenix, compiled in rather than read off disk.
///
/// `NSAlert` defaults to the application icon, which is the phoenix inside `Arin.app` and a
/// generic one everywhere else, because a bare binary has no bundle to take an icon from.
/// That covers `cargo run` during development and anything else that runs the executable
/// directly, and a box introducing Arin is the wrong place to show macOS's stand-in.
///
/// `assets/logo.png` stays the single source it already is for `AppIcon.icns`: this reads
/// the same 1024px file `bundle.sh` and `package.nix` build the icns from, rather than
/// adding a second copy at a second size for somebody to forget.
const LOGO: &[u8] = include_bytes!("../../../assets/logo.png");

/// The size `NSAlert` gives its icon. Set on the image, so the 1024px source is drawn down
/// to it rather than handed over at full size.
const ICON_POINTS: f64 = 64.0;

/// The X account, and the Discord invite.
///
/// Both are also in the documentation site's footer. They are written out rather than
/// derived from anything, because there is nothing to derive them from: the crate manifest
/// knows the repository and not where the people are.
const X_URL: &str = "https://x.com/kranirudha";
const DISCORD_URL: &str = "https://discord.gg/5YrbwNRGaE";

/// What Arin is, for somebody who opened the menu to find out.
///
/// The second paragraph is the product boundary, in the words a user needs rather than the
/// words `just draw-only` enforces it in. Somebody deciding whether to leave a menu bar app
/// with Screen Recording permission running is asking exactly that question, and the answer
/// should be in the one place they go looking.
const BODY: &str = "An annotation layer any agent can draw on.\n\n\
     Arin gives an agent a pointer, highlights, and captions on your screen, so it can show \
     you what it means instead of describing it.\n\n\
     It draws, and does nothing else. It never clicks, types, or scrolls, and the overlay \
     has no buttons in it to click by accident.\n\n\
     Clear the marks from this menu, or with \u{2318}\u{21e7}K from anywhere.";

/// The name and the running version, which is the line somebody quotes in a bug report.
fn title() -> String {
    format!("Arin {}", env!("CARGO_PKG_VERSION"))
}

/// Put the box on screen, and follow whichever link was clicked.
///
/// Main thread only, which is what the marker proves.
pub fn present(mtm: MainThreadMarker) {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(&title()));
    alert.setInformativeText(&NSString::from_str(BODY));
    // Informational, unlike the consent prompt: nothing here is being decided.
    alert.setAlertStyle(NSAlertStyle::Informational);

    // Leaves the default in place if the image will not decode, which is a stand-in icon
    // rather than no box.
    if let Some(logo) = logo() {
        // SAFETY: the alert takes its own reference, and the image outlives the call.
        unsafe { alert.setIcon(Some(&logo)) };
    }

    // First is the default and the rightmost, so return dismisses.
    alert.addButtonWithTitle(&NSString::from_str("Close"));
    alert.addButtonWithTitle(&NSString::from_str("Follow on X"));
    alert.addButtonWithTitle(&NSString::from_str("Join Discord"));

    // Same reason the consent prompt does it: with no windows of its own, Arin's alert can
    // come up behind whatever the user is looking at.
    NSApplication::sharedApplication(mtm).activate();

    match alert.runModal() {
        r if r == NSAlertSecondButtonReturn => open(X_URL),
        r if r == NSAlertThirdButtonReturn => open(DISCORD_URL),
        // Close, escape, or anything else.
        _ => {}
    }
}

/// The embedded logo, at the size the alert draws it.
fn logo() -> Option<Retained<NSImage>> {
    let data = NSData::with_bytes(LOGO);
    let image = NSImage::initWithData(NSImage::alloc(), &data)?;
    image.setSize(CGSize::new(ICON_POINTS, ICON_POINTS));
    Some(image)
}

/// Hand a URL to whatever the user opens links with.
fn open(url: &str) {
    let Some(parsed) = NSURL::URLWithString(&NSString::from_str(url)) else {
        tracing::warn!("{url} is not a URL");
        return;
    };
    if !NSWorkspace::sharedWorkspace().openURL(&parsed) {
        tracing::warn!("could not open {url}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The box is the one place a user is told what Arin will and will not do to their
    /// machine, and they are reading it while deciding whether to leave something with
    /// Screen Recording permission running. `just draw-only` keeps the code honest about
    /// this; nothing but a test keeps the sentence from being edited away.
    #[test]
    fn the_box_states_the_draw_only_rule() {
        assert!(BODY.contains("never clicks"), "got {BODY}");
        assert!(BODY.contains("does nothing else"), "got {BODY}");
    }

    /// A version nobody can read is a bug report nobody can place.
    #[test]
    fn the_title_carries_the_running_version() {
        let title = title();
        assert!(title.starts_with("Arin "), "got {title}");
        assert!(
            title.contains(env!("CARGO_PKG_VERSION")),
            "got {title}, want {}",
            env!("CARGO_PKG_VERSION")
        );
    }

    /// A broken path fails the build, so what this catches is the asset being replaced by
    /// something `NSImage` will not decode. The failure would otherwise be silent: the box
    /// falls back to the system icon, which is the thing this was added to stop showing.
    #[test]
    fn the_embedded_logo_is_a_png() {
        assert_eq!(
            &LOGO[..8],
            b"\x89PNG\r\n\x1a\n",
            "assets/logo.png is not a PNG"
        );
    }

    /// These ship to users inside a binary, where a typo cannot be fixed by editing a page.
    #[test]
    fn the_links_go_where_they_say() {
        assert!(X_URL.starts_with("https://x.com/"), "got {X_URL}");
        assert!(
            DISCORD_URL.starts_with("https://discord.gg/"),
            "got {DISCORD_URL}"
        );
    }
}
