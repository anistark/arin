//! The global hotkey that clears every annotation.
//!
//! # Why this does not need Accessibility
//!
//! Screen Recording is the only permission Arin asks for, and a global hotkey is the
//! obvious way to break that. There are two ways to listen for one on macOS:
//!
//! - `RegisterEventHotKey`, the Carbon API, which asks the window server to deliver one
//!   specific chord and needs no permission at all.
//! - `CGEventTapCreate`, which sees every keystroke on the machine and needs
//!   Accessibility, the permission Arin promises never to want.
//!
//! `global-hotkey` uses the first for ordinary chords and only reaches for the second
//! when a *media* key is registered. So the chord is checked before it is registered, and
//! a media key is refused rather than silently escalating what Arin asks of the user.
//!
//! The check is not decoration. Someone making the hotkey configurable later will pass
//! whatever the user typed straight into `register`, and this is what stops that from
//! quietly turning Arin into a keylogger-shaped process.

use anyhow::{Context, Result, bail};
use arin_core::Daemon;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager};
use std::sync::Arc;

/// The chord that clears everything.
///
/// Not configurable yet. When it becomes so, it goes through [`refuse_media_keys`].
fn clear_chord() -> HotKey {
    HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyK)
}

/// How the chord reads in a menu or a log line.
pub const CLEAR_CHORD_LABEL: &str = "Cmd+Shift+K";

/// Media keys, which are the ones that would force an event tap.
///
/// Listed rather than pattern matched because `Code` is non exhaustive and a wrong guess
/// here has a permission consequence.
const MEDIA_KEYS: &[Code] = &[
    Code::AudioVolumeUp,
    Code::AudioVolumeDown,
    Code::AudioVolumeMute,
    Code::MediaPlayPause,
    Code::MediaStop,
    Code::MediaTrackNext,
    Code::MediaTrackPrevious,
];

/// Refuse a chord that would make the hotkey library open an event tap.
///
/// An event tap needs Accessibility. Arin does not ask for Accessibility. A chord that
/// would require it is a bug in whatever chose the chord, not a prompt to show the user.
fn refuse_media_keys(hotkey: &HotKey) -> Result<()> {
    if MEDIA_KEYS.contains(&hotkey.key) {
        bail!(
            "refusing to bind a media key: listening for one needs an event tap, and \
             that needs the Accessibility permission Arin does not ask for"
        );
    }
    Ok(())
}

/// Listen for the clear chord for as long as the daemon runs.
///
/// Returns the manager, which has to stay alive: dropping it unregisters the chord.
pub fn listen(daemon: Arc<Daemon>) -> Result<GlobalHotKeyManager> {
    let chord = clear_chord();
    refuse_media_keys(&chord)?;

    let manager = GlobalHotKeyManager::new().context("could not start the hotkey listener")?;
    manager
        .register(chord)
        .context("could not register the clear hotkey, another app may already hold it")?;

    let receiver = GlobalHotKeyEvent::receiver().clone();
    std::thread::Builder::new()
        .name("arin-hotkey".into())
        .spawn(move || {
            for event in receiver.iter() {
                // Both press and release arrive. Acting on one of them keeps a single
                // keypress from clearing twice.
                if event.state != global_hotkey::HotKeyState::Pressed {
                    continue;
                }
                let cleared = daemon.clear_everything();
                if !cleared.is_empty() {
                    tracing::info!(count = cleared.len(), "cleared by hotkey");
                }
            }
        })
        .context("could not start the hotkey thread")?;

    tracing::info!(chord = CLEAR_CHORD_LABEL, "clear hotkey registered");
    Ok(manager)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clear_chord_does_not_need_accessibility() {
        // The whole permission story rests on this staying true.
        refuse_media_keys(&clear_chord()).expect("the built in chord must not need a tap");
    }

    #[test]
    fn a_media_key_is_refused() {
        let media = HotKey::new(None, Code::MediaPlayPause);
        assert!(
            refuse_media_keys(&media).is_err(),
            "binding a media key would open an event tap and require Accessibility"
        );
    }

    #[test]
    fn ordinary_chords_are_allowed() {
        for code in [Code::KeyA, Code::Escape, Code::F5, Code::Digit1] {
            let chord = HotKey::new(Some(Modifiers::SUPER), code);
            assert!(refuse_media_keys(&chord).is_ok(), "{code:?} should be fine");
        }
    }
}
