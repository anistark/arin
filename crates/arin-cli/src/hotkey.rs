//! The global hotkeys: one clears every annotation, one switches the marker.
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
//! when a *media* key is registered. So each chord is checked before it is registered,
//! and a media key is refused rather than silently escalating what Arin asks of the user.
//!
//! The check is not decoration. Someone making the hotkeys configurable later will pass
//! whatever the user typed straight into `register`, and this is what stops that from
//! quietly turning Arin into a keylogger-shaped process.
//!
//! # Why the marker has a chord at all
//!
//! While the marker is on, the overlay takes every click, so the menu bar is the only
//! thing left to click on and the keyboard is the only other way out. The chord is that
//! way out, and it is the same one the menu item shows.

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

/// The chord that switches the marker on and off.
fn marker_chord() -> HotKey {
    HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyM)
}

/// How the clear chord reads in a menu or a log line.
pub const CLEAR_CHORD_LABEL: &str = "Cmd+Shift+K";

/// How the marker chord reads in a menu or a log line.
pub const MARKER_CHORD_LABEL: &str = "Cmd+Shift+M";

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

/// Listen for the chords for as long as the daemon runs.
///
/// Returns the manager, which has to stay alive: dropping it unregisters the chords.
///
/// Each chord is bound on its own. Another app holding one of them, or a second Arin
/// holding both, costs only what it holds, and both have the menu bar as a way in anyway.
/// It is an error only when neither could be bound, which is what the caller reports.
pub fn listen(daemon: Arc<Daemon>) -> Result<GlobalHotKeyManager> {
    let clear = clear_chord();
    let marker = marker_chord();
    refuse_media_keys(&clear)?;
    refuse_media_keys(&marker)?;

    let manager = GlobalHotKeyManager::new().context("could not start the hotkey listener")?;
    let clear = bind(&manager, clear, CLEAR_CHORD_LABEL);
    let marker = bind(&manager, marker, MARKER_CHORD_LABEL);
    if clear.is_none() && marker.is_none() {
        bail!("could not register either hotkey, another app may already hold them");
    }

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
                if Some(event.id) == clear {
                    // The person's own strokes go with the agent's marks, as they do
                    // from the menu.
                    arin_mac::clear_marker();
                    let cleared = daemon.clear_everything();
                    if !cleared.is_empty() {
                        tracing::info!(count = cleared.len(), "cleared by hotkey");
                    }
                } else if Some(event.id) == marker {
                    arin_mac::toggle_marker();
                }
            }
        })
        .context("could not start the hotkey thread")?;

    tracing::info!(
        clear = clear.map(|_| CLEAR_CHORD_LABEL).unwrap_or("unavailable"),
        marker = marker.map(|_| MARKER_CHORD_LABEL).unwrap_or("unavailable"),
        "hotkeys registered"
    );
    Ok(manager)
}

/// Bind one chord, and say so if it could not be.
fn bind(manager: &GlobalHotKeyManager, chord: HotKey, label: &str) -> Option<u32> {
    match manager.register(chord) {
        Ok(()) => Some(chord.id()),
        Err(e) => {
            tracing::warn!(
                error = %e,
                chord = label,
                "hotkey unavailable, another app may already hold it. The menu bar still works"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_chords_do_not_need_accessibility() {
        // The whole permission story rests on this staying true.
        refuse_media_keys(&clear_chord()).expect("the clear chord must not need a tap");
        refuse_media_keys(&marker_chord()).expect("the marker chord must not need a tap");
    }

    #[test]
    fn the_chords_are_distinct() {
        assert_ne!(clear_chord().id(), marker_chord().id());
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
