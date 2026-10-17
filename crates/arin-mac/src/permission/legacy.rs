//! Clearing the Screen Recording rows that earlier bundle identifiers left behind.
//!
//! macOS keeps one Screen Recording row per bundle identifier, so a machine that ran a build
//! from before the identifier changed lists Arin twice, and the old row belongs to nothing
//! that will run again. Neither Homebrew hook can clear it: a formula's `post_install` and a
//! cask's `postflight_steps` both run in a sandbox that refuses `tccutil`. The daemon runs as
//! the user with nothing in the way, so it does it.
//!
//! `tccutil` finds an identifier through LaunchServices and refuses one that no installed app
//! carries, which after an upgrade is the usual case. The row then stays until the user
//! removes it in System Settings. That is expected, not a fault, and is logged at debug.

use super::record;
use arin_core::LEGACY_BUNDLE_IDS;
use std::process::Command;

/// What `tccutil` made of one reset.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// The row is gone, or there was none to begin with. `tccutil` does not say which.
    Cleared,
    /// No installed app carries the identifier, so `tccutil` could not resolve it.
    Unregistered,
    Failed(String),
}

/// Reset Screen Recording for every identifier in [`LEGACY_BUNDLE_IDS`] not yet cleared.
///
/// Blocks on a `tccutil` run per identifier, so keep it off the main thread.
pub fn retire() {
    for &identifier in LEGACY_BUNDLE_IDS {
        if record::retired(identifier) {
            continue;
        }
        let reset = Command::new("/usr/bin/tccutil")
            .args(["reset", "ScreenCapture", identifier])
            .output();
        match reset.map(|output| outcome(output.status.success(), &output.stderr)) {
            Ok(Outcome::Cleared) => {
                record::remember_retired(identifier);
                tracing::info!(
                    identifier,
                    "cleared the Screen Recording row an earlier build of Arin left under its \
                     old identifier"
                );
            }
            Ok(Outcome::Unregistered) => tracing::debug!(
                identifier,
                "no installed app carries the old identifier, so its Screen Recording row, if \
                 there is one, has to be removed in System Settings"
            ),
            Ok(Outcome::Failed(reason)) => {
                tracing::debug!(identifier, %reason, "could not clear the old Screen Recording row");
            }
            Err(e) => tracing::debug!(identifier, error = %e, "could not run tccutil"),
        }
    }
}

fn outcome(succeeded: bool, stderr: &[u8]) -> Outcome {
    if succeeded {
        return Outcome::Cleared;
    }
    let stderr = String::from_utf8_lossy(stderr).trim().to_owned();
    if stderr.contains("No such bundle identifier") {
        Outcome::Unregistered
    } else {
        Outcome::Failed(stderr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Running `tccutil` from a test would reset a real grant, so these hold the output it was
    /// seen to print on macOS 26.6 instead.
    #[test]
    fn an_identifier_nothing_carries_is_not_a_failure() {
        let stderr = b"tccutil: No such bundle identifier \"com.anistark.arin\": The operation \
                       couldn\xe2\x80\x99t be completed. (OSStatus error -10814.)\n";
        assert_eq!(outcome(false, stderr), Outcome::Unregistered);
    }

    #[test]
    fn a_sandbox_refusal_is_a_failure() {
        let stderr = b"tccutil: Operation not permitted from sandbox\n";
        assert_eq!(
            outcome(false, stderr),
            Outcome::Failed("tccutil: Operation not permitted from sandbox".into())
        );
    }

    #[test]
    fn success_is_cleared_whatever_was_printed() {
        assert_eq!(outcome(true, b""), Outcome::Cleared);
    }
}
