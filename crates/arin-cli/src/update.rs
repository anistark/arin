//! `arin update` and `arin upgrade`: whether a newer Arin has been released, and moving to it.
//!
//! `update` stays on the installed major version and `upgrade` takes the newest release there
//! is. Major means the first number, on 0.x as well, so 0.7 to 0.8 is an `update` and 1.0 is
//! an `upgrade`. `--check` on either reports and changes nothing.
//!
//! # Arin never replaces itself
//!
//! This never downloads a build and never installs one, and that is a decision rather than a
//! stage. Builds are unsigned today, so an updater that fetched one would land the user in
//! front of Gatekeeper, which is the thing an updater exists to avoid. Replacing a running app
//! on transport trust alone, with no signature to check, is a code execution path dressed up
//! as a convenience. Downloading waits for the certificate.
//!
//! What it may do, when the person runs the command and not otherwise, is hand over to the
//! package manager that already owns the install. The documented install route is Homebrew,
//! so `brew upgrade` is the mechanism that already updates anybody who followed the
//! instructions, and Homebrew builds from source on the machine, which is exactly why it never
//! meets Gatekeeper. Nothing about that route changes by Arin typing the command for them.
//!
//! Homebrew is the only one. Every other install changes nothing, prints what to do, and exits
//! non-zero:
//!
//! - The dmg is a download, which is the thing this refuses to do.
//! - Nix could be run, and is not. A store path says Nix built this copy and not whether `nix
//!   run`, a profile, or nix-darwin holds it, and those update differently. A profile follows
//!   `main` rather than a release, so `nix profile upgrade` cannot be held to a major version.
//! - `cargo install` is the same bare binary from a clone or a tag, and which one is the
//!   person's call. Never `cargo install arin`, which is a library and has no binary.
//!
//! The cask in `packaging/homebrew` is not in the tap yet. When it is, an `Arin.app` in
//! `/Applications` could be the cask or the dmg, and `Install::of` has to learn the
//! difference.
//!
//! Every route installs only its newest release, so nothing can stop short of a new major
//! version. `update` holds back once one is out and says `upgrade` is what moves to it, which
//! is also why the one request below is all this needs.
//!
//! # It phones home
//!
//! One request to GitHub, carrying nothing but a user agent. That is still egress from a
//! daemon whose entire consent story rests on grounding being the only thing that leaves
//! the machine, so the daemon's check is off unless asked for, exactly like `--resolver`, and
//! the commands make it only when someone runs them. Nothing here runs in the background
//! except that opt-in watcher, and the watcher only ever reports.

use anyhow::{Context, Result, bail};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where releases are published.
const RELEASES_API: &str = "https://api.github.com/repos/anistark/arin/releases/latest";

/// Names a stand-in for [`RELEASES_API`], so the commands can be tested against a stub.
///
/// The environment is the only way to set it, and no file read from the current directory
/// ever is, because a cloned project must not get to choose what this believes is out.
const RELEASES_API_VARIABLE: &str = "ARIN_RELEASES_API";

/// What this build is.
pub(crate) const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// The one thing Arin will run. The tap is named so no other formula called `arin` can answer.
const UPGRADE: [&str; 3] = ["brew", "upgrade", "anistark/tools/arin"];

/// Where the release page is, for everything Homebrew does not own.
const RELEASE_PAGE: &str = "https://github.com/anistark/arin/releases/latest";

/// Where the formula puts the binary, reached through the alias Homebrew keeps re-pointing.
const KEG_BINARY: &str = "opt/arin/Arin.app/Contents/MacOS/arin";

/// What an install has to be told once it has moved.
///
/// An upgrade does not touch a daemon that is already running, and Arin restarts nothing on
/// its own, so this is the person's to run. The grant is here because every build needs its
/// own until releases are signed, and finding that out from marks that stopped following a
/// scroll is a poor way to learn it.
const AFTER_MOVING: &str = "A daemon that is already running stays on the old build until it \
                            restarts:\n\n  arin service restart\n\n\
                            Started with `arin -d`? Stop it and start it again.\n\n\
                            Screen Recording has to be granted again for the new build:\n  \
                            https://arin.anirudha.dev/docs/install/#when-permissions-go-wrong";

