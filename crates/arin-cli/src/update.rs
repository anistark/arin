//! Whether a newer Arin has been released.
//!
//! Notify only. This never downloads anything and never installs anything, and that is a
//! decision rather than a stage. Builds are unsigned today, so an updater that fetched one
//! would land the user in front of Gatekeeper, which is the thing an updater exists to
//! avoid; and replacing a running app on transport trust alone, with no signature to
//! check, is a code execution path dressed up as a convenience. Downloading waits for the
//! certificate.
//!
//! It is also the smaller half of the problem. The documented install route is Homebrew,
//! so `brew upgrade` already updates anybody who followed the instructions. What is
//! missing is not the mechanism but the knowing, and that is what this supplies.
//!
//! # It phones home
//!
//! One request to GitHub, carrying nothing but a user agent. That is still egress from a
//! daemon whose entire consent story rests on grounding being the only thing that leaves
//! the machine, so it is off unless asked for, exactly like `--resolver`.

use anyhow::{Context, Result};

/// Where releases are published.
const RELEASES_API: &str = "https://api.github.com/repos/anistark/arin/releases/latest";

/// What this build is.
pub(crate) const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// How the user upgrades, which is the only actionable part of the answer.
const UPGRADE: &str = "brew upgrade anistark/tools/arin";

/// A release version, compared by precedence rather than as a string.
///
/// `"0.10.0" > "0.9.0"` is false as text and true as a version, which is the whole reason
/// this is not a string comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Version(u64, u64, u64);

impl Version {
    /// Read `0.2.1`, or `v0.2.1` as a tag spells it.
    ///
    /// Anything with a suffix, `0.3.0-rc1`, is refused rather than guessed at. A
    /// pre-release is not newer than the release it precedes, and treating it as one would
    /// nag every user of a stable build to "upgrade" to something unfinished.
    fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim().strip_prefix('v').unwrap_or(raw.trim());
        let mut parts = raw.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self(major, minor, patch))
    }
}

