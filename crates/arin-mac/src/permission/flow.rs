//! Asking for Screen Recording once per build, then watching for the switch to flip.
//!
//! # Why this asks once per build
//!
//! It used to ask at every start, on the understanding that macOS stays silent after the
//! first time. It does not in the state every upgrade leaves behind. macOS keeps one Screen
//! Recording row for Arin, pinned to the exact build it was granted to, and while that row
//! belongs to another build it holds no answer for this one and puts its dialog up at every
//! request. So the daemon raised the dialog at every login, on a machine where System
//! Settings showed Arin switched on. That is the bug fixed on 2026-09-29.
//!
//! So a build asks once, and the record of having asked is kept against it. After that the
//! menu bar item is the way in, and it asks again because somebody chose it.
//!
//! # Why this never opens System Settings
//!
//! It used to, once per build, when the request looked as though macOS had stayed silent.
//! That was read off how long the call took, and on macOS 26 the call returns before the
//! dialog appears (see [`super::tcc::request`]), so the pane opened underneath the dialog.
//! The dialog has its own button for the pane, and a daemon started at login has no business
//! taking the screen, which was the bug fixed on 2026-08-11. The pane opens from the menu bar
//! item and from `arin permissions --open`, and from nowhere else.

use super::{record, tcc};
use arin_core::Access;
use std::time::{Duration, Instant};

/// How long to keep watching for a grant before giving up.
///
/// Long enough for someone to go and find the switch, short enough that a daemon left
/// running for a week is not still polling for something that is never coming.
const WATCH_FOR: Duration = Duration::from_secs(300);

/// How often to look while watching.
const POLL: Duration = Duration::from_secs(2);

/// Ask for Screen Recording if it is missing, then watch until it is granted.
///
/// Returns immediately, doing the work on its own thread. Nothing waits on the outcome: the
/// overlay draws perfectly well without capture, and only scroll invalidation is off until
/// the permission lands.
pub fn begin_screen_recording_flow() {
    std::thread::Builder::new()
        .name("arin-permission".into())
        .spawn(run)
        .expect("spawn the permission thread");
}

fn run() {
    match super::access() {
        Access::Working => {
            record::remember_holder();
            // At info, alongside the socket and the hotkey. A user checking whether capture
            // works should find the answer in the startup lines rather than having to infer
            // it from the absence of a complaint.
            tracing::info!("screen recording granted, scroll detection is live");
            return;
        }
        Access::NotRequired => return,
        Access::NeedsRestart => {
            tracing::warn!("{}", super::explain(Access::NeedsRestart));
            return;
        }
        // No amount of waiting helps, because the grant has nothing to attach to. Say what
        // is wrong and stop rather than poll for five minutes on a state that cannot change.
        Access::Unidentified => {
            tracing::warn!("{}", super::identity::of_this_build().explain());
            return;
        }
        Access::Missing => {}
    }

    tracing::info!(
        "screen recording is needed for scroll detection. Annotations will draw either way, \
         but they will not clear when the page moves under them."
    );

    // The state an upgrade leaves behind, and the one that looks impossible from System
    // Settings, where the switch reads as on.
    if let Some(holder) = record::holder_elsewhere() {
        tracing::warn!("{}", holder.explain());
    }

    if record::asked() {
        tracing::info!(
            "screen recording is off, and Arin asked macOS for it when this build first \
             started. Not asking again, since macOS can put the same dialog up at every start. \
             To grant it, choose Screen Recording from Arin's menu bar item, or {}.",
            super::SCREEN_RECORDING_HELP
        );
        return;
    }

    record::remember_asked();
    tcc::request();
    watch_for_grant();
}

/// Poll until the permission appears, then say what state it left things in.
fn watch_for_grant() {
    let deadline = Instant::now() + WATCH_FOR;
    while Instant::now() < deadline {
        std::thread::sleep(POLL);
        if !tcc::granted() {
            continue;
        }
        match super::access() {
            Access::Working => {
                record::remember_holder();
                tracing::info!("screen recording granted, scroll detection is live");
            }
            // The common ending. The user granted it just now, so ScreenCaptureKit will not
            // serve this process until it restarts.
            other => tracing::warn!("{}", super::explain(other)),
        }
        return;
    }

    tracing::warn!(
        "gave up waiting for screen recording. Scroll detection stays off for this run. To \
         turn it on, {}, then restart Arin.",
        super::SCREEN_RECORDING_HELP
    );
}