/// How far a run may move this install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// `arin update`: minor and patch releases of the installed major version.
    SameMajor,
    /// `arin upgrade`: the newest release, whatever its major version.
    Latest,
}

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

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
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

    let stand_in = std::env::var(RELEASES_API_VARIABLE)
        .ok()
        .map(|raw| on_this_machine(&raw))
        .transpose()?;

    let mut client = reqwest::Client::builder()
        .user_agent(concat!("arin/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(10));
    // Nothing on loopback needs one, and a proxy is a way for a request meant to stay here to
    // leave.
    if stand_in.is_some() {
        client = client.no_proxy();
    }
    let client = client.build().context("building the http client")?;

    let response = client
        .get(stand_in.as_deref().unwrap_or(RELEASES_API))
        .send()
        .await
        .context("asking GitHub for the latest release")?;

    // A rate limit reads as a 403 or a 429 with a body that is not a release, so the status is
    // checked before the body is parsed. Otherwise the failure surfaces as a confusing
    // deserialization error rather than as what actually happened.
    //
    // The limit is per address, so a machine behind a shared connection can arrive here
    // having made no requests of its own, and "403 Forbidden" reads as Arin being refused
    // rather than as a quota somebody else used up. GitHub says which it is in the
    // headers, so that is what is reported, along with when it clears.
    let status = response.status();
    if !status.is_success() {
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let refused = matches!(
            status,
            reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::TOO_MANY_REQUESTS
        );
        if refused && header("x-ratelimit-remaining").as_deref() == Some("0") {
            let reset = header("x-ratelimit-reset").and_then(|v| v.parse().ok());
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            anyhow::bail!("{}", rate_limited(reset, now));
        }
        anyhow::bail!("GitHub answered {status} rather than a release");
    }

    let release: Release = response
        .json()
        .await
        .context("reading the release GitHub sent back")?;
    Ok(release.tag_name)
}

/// What to say when GitHub's unauthenticated quota for this address is spent.
///
/// `reset` is the epoch second GitHub says the window reopens at, if it sent one, and
/// `now` is passed in rather than read so the wording can be tested. The answer is a
/// duration rather than a clock time because a duration needs no time zone to be read.
fn rate_limited(reset: Option<u64>, now: u64) -> String {
    let what = "GitHub's rate limit for this address is used up";
    match reset {
        Some(at) if at > now => {
            let minutes = (at - now).div_ceil(60);
            let s = if minutes == 1 { "" } else { "s" };
            format!("{what}. It resets in about {minutes} minute{s}.")
        }
        Some(_) => format!("{what}. It should have reset by now, so try again."),
        None => format!("{what}. Try again in an hour."),
    }
}

/// Accept a stand-in for the release API only if it names this machine.
///
/// A test seam rather than a mirror setting. Holding it to loopback is what keeps "the only
/// place this request goes is GitHub" true however the environment is set, which is the same
/// reason the local resolver refuses anything else.
fn on_this_machine(raw: &str) -> Result<String> {
    let url = reqwest::Url::parse(raw)
        .with_context(|| format!("{RELEASES_API_VARIABLE} is {raw:?}, which is not a url"))?;
    if !arin_resolve::local::names_loopback(&url) {
        bail!(
            "{RELEASES_API_VARIABLE} is {raw:?}, which is not on this machine. Releases are \
             only ever asked for from GitHub, so this will only talk to 127.0.0.1, ::1, or \
             localhost."
        );
    }
    Ok(raw.to_owned())
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
                let command = match decide(&latest, CURRENT, Scope::SameMajor) {
                    Decision::HeldBack(_) => "arin upgrade",
                    _ => "arin update",
                };
                let latest = latest.trim_start_matches('v').to_owned();
                tracing::info!(
                    latest = %latest,
                    current = CURRENT,
                    "a newer Arin is available. `{command}` moves to it, or says how"
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

/// What a run should do, given what is out and what this is.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// Nothing newer, or nothing that could be read as newer.
    UpToDate,
    /// The newest release is a new major version and this is `arin update`.
    HeldBack(Version),
    /// Move to this release.
    Move(Version),
}

/// The major version rule, on its own so it is the one place to read it.
///
/// Anything unreadable is up to date, for the reason [`is_newer`] gives.
fn decide(latest: &str, current: &str, scope: Scope) -> Decision {
    let (Some(latest), Some(current)) = (Version::parse(latest), Version::parse(current)) else {
        return Decision::UpToDate;
    };
    if latest <= current {
        Decision::UpToDate
    } else if scope == Scope::SameMajor && latest.0 > current.0 {
        Decision::HeldBack(latest)
    } else {
        Decision::Move(latest)
    }
}

/// How this copy of Arin got here, read from where the binary really is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Install {
    /// In a keg, and `<prefix>/opt/arin` agrees.
    Homebrew,
    /// In the Nix store.
    Nix,
    /// The binary in Cargo's own bin directory.
    Cargo,
    /// An `Arin.app` nothing above owns, from the dmg or built from a clone.
    App,
    /// Anywhere else, a build in `target/` among them.
    Unknown,
}

impl Install {
    /// Place the running binary.
    ///
    /// Resolved first, because on macOS the running path is the one it was typed by, and for
    /// anything on PATH that is a symlink saying nothing about what is behind it.
    fn detect() -> Self {
        let cargo_home = std::env::var_os("CARGO_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".cargo")));
        std::env::current_exe()
            .and_then(std::fs::canonicalize)
            .map_or(Self::Unknown, |binary| {
                Self::of(&binary, cargo_home.as_deref())
            })
    }

    /// Place a resolved binary.
    ///
    /// Whole paths and the filesystem, never a substring, because the failure this guards
    /// against is a clone in a folder called `homebrew-tools` running `brew upgrade` on an
    /// install it has nothing to do with. Anything in doubt falls through to a variant that
    /// changes nothing.
    fn of(binary: &Path, cargo_home: Option<&Path>) -> Self {
        let alias = unversioned(binary.to_path_buf());
        if alias != binary && alias.ends_with(KEG_BINARY) {
            Self::Homebrew
        } else if binary.starts_with("/nix/store") {
            Self::Nix
        } else if cargo_home.is_some_and(|home| {
            std::fs::canonicalize(home.join("bin/arin")).is_ok_and(|found| found == binary)
        }) {
            Self::Cargo
        } else if binary.ends_with("Arin.app/Contents/MacOS/arin") {
            Self::App
        } else {
            Self::Unknown
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Homebrew => "Homebrew",
            Self::Nix => "Nix",
            Self::Cargo => "cargo install",
            Self::App => "Arin.app, from the dmg or built by hand",
            Self::Unknown => "unknown",
        }
    }

    /// What the person has to do themselves, for every install Arin does not hand over from.
    ///
    /// `None` is Homebrew, the one route with something to run.
    fn by_hand(self, target: Version) -> Option<String> {
        let unchanged = "so nothing was changed";
        match self {
            Self::Homebrew => None,
            Self::Nix => Some(format!(
                "Nix built this copy, and a store path does not say whether `nix run`, a profile \
                 or nix-darwin holds it, {unchanged}.\n\n  \
                 nix profile upgrade arin    for a profile\n  \
                 nix flake update arin       for nix-darwin, then rebuild\n\n\
                 `nix run github:anistark/arin` needs nothing: it follows `main` by itself."
            )),
            Self::Cargo => Some(format!(
                "This copy came from `cargo install`, and Arin only hands over to Homebrew, \
                 {unchanged}. To build {target}:\n\n  \
                 cargo install --git https://github.com/anistark/arin --tag v{target} arin-cli"
            )),
            Self::App => Some(format!(
                "No package manager owns this Arin.app and Arin does not download builds, \
                 {unchanged}. The release page has the new dmg:\n\n  {RELEASE_PAGE}"
            )),
            Self::Unknown => Some(format!(
                "Arin cannot tell what installed this copy, {unchanged}. Homebrew keeps it \
                 current from here on:\n\n  brew install anistark/tools/arin\n\n\
                 The release page has the dmg:\n\n  {RELEASE_PAGE}"
            )),
        }
    }
}

/// `arin update` and `arin upgrade`.
pub(crate) fn run(scope: Scope, check: bool) -> Result<()> {
    let latest = crate::block_on(latest_release())?;
    let decision = decide(&latest, CURRENT, scope);
    let install = Install::detect();

    if check {
        println!("{}", plan(&decision, install, CURRENT));
        return Ok(());
    }
    match decision {
        Decision::UpToDate => println!("{}", up_to_date(CURRENT)),
        Decision::HeldBack(latest) => println!("{}", held_back(latest, CURRENT)),
        Decision::Move(target) => {
            println!("{}\n", announcement(target, CURRENT));
            let Some(instructions) = install.by_hand(target) else {
                return hand_over(target);
            };
            println!("{instructions}");
            std::process::exit(1);
        }
    }
    Ok(())
}

/// Being up to date is one line. Anything more reads as a problem when there is none.
fn up_to_date(current: &str) -> String {
    format!("Arin {current} is the latest.")
}

fn announcement(latest: Version, current: &str) -> String {
    format!("Arin {latest} is out. You have {current}.")
}

/// Why `arin update` stopped, and what moves past it.
fn held_back(latest: Version, current: &str) -> String {
    format!(
        "{}\n\n\
         That is a new major version, which `arin update` leaves alone. Every install gets only \
         the newest release, so nothing was changed.\n\n  arin upgrade",
        announcement(latest, current)
    )
}

/// What `--check` reports: how Arin was installed, where it would move, and the exact command.
fn plan(decision: &Decision, install: Install, current: &str) -> String {
    let row = |label: &str, value: &str| format!("  {label:<16}{value}");
    let method = row("Installed with", install.name());
    match *decision {
        Decision::UpToDate => up_to_date(current),
        Decision::HeldBack(latest) => format!(
            "{}\n\n{method}\n{}\n{}\n\n\
             {latest} is a new major version, which `arin update` leaves alone. \
             `arin upgrade` moves to it.",
            announcement(latest, current),
            row("Target", "none on this major version"),
            row("Would run", "nothing"),
        ),
        Decision::Move(target) => {
            let target_row = row("Target", &target.to_string());
            let head = announcement(target, current);
            match install.by_hand(target) {
                None => {
                    let run = row("Would run", &UPGRADE.join(" "));
                    format!("{head}\n\n{method}\n{target_row}\n{run}")
                }
                Some(instructions) => {
                    let run = row("Would run", "nothing");
                    format!("{head}\n\n{method}\n{target_row}\n{run}\n\n{instructions}")
                }
            }
        }
    }
}

/// Run the package manager, then say what actually changed.
///
/// The terminal is inherited and there is no deadline. The formula compiles Arin on the
/// machine, which takes as long as it takes, and whatever Homebrew has to say or ask goes
/// straight to the person it is for.
fn hand_over(target: Version) -> Result<()> {
    let line = UPGRADE.join(" ");
    println!("Running: {line}\n");
    match Command::new(UPGRADE[0]).args(&UPGRADE[1..]).status() {
        Ok(status) if status.success() => {}
        Ok(status) => bail!("`{line}` failed ({status}). What Homebrew said is above."),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!(
            "Homebrew installed this copy, but `brew` is not on your PATH. Run `{line}` from a \
             shell that has it."
        ),
        Err(e) => return Err(e).with_context(|| format!("could not run `{line}`")),
    }
    println!(
        "\n{}",
        outcome(version_on_path().as_deref(), target, CURRENT)
    );
    Ok(())
}

/// What typing `arin` gives now.
///
/// Found through PATH rather than from the running binary, because the path this process
/// started from is the keg `brew upgrade` has just removed.
fn version_on_path() -> Option<String> {
    let output = Command::new("arin").arg("--version").output().ok()?;
    let banner = String::from_utf8_lossy(&output.stdout);
    let version = banner.trim().strip_prefix("arin ")?;
    (output.status.success() && !version.is_empty()).then(|| version.to_owned())
}

/// What to say once Homebrew has finished, from what `arin` reports rather than from what was
/// asked for.
///
/// Homebrew finishing is not the same as Arin having moved. The tap follows a release by a
/// pull request, so for a while it offers nothing new and exits happily. That is not a
/// failure, and it is not a success to claim either.
fn outcome(found: Option<&str>, target: Version, current: &str) -> String {
    let lagging = "Homebrew may not offer it yet, or another copy comes first on your PATH";
    match found {
        None => "Homebrew finished, but no `arin` on your PATH reported a version, so this \
                 cannot say what is installed now. Check with `arin --version`."
            .to_owned(),
        Some(version) if version == current => {
            format!("`arin` is still {current}, not {target}. {lagging}.")
        }
        Some(version) if Version::parse(version).is_some_and(|v| v >= target) => {
            format!("Arin is now {version}.\n\n{AFTER_MOVING}")
        }
        Some(version) => {
            format!("`arin` is now {version}, not {target}. {lagging}.\n\n{AFTER_MOVING}")
        }
    }
}

/// Trade a versioned Homebrew path for the alias that outlives an upgrade.
///
/// Resolving the symlink on PATH lands in `<prefix>/Cellar/arin/<version>/`, and writing
/// that into the agent would break it on the next `brew upgrade`, because the version is
/// part of the path and the old keg is deleted. `<prefix>/opt/arin` is the same keg under a
/// name Homebrew re-points, so the agent survives.
///
/// Confirmed against the filesystem rather than trusted, so a path that merely contains a
/// directory called `Cellar` is left alone. That is also what makes it the test for whether
/// Homebrew owns a binary at all, which is why it lives here rather than in `service`.
pub(crate) fn unversioned(binary: PathBuf) -> PathBuf {
    let Some(alias) = homebrew_opt(&binary) else {
        return binary;
    };
    match (
        std::fs::canonicalize(&alias),
        std::fs::canonicalize(&binary),
    ) {
        (Ok(through_alias), Ok(resolved)) if through_alias == resolved => alias,
        _ => binary,
    }
}

/// Rewrite `<prefix>/Cellar/<name>/<version>/<rest>` as `<prefix>/opt/<name>/<rest>`.
fn homebrew_opt(binary: &Path) -> Option<PathBuf> {
    let parts: Vec<_> = binary.components().collect();
    let cellar = parts.iter().position(|part| part.as_os_str() == "Cellar")?;
    let name = parts.get(cellar + 1)?;
    // The version, which is the component being dropped. Absent means this is not a keg.
    parts.get(cellar + 2)?;

    let mut alias: PathBuf = parts[..cellar].iter().collect();
    alias.push("opt");
    alias.push(name);
    alias.extend(&parts[cellar + 3..]);
    Some(alias)
}

#[cfg(test)]
mod tests {
    use super::{
        CURRENT, Decision, Install, Scope, Version, decide, homebrew_opt, is_newer,
        on_this_machine, outcome, plan,
    };
    use std::path::{Path, PathBuf};

    fn version(raw: &str) -> Version {
        Version::parse(raw).expect("a test version parses")
    }

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

    /// The user's rule, and it is literal on 0.x: major is the first number, so 0.7 to 0.8 is
    /// an update and 1.0 is an upgrade.
    #[test]
    fn update_moves_within_the_major_version_and_stops_at_a_new_one() {
        for (latest, current) in [("0.7.1", "0.7.0"), ("v0.8.0", "0.7.0"), ("1.4.0", "1.3.9")] {
            assert_eq!(
                decide(latest, current, Scope::SameMajor),
                Decision::Move(version(latest)),
                "{current} to {latest} stays on the major version"
            );
        }
        for (latest, current) in [("1.0.0", "0.7.0"), ("v2.0.0", "1.9.9")] {
            assert_eq!(
                decide(latest, current, Scope::SameMajor),
                Decision::HeldBack(version(latest)),
                "{current} to {latest} crosses a major version"
            );
        }
    }

    /// `upgrade` is not a refusal when there is no new major version. It takes the newest
    /// release either way.
    #[test]
    fn upgrade_takes_the_newest_release_whatever_its_major_version() {
        for (latest, current) in [("1.0.0", "0.7.0"), ("0.8.0", "0.7.0"), ("0.7.1", "0.7.0")] {
            assert_eq!(
                decide(latest, current, Scope::Latest),
                Decision::Move(version(latest))
            );
        }
    }

    #[test]
    fn nothing_newer_is_up_to_date_on_either_command() {
        for scope in [Scope::SameMajor, Scope::Latest] {
            for latest in ["0.7.0", "v0.7.0", "0.6.9", "1.0.0-rc1", "nightly", ""] {
                assert_eq!(
                    decide(latest, "0.7.0", scope),
                    Decision::UpToDate,
                    "{latest:?} moves nothing"
                );
            }
        }
    }

    /// What this exists to prevent, written out: a path that looks like Homebrew's running
    /// `brew upgrade` on an install it has nothing to do with. The real keg is the integration
    /// test's, because it has to exist on disk to be believed.
    #[test]
    fn a_path_that_only_looks_like_homebrew_is_not_homebrew() {
        for (path, expected) in [
            (
                "/Users/dev/homebrew-tools/target/release/arin",
                Install::Unknown,
            ),
            ("/Users/dev/Cellar/arin/target/debug/arin", Install::Unknown),
            (
                "/Users/dev/Cellar/arin/0.7.0/Arin.app/Contents/MacOS/arin",
                Install::App,
            ),
            ("/opt/arin/Arin.app/Contents/MacOS/arin", Install::App),
        ] {
            assert_eq!(Install::of(Path::new(path), None), expected, "{path}");
        }
    }

    #[test]
    fn nix_is_the_store_itself_and_not_a_path_mentioning_it() {
        assert_eq!(
            Install::of(
                Path::new("/nix/store/abc-arin-0.7.0/Applications/Arin.app/Contents/MacOS/arin"),
                None
            ),
            Install::Nix
        );
        for path in ["/Users/dev/nix/store/arin", "/nix/store-backup/arin"] {
            assert_eq!(
                Install::of(Path::new(path), None),
                Install::Unknown,
                "{path}"
            );
        }
    }

    #[test]
    fn an_app_nothing_owns_is_told_about_the_dmg() {
        assert_eq!(
            Install::of(
                Path::new("/Applications/Arin.app/Contents/MacOS/arin"),
                None
            ),
            Install::App
        );
        let told = Install::App
            .by_hand(version("0.8.0"))
            .expect("no hand over");
        assert!(told.contains("releases/latest"), "{told}");
    }

    /// `arin` on crates.io is a library with no binary, and the bare name is the wrong thing to
    /// put in front of anybody. Cargo gets the documented route, pinned to the release.
    #[test]
    fn only_homebrew_is_handed_over_and_nothing_suggests_the_bare_crate() {
        let target = version("0.8.0");
        assert_eq!(Install::Homebrew.by_hand(target), None);
        for install in [Install::Nix, Install::Cargo, Install::App, Install::Unknown] {
            let told = install
                .by_hand(target)
                .expect("nothing to run for this one");
            assert!(told.contains("nothing was changed"), "{install:?}: {told}");
            assert!(!told.contains("cargo install arin"), "{install:?}: {told}");
        }
        let told = Install::Cargo.by_hand(target).expect("no hand over");
        assert!(told.contains("--tag v0.8.0 arin-cli"), "{told}");
    }

    /// `--check` is what `arin update` used to be, so the first line reads as it always did.
    #[test]
    fn a_check_names_the_method_the_target_and_the_exact_command() {
        let told = plan(
            &Decision::Move(version("0.8.0")),
            Install::Homebrew,
            "0.7.0",
        );
        assert!(
            told.starts_with("Arin 0.8.0 is out. You have 0.7.0."),
            "{told}"
        );
        assert!(told.contains("Homebrew"), "{told}");
        assert!(told.contains("Target          0.8.0"), "{told}");
        assert!(told.contains("brew upgrade anistark/tools/arin"), "{told}");
    }

    #[test]
    fn a_check_on_anything_else_would_run_nothing_and_says_what_to_do() {
        let told = plan(&Decision::Move(version("0.8.0")), Install::Nix, "0.7.0");
        assert!(told.contains("Would run       nothing"), "{told}");
        assert!(told.contains("nix profile upgrade arin"), "{told}");
        assert!(!told.contains("brew upgrade"), "{told}");
    }

    #[test]
    fn a_check_held_back_says_what_moves_past_it() {
        let told = plan(
            &Decision::HeldBack(version("1.0.0")),
            Install::Homebrew,
            "0.7.0",
        );
        assert!(told.contains("Would run       nothing"), "{told}");
        assert!(told.contains("`arin upgrade` moves to it"), "{told}");
        assert!(!told.contains("brew upgrade"), "nothing to run: {told}");
    }

    /// Being up to date is one line. Anything more reads as a problem when there is none.
    #[test]
    fn being_current_says_so_and_stops() {
        let told = plan(&Decision::UpToDate, Install::Homebrew, "0.2.1");
        assert_eq!(told, "Arin 0.2.1 is the latest.");
    }

    /// Homebrew finishing is not Arin having moved, so success is only ever claimed from what
    /// `arin` itself reports.
    #[test]
    fn success_is_claimed_only_from_the_version_read_back() {
        let target = version("0.8.0");

        let moved = outcome(Some("0.8.0"), target, "0.7.0");
        assert!(moved.starts_with("Arin is now 0.8.0."), "{moved}");
        assert!(moved.contains("arin service restart"), "{moved}");

        let lagging = outcome(Some("0.7.0"), target, "0.7.0");
        assert!(lagging.contains("still 0.7.0"), "{lagging}");
        assert!(!lagging.contains("is now"), "{lagging}");
        assert!(!lagging.contains("restart"), "nothing moved: {lagging}");

        let short = outcome(Some("0.7.5"), target, "0.7.0");
        assert!(short.contains("now 0.7.5, not 0.8.0"), "{short}");
        assert!(
            short.contains("arin service restart"),
            "it did move: {short}"
        );

        let unread = outcome(None, target, "0.7.0");
        assert!(unread.contains("arin --version"), "{unread}");
        assert!(!unread.contains("is now"), "{unread}");
    }

    /// The stand-in is a test seam. If it could name a host, the request could go somewhere
    /// other than GitHub, and the privacy page says it does not.
    #[test]
    fn the_stand_in_has_to_be_on_this_machine() {
        for raw in [
            "http://127.0.0.1:8080/latest",
            "http://[::1]:8080/latest",
            "http://localhost:8080/latest",
        ] {
            assert!(on_this_machine(raw).is_ok(), "{raw}");
        }
        for raw in [
            "https://example.com/latest",
            "http://localhost.example.com/latest",
            "http://127.0.0.1.example.com/latest",
            "not a url",
        ] {
            assert!(on_this_machine(raw).is_err(), "{raw}");
        }
    }

    /// Printed in every notice and compared against what `arin --version` says, so it has to
    /// spell a version the way the binary does.
    #[test]
    fn a_version_prints_without_its_tag_prefix() {
        assert_eq!(version("v0.8.0").to_string(), "0.8.0");
    }

    /// The upgrade path. A keg is deleted when the version it holds is replaced, so an
    /// agent pointing into one stops working at the next `brew upgrade`.
    #[test]
    fn a_versioned_keg_is_traded_for_the_alias_that_survives_an_upgrade() {
        assert_eq!(
            homebrew_opt(Path::new(
                "/opt/homebrew/Cellar/arin/0.4.1/Arin.app/Contents/MacOS/arin"
            )),
            Some(PathBuf::from(
                "/opt/homebrew/opt/arin/Arin.app/Contents/MacOS/arin"
            ))
        );

        // Intel, where the prefix is not /opt/homebrew.
        assert_eq!(
            homebrew_opt(Path::new(
                "/usr/local/Cellar/arin/0.9.0/Arin.app/Contents/MacOS/arin"
            )),
            Some(PathBuf::from(
                "/usr/local/opt/arin/Arin.app/Contents/MacOS/arin"
            ))
        );
    }

    #[test]
    fn a_path_that_is_not_a_keg_is_left_alone() {
        for path in [
            "/Applications/Arin.app/Contents/MacOS/arin",
            "/nix/store/abc123-arin-0.4.1/Applications/Arin.app/Contents/MacOS/arin",
            // A directory called Cellar with nothing under it is not a keg.
            "/opt/homebrew/Cellar",
        ] {
            assert_eq!(homebrew_opt(Path::new(path)), None, "rewrote {path}");
        }
    }

    /// A spent quota is not a refusal, and the message has to say which it is and when it
    /// clears, or the reader is left thinking Arin has been blocked.
    #[test]
    fn a_spent_rate_limit_says_so_and_when_it_resets() {
        let notice = super::rate_limited(Some(1_000 + 40 * 60), 1_000);
        assert!(notice.contains("rate limit"), "names the cause: {notice}");
        assert!(
            notice.contains("40 minutes"),
            "says when it clears: {notice}"
        );
        assert!(
            !notice.contains("403"),
            "the status is not the explanation: {notice}"
        );

        // Partial minutes round up rather than promising a reset that has not happened.
        assert!(super::rate_limited(Some(1_000 + 61), 1_000).contains("2 minutes"));
        assert!(super::rate_limited(Some(1_000 + 60), 1_000).contains("1 minute."));

        // A reset already in the past, or none sent, still gives the reader something to do.
        assert!(super::rate_limited(Some(500), 1_000).contains("try again"));
        assert!(super::rate_limited(None, 1_000).contains("an hour"));
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
