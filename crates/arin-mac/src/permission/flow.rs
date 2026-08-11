//! Asking for Screen Recording once, then watching for the switch to flip.
//!
//! # Why this stopped opening System Settings on every start
//!
//! It used to do this on each run that found the permission missing: raise the system
//! prompt, notice macOS had stayed silent because it had asked before, and take the user to
//! System Settings on the assumption they were on their way there anyway.
//!
//! Under a launch agent that assumption is wrong in the worst way. The daemon starts at
//! login, finds the permission missing, and throws System Settings into the user's face
//! before they have opened anything. Every login. Nothing the user does in that pane
//! necessarily fixes it either, because the reason the permission reads as missing may be
//! that the build has no identity to hold a grant, which is [`super::identity`]'s subject.
//! So an unprompted trip to System Settings is now the exception:
//!
//! - A login agent never opens it. It has no business taking focus, and the menu bar item
//!   is already showing the permission state with a link to the same pane.
//! - Anything else opens it at most once per build, remembered on disk.
//!
//! Arin still asks the system to prompt on every start, because that call is how macOS
//! wants to be asked and it is silent after the first time by design.

use super::{identity, settings, tcc};
use arin_core::Access;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How long to keep watching for a grant before giving up.
///
/// Long enough for someone to go and find the switch, short enough that a daemon left
/// running for a week is not still polling for something that is never coming.
const WATCH_FOR: Duration = Duration::from_secs(300);

/// How often to look while watching.
const POLL: Duration = Duration::from_secs(2);

/// Prompt for Screen Recording if it is missing, then watch until it is granted.
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
            tracing::warn!("{}", identity::of_this_build().explain());
            return;
        }
        Access::Missing => {}
    }

    tracing::info!(
        "screen recording is needed for scroll detection. Annotations will draw either way, \
         but they will not clear when the page moves under them."
    );

    ask();
    watch_for_grant();
}

/// Ask for the permission, and decide whether to walk the user to the switch.
fn ask() {
    let prompt = tcc::request();

    if prompt == tcc::Prompt::Shown {
        // The dialog is up and the user is reading it. Opening System Settings underneath it
        // would be answering the question for them.
        return;
    }

    // Silent, so macOS has asked this user about Arin before and never will again. The switch
    // is the only way through from here, and whether to take them to it is the whole subject
    // of this module.
    match Interruption::allowed() {
        Interruption::Allowed => {
            tracing::info!("opening System Settings, since macOS will not ask again");
            settings::open_screen_recording_settings();
        }
        Interruption::LoginAgent => {
            tracing::info!(
                "screen recording is off. Not opening System Settings, since Arin started at \
                 login and should not take your screen to do it. The menu bar item has the \
                 switch, or {}.",
                super::SCREEN_RECORDING_HELP
            );
        }
        Interruption::AlreadySent => {
            tracing::info!(
                "screen recording is off, and Arin has already sent you to the switch once \
                 for this build. Not opening System Settings again. To grant it, {}.",
                super::SCREEN_RECORDING_HELP
            );
        }
    }
}

/// Whether Arin may take the user's screen to show them the switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Interruption {
    /// Started by hand, and this build has not sent them yet.
    Allowed,
    /// Started at login, where taking focus is never acceptable.
    LoginAgent,
    /// This build has already opened System Settings once.
    AlreadySent,
}

impl Interruption {
    fn allowed() -> Self {
        if started_by_launch_agent() {
            return Self::LoginAgent;
        }
        if already_sent() {
            return Self::AlreadySent;
        }
        // Recorded before the pane opens rather than after, so a failure to remember costs
        // one extra trip to System Settings rather than one on every start, which is the
        // failure this whole module is here to stop.
        remember_sent();
        Self::Allowed
    }
}

/// Whether launchd started this as the login agent.
///
/// `XPC_SERVICE_NAME` carries the job label under launchd and is the string `0` in a shell,
/// which is the difference being read here. `__CFBundleIdentifier`, which `cli::Launch`
/// checks, answers a different question: that one is set by LaunchServices when Finder opens
/// the app, and is absent for an agent.
fn started_by_launch_agent() -> bool {
    std::env::var("XPC_SERVICE_NAME").is_ok_and(|name| name != "0")
}

/// Where the memory of having sent someone to System Settings lives.
fn marker() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join("Library/Application Support/Arin")
            .join("screen-recording-prompted"),
    )
}

/// What identifies this build for the purpose of not asking twice.
///
/// The binary's path and modification time rather than its `cdhash`, which would be the
/// honest key and would cost a `codesign` run on a path that is meant to be cheap. Both
/// change when a build is replaced, which is the only property needed: a new build should be
/// allowed to ask once, because a new build genuinely needs a new grant.
fn build_key() -> String {
    let Ok(exe) = std::env::current_exe() else {
        return "unknown".into();
    };
    let modified = std::fs::metadata(&exe)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_secs())
        .unwrap_or(0);
    format!("{} {modified}", exe.display())
}

fn already_sent() -> bool {
    let Some(marker) = marker() else {
        return false;
    };
    std::fs::read_to_string(marker).is_ok_and(|recorded| recorded.trim() == build_key())
}

fn remember_sent() {
    let Some(marker) = marker() else {
        return;
    };
    let written = marker
        .parent()
        .map(std::fs::create_dir_all)
        .transpose()
        .and_then(|_| std::fs::write(&marker, build_key()));
    if let Err(e) = written {
        // Not worth a warning. The cost is one more trip to System Settings on the next
        // start, and the user has a real problem in front of them already.
        tracing::debug!(error = %e, path = %marker.display(), "could not record the prompt");
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this module was rewritten for. A daemon started at login must not open
    /// System Settings, however many times it finds the permission missing.
    #[test]
    fn a_login_agent_is_never_allowed_to_interrupt() {
        // Safety: single threaded test, and both variables are restored before it returns.
        let saved = std::env::var_os("XPC_SERVICE_NAME");
        unsafe { std::env::set_var("XPC_SERVICE_NAME", "com.anistark.arin") };
        assert!(started_by_launch_agent());
        assert_eq!(Interruption::allowed(), Interruption::LoginAgent);

        // What a shell sets it to, which must read as a hand started daemon.
        unsafe { std::env::set_var("XPC_SERVICE_NAME", "0") };
        assert!(!started_by_launch_agent());

        unsafe { std::env::remove_var("XPC_SERVICE_NAME") };
        assert!(!started_by_launch_agent());

        if let Some(saved) = saved {
            unsafe { std::env::set_var("XPC_SERVICE_NAME", saved) };
        }
    }

    /// A key that did not change between two builds would let a replaced binary inherit the
    /// old one's "already asked", which is the opposite mistake and just as annoying.
    #[test]
    fn the_build_key_names_the_running_binary() {
        let key = build_key();
        assert!(key.contains(char::is_whitespace), "no timestamp in {key}");
        assert_eq!(key, build_key(), "the key has to be stable within a run");
    }
}
