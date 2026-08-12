//! The System Settings pane that holds the switch.

use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSURL};

/// The Screen Recording list in System Settings.
const SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

/// Open the Screen Recording list in System Settings.
///
/// Takes focus away from whatever the user is doing, which is why almost nothing calls this
/// on its own initiative. See the `flow` module for what is allowed to.
pub fn open_screen_recording_settings() -> bool {
    let Some(url) = NSURL::URLWithString(&NSString::from_str(SETTINGS_URL)) else {
        return false;
    };
    NSWorkspace::sharedWorkspace().openURL(&url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_link_points_at_screen_recording() {
        // A wrong pane here drops the user in Settings with no idea what to switch on.
        assert!(SETTINGS_URL.starts_with("x-apple.systempreferences:"));
        assert!(SETTINGS_URL.ends_with("Privacy_ScreenCapture"));
    }
}