/// Whether `latest` is a release newer than `current`.
///
/// Returns false when either side cannot be read, because the honest answer to "is there
/// something newer" when the question cannot be parsed is silence rather than a prompt.
pub(crate) fn is_newer(latest: &str, current: &str) -> bool {
    match (Version::parse(latest), Version::parse(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

/// Ask GitHub for the most recent release tag.
///
/// A user agent is not optional: the API refuses requests without one. No token, so this
/// is subject to the unauthenticated rate limit of sixty an hour per address, which is
/// what makes a daily check the right cadence and a check on every start the wrong one.
pub(crate) async fn latest_release() -> Result<String> {
    #[derive(serde::Deserialize)]
    struct Release {
        tag_name: String,
    }

    let client = reqwest::Client::builder()
        .user_agent(concat!("arin/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("building the http client")?;

    let response = client
        .get(RELEASES_API)
        .send()
        .await
        .context("asking GitHub for the latest release")?;

    // A rate limit reads as a 403 with a body that is not a release, so the status is
    // checked before the body is parsed. Otherwise the failure surfaces as a confusing
    // deserialization error rather than as what actually happened.
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("GitHub answered {status} rather than a release");
    }

    let release: Release = response
        .json()
        .await
        .context("reading the release GitHub sent back")?;
    Ok(release.tag_name)
}

/// The newer version found by the background check, if there is one.
///
/// A static rather than state on the daemon, because `arin-core` has no business knowing
/// that releases exist. The menu bar reads it when the menu opens.
static AVAILABLE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// What the background check last found, for the menu bar to show.
///
/// Gated to where something reads it. The menu bar is the only reader and it is macOS
/// only, so on any other platform this is dead code and CI denies that. The static above
/// stays ungated: the check itself runs anywhere, and writing to it is a use.
#[cfg(target_os = "macos")]
pub(crate) fn available() -> Option<String> {
    AVAILABLE.lock().expect("update lock").clone()
}

/// Check on startup and once a day after, until the daemon stops.
///
/// Only ever called when `--check-updates` was passed, which is what makes the egress
/// something the user asked for rather than something Arin decided.
///
/// A failed check is logged and forgotten. Being unable to reach GitHub is not a reason to
/// interrupt somebody drawing on their screen, and a daemon that retried hard would turn a
/// flaky network into the rate limit.
///
/// Daily is enforced by the interval alone rather than by a timestamp on disk. The daemon's
/// life is a login session, so restarts are rare enough that a check per start plus one a
/// day stays far inside sixty an hour. A developer restarting in a loop could pass that,
/// and would get a logged warning rather than anything worse.
pub(crate) async fn watch_for_releases() {
    const A_DAY: std::time::Duration = std::time::Duration::from_secs(60 * 60 * 24);

    loop {
        match latest_release().await {
            Ok(latest) if is_newer(&latest, CURRENT) => {
                let latest = latest.trim_start_matches('v').to_owned();
                tracing::info!(
                    latest = %latest,
                    current = CURRENT,
                    "a newer Arin is available. Upgrade with `{UPGRADE}`"
                );
                *AVAILABLE.lock().expect("update lock") = Some(latest);
            }
            Ok(_) => {
                tracing::debug!(current = CURRENT, "this is the latest Arin");
                *AVAILABLE.lock().expect("update lock") = None;
            }
            Err(e) => tracing::debug!(error = format!("{e:#}"), "could not check for updates"),
        }
        tokio::time::sleep(A_DAY).await;
    }
}

/// Check, and say what was found in the terms the reader needs.
pub(crate) fn check() -> Result<()> {
    // The fetch lives inside the future because `block_on` here only carries `Result<()>`,
    // and widening it for one caller is a worse trade than moving three lines.
    crate::block_on(async {
        let latest = latest_release().await?;
        println!("{}", report(&latest, CURRENT));
        Ok(())
    })
}

/// What to tell the reader, given what the two versions are.
///
/// Separated from printing it so the wording is covered by a test. The only actionable
/// part of an upgrade notice is the command, so that is what it leads with.
fn report(latest: &str, current: &str) -> String {
    if !is_newer(latest, current) {
        return format!("Arin {current} is the latest.");
    }
    let latest = latest.trim_start_matches('v');
    format!(
        "Arin {latest} is out. You have {current}.\n\n  {UPGRADE}\n\n\
         Installed another way? The release page has the dmg:\n  \
         https://github.com/anistark/arin/releases/latest"
    )
}

#[cfg(test)]
mod tests {
    use super::{CURRENT, Version, is_newer};

    /// The reason this is not a string comparison, stated as a test so nobody
    /// simplifies it back into one.
    #[test]
    fn versions_compare_by_number_and_not_as_text() {
        assert!(
            is_newer("0.10.0", "0.9.0"),
            "0.10.0 is newer than 0.9.0, though it sorts earlier as a string"
        );
        assert!(!is_newer("0.9.0", "0.10.0"));
    }

    /// Tags carry a `v` and the manifest does not, so both spellings have to read the
    /// same or every check would report an upgrade to the version already installed.
    #[test]
    fn a_tag_and_a_manifest_version_read_the_same() {
        assert_eq!(Version::parse("v0.2.1"), Version::parse("0.2.1"));
        assert!(
            !is_newer("v0.2.1", "0.2.1"),
            "the same version is not newer"
        );
        assert!(is_newer("v0.3.0", "0.2.1"));
    }

    /// Silence is the right answer to a question that cannot be read. Nagging somebody to
    /// upgrade to something unparseable is worse than saying nothing.
    #[test]
    fn anything_unreadable_reports_nothing_newer() {
        for latest in ["", "latest", "0.2", "0.2.1.4", "v", "nightly-2026-08-07"] {
            assert!(
                !is_newer(latest, "0.2.1"),
                "{latest:?} is not a release version and must not prompt an upgrade"
            );
        }
    }

    /// A pre-release precedes the release it names, so it must not be offered to somebody
    /// on a stable build.
    #[test]
    fn a_prerelease_is_not_an_upgrade() {
        assert!(!is_newer("0.3.0-rc1", "0.2.1"));
        assert!(!is_newer("v0.3.0-rc1", "0.2.1"));
    }

    /// An upgrade notice that does not say how to upgrade is a nag. The command is the
    /// only actionable thing in it, so it is what the test pins.
    #[test]
    fn an_upgrade_notice_names_the_command_and_both_versions() {
        let notice = super::report("v0.3.0", "0.2.1");
        assert!(notice.contains("0.3.0"), "says what is available: {notice}");
        assert!(notice.contains("0.2.1"), "says what you have: {notice}");
        assert!(
            notice.contains("brew upgrade anistark/tools/arin"),
            "says how to get it: {notice}"
        );
    }

    /// Being up to date is one line. Anything more reads as a problem when there is none.
    #[test]
    fn being_current_says_so_and_stops() {
        let notice = super::report("v0.2.1", "0.2.1");
        assert_eq!(notice, "Arin 0.2.1 is the latest.");
        assert!(!notice.contains("brew"), "nothing to do, so nothing to run");
    }

    /// The version this build reports has to be readable by the comparison it feeds, or
    /// every check silently answers "nothing newer" forever.
    #[test]
    fn this_builds_own_version_parses() {
        assert!(
            Version::parse(CURRENT).is_some(),
            "CARGO_PKG_VERSION is {CURRENT:?}, which the comparison cannot read"
        );
    }
}
