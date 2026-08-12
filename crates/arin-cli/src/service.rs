//! `arin service`: the launch agent, managed from the binary the agent runs.
//!
//! The daemon runs in the foreground and stops on Ctrl-C, deliberately, so having Arin
//! there after a reboot means a launchd agent. This installs one.
//!
//! # Why this is not the shell script it replaces
//!
//! `packaging/macos/launch-agent.sh` did the same job and had to be told where `Arin.app`
//! was, because a shell script cannot ask. Everything that went wrong with it went wrong
//! there: the Homebrew invocation in the docs omitted the path, so the agent was written
//! against `/Applications/Arin.app`, which on a machine with both installs is a different
//! build of Arin than the one you asked for. A process can find its own bundle, so this
//! cannot make that mistake.
//!
//! # Why launchctl rather than a crate
//!
//! The plist is generated from `packaging/macos/com.anistark.arin.plist`, the same file the
//! bundlers install and the nix-darwin module is checked against, so there is nothing here
//! to build a plist with. What is left is `bootstrap`, `bootout` and `kickstart`, which no
//! crate wraps and which have no API besides the tool.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::cli::{BUNDLE_ID, ServiceAction};

/// The launch agent, from the file the bundlers install and the Nix module tracks.
///
/// Included rather than rebuilt so there is one description of the agent. The plist is the
/// reference, and a second definition in Rust would be a copy to keep in step with it and
/// with `packaging/nix/darwin-module.nix`.
const TEMPLATE: &str = include_str!("../../../packaging/macos/com.anistark.arin.plist");

pub(crate) fn run(action: ServiceAction) -> Result<()> {
    match action {
        ServiceAction::Enable { app } => enable(app.as_deref()),
        ServiceAction::Disable => disable(),
        ServiceAction::Status => status(),
        ServiceAction::Restart => restart(),
    }
}

fn enable(app: Option<&Path>) -> Result<()> {
    let binary = match app {
        Some(app) => in_bundle(&app.join("Contents/MacOS/arin"))
            .with_context(|| format!("{} is not an Arin.app with a binary in it", app.display()))?,
        None => locate()?,
    };

    let plist = plist_path()?;
    refuse_to_fight_over(&plist)?;

    let logs = log_dir()?;
    std::fs::create_dir_all(&logs)
        .with_context(|| format!("could not create {}", logs.display()))?;
    if let Some(parent) = plist.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    std::fs::write(&plist, render(&binary, &logs))
        .with_context(|| format!("could not write {}", plist.display()))?;

    // Booted out first so that enabling is repeatable. Re-running this after the app moved
    // should replace the agent rather than fail on one already loaded.
    let _ = launchctl(&["bootout".as_ref(), target().as_ref()]);
    let bootstrap = launchctl(&["bootstrap".as_ref(), domain().as_ref(), plist.as_os_str()])?;
    if !bootstrap.status.success() {
        bail!(
            "launchctl would not load {}: {}",
            plist.display(),
            message(&bootstrap)
        );
    }
    let _ = launchctl(&["enable".as_ref(), target().as_ref()]);

    println!("Arin starts at login, from {}", binary.display());
    println!("Logs: {}", logs.join("arin.log").display());
    println!();
    println!("The Screen Recording grant belongs to this bundle. If you replace the app with");
    println!("a build macOS sees as a different one, grant it again.");
    Ok(())
}

fn disable() -> Result<()> {
    let plist = plist_path()?;
    refuse_to_fight_over(&plist)?;

    let _ = launchctl(&["bootout".as_ref(), target().as_ref()]);
    match std::fs::remove_file(&plist) {
        Ok(()) => println!("Arin no longer starts at login. The app itself is untouched."),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("Arin was not starting at login. Nothing to remove.");
        }
        Err(e) => {
            return Err(e).with_context(|| format!("could not remove {}", plist.display()));
        }
    }
    Ok(())
}

/// Report the agent, and exit non-zero when there is not one.
///
/// The exit code is the point as much as the output is. `arin permissions` gates a script
/// the same way, and the question "is Arin set up to come back after a reboot" is one a
/// setup script wants to ask without reading prose.
fn status() -> Result<()> {
    let plist = plist_path()?;
    let printed = launchctl(&["print".as_ref(), target().as_ref()])?;
    if !printed.status.success() {
        println!("not enabled. Run: arin service enable");
        if plist.exists() {
            println!();
            println!(
                "There is a plist at {}, so the agent was installed and",
                plist.display()
            );
            println!("then unloaded. Enabling again will load it.");
        }
        std::process::exit(1);
    }

    let printed = String::from_utf8_lossy(&printed.stdout);
    println!("enabled");
    for key in ["state", "program", "pid", "last exit code"] {
        if let Some(value) = field(&printed, key) {
            println!("{key:<15}{value}");
        }
    }

    // launchd does not create the directory it is told to write to, and says nothing when it
    // cannot. `enable` above creates it, so a missing one means this agent was installed by
    // something else, and every line the daemon has ever logged went nowhere. That is worse
    // than it sounds: `arin permissions` sends people to this file to find out whether the
    // daemon can capture, because from a terminal there is no other honest answer.
    let logs = log_dir()?;
    if !logs.is_dir() {
        println!();
        println!("The log directory does not exist, so launchd is discarding the daemon's");
        println!("output: {}", logs.display());
        println!("`arin service enable` creates it and reloads the agent.");
    }
    Ok(())
}

fn restart() -> Result<()> {
    let kicked = launchctl(&["kickstart".as_ref(), "-k".as_ref(), target().as_ref()])?;
    if !kicked.status.success() {
        bail!(
            "launchctl would not restart the agent: {}\n\
             If it is not installed, `arin service enable` installs it.",
            message(&kicked)
        );
    }
    println!("Restarted. The daemon is the build the agent points at, which after an");
    println!("upgrade is the new one.");
    Ok(())
}

/// Read one `key = value` out of `launchctl print`.
///
/// The first match wins, which is what picks the service's own `state` over the `state` of
/// each endpoint nested below it. launchctl prints the service before its endpoints.
fn field<'a>(printed: &'a str, key: &str) -> Option<&'a str> {
    printed.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        (name.trim() == key).then(|| value.trim())
    })
}

/// The binary inside the bundle, found from the one that is running.
///
/// The agent has to name the binary inside `Arin.app` rather than a symlink to it on PATH.
/// macOS attributes the Screen Recording grant to the bundle, and an agent that ran a bare
/// binary would come up unable to see the screen and unable to explain why.
fn locate() -> Result<PathBuf> {
    let running = std::env::current_exe().context("could not find the running binary")?;

    // The invocation path first, and only then the resolved one. They differ for a symlink
    // on PATH, and the unresolved path is the better answer whenever it is a real answer.
    if let Ok(binary) = in_bundle(&running) {
        return Ok(binary);
    }

    let resolved = std::fs::canonicalize(&running)
        .with_context(|| format!("could not resolve {}", running.display()))?;
    let binary = in_bundle(&resolved).with_context(|| {
        format!(
            "{} is not inside Arin.app, so there is no bundle for the agent to start.\n\n\
             The Screen Recording grant belongs to a bundle rather than to a path, so an \
             agent running a bare binary would come up unable to see the screen.\n\n\
             Install the app with `brew install anistark/tools/arin`, or build one from a \
             clone with `just bundle` and name it:\n  \
             arin service enable --app path/to/Arin.app",
            running.display()
        )
    })?;

    Ok(unversioned(binary))
}

/// Accept a path only if it really is the binary inside an app bundle.
fn in_bundle(binary: &Path) -> Result<PathBuf> {
    let layout = || -> Option<()> {
        let macos = binary.parent()?;
        (macos.file_name()? == "MacOS").then_some(())?;
        let contents = macos.parent()?;
        (contents.file_name()? == "Contents").then_some(())?;
        (contents.parent()?.extension()? == "app").then_some(())
    };
    match layout() {
        Some(()) if binary.is_file() => Ok(binary.to_path_buf()),
        Some(()) => bail!("{} does not exist", binary.display()),
        None => bail!("{} is not Contents/MacOS inside an .app", binary.display()),
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
/// directory called `Cellar` is left alone.
fn unversioned(binary: PathBuf) -> PathBuf {
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

/// Refuse to write over an agent something else is managing.
///
/// nix-darwin writes `services.arin.enable` to this same path under this same label, and
/// two managers of one agent is a state where whichever ran last wins and neither says so.
fn refuse_to_fight_over(plist: &Path) -> Result<()> {
    if !nix_managed(plist) {
        return Ok(());
    }
    bail!(
        "{} is managed by nix-darwin.\n\n\
         It installs the same agent under the same label, so a second manager would leave \
         two definitions of a single agent and whichever ran last would win. This command \
         will not touch it.\n\n\
         Turn it on and off with `services.arin.enable` in your nix-darwin configuration, \
         which is this agent with the daemon's command line under version control. To hand \
         the job to this command instead, unset that option, rebuild, and run this again.",
        plist.display()
    )
}

/// Whether the agent already installed came from Nix.
///
/// The store path is the signal, and it survives both shapes nix-darwin has used: a symlink
/// into the store, and a real file whose `ProgramArguments` name a binary there. No other
/// install route can produce either.
fn nix_managed(plist: &Path) -> bool {
    const STORE: &str = "/nix/store";

    if std::fs::symlink_metadata(plist).is_err() {
        return false;
    }
    if std::fs::read_link(plist).is_ok_and(|target| target.starts_with(STORE)) {
        return true;
    }
    std::fs::read_to_string(plist).is_ok_and(|contents| contents.contains(STORE))
}

fn render(binary: &Path, logs: &Path) -> String {
    TEMPLATE
        .replace("@ARIN_BINARY@", &escape(&binary.to_string_lossy()))
        .replace("@LOG_DIR@", &escape(&logs.to_string_lossy()))
}

/// Make a path safe to drop into the plist's XML.
///
/// `&` and `<` are legal in a macOS path and would produce a plist launchd refuses to read,
/// which is a failure that shows up at the next login rather than here.
fn escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn plist_path() -> Result<PathBuf> {
    Ok(home()?.join(format!("Library/LaunchAgents/{BUNDLE_ID}.plist")))
}

fn log_dir() -> Result<PathBuf> {
    Ok(home()?.join("Library/Logs/Arin"))
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set, so there is no LaunchAgents directory to write to")
}

/// The user's GUI domain, which is where a login agent lives.
///
/// Not `user/<uid>`, which is loaded before login and has no window server to draw on.
fn domain() -> String {
    format!("gui/{}", arin_core::peer::current_uid())
}

fn target() -> String {
    format!("{}/{BUNDLE_ID}", domain())
}

fn launchctl(args: &[&std::ffi::OsStr]) -> Result<Output> {
    Command::new("launchctl")
        .args(args)
        .output()
        .context("could not run launchctl")
}

/// Whatever launchctl said about a failure, which is often only on one of the two streams.
fn message(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let text = if stderr.trim().is_empty() {
        String::from_utf8_lossy(&output.stdout)
    } else {
        stderr
    };
    match (text.trim(), output.status.code()) {
        ("", Some(code)) => format!("it said nothing and exited {code}"),
        ("", None) => "it said nothing and was killed by a signal".to_owned(),
        (said, _) => said.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The label in the template is what every `launchctl` call here names. If they drift,
    /// `enable` installs one agent and `status` asks about a different one that is not
    /// there, so both look broken while each is doing what it was told.
    #[test]
    fn the_template_and_the_commands_agree_on_the_label() {
        assert!(
            TEMPLATE.contains(&format!("<string>{BUNDLE_ID}</string>")),
            "the plist template does not declare {BUNDLE_ID} as its label"
        );
        assert!(target().ends_with(BUNDLE_ID));
    }

    #[test]
    fn rendering_fills_in_every_placeholder() {
        let rendered = render(
            Path::new("/Applications/Arin.app/Contents/MacOS/arin"),
            Path::new("/Users/someone/Library/Logs/Arin"),
        );

        assert!(
            !rendered.contains('@'),
            "a placeholder survived:\n{rendered}"
        );
        assert!(rendered.contains("<string>/Applications/Arin.app/Contents/MacOS/arin</string>"));
        assert!(rendered.contains("/Users/someone/Library/Logs/Arin/arin.log"));
    }

    /// An ampersand is legal in a macOS path and fatal in XML. launchd would refuse the
    /// plist at the next login, a long way from anything that would explain it.
    #[test]
    fn a_path_that_is_not_xml_safe_is_escaped() {
        let rendered = render(
            Path::new("/Users/a&b/Arin.app/Contents/MacOS/arin"),
            Path::new("/Users/a&b/Logs"),
        );

        assert!(
            rendered.contains("/Users/a&amp;b/Arin.app"),
            "got:\n{rendered}"
        );
        assert!(
            !rendered.contains("/Users/a&b/"),
            "the raw ampersand reached the plist:\n{rendered}"
        );
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

    #[test]
    fn the_bundle_layout_is_recognised_and_a_bare_binary_is_not() {
        // Rejected on shape, before existence is considered.
        for path in [
            "/opt/homebrew/bin/arin",
            "/Applications/Arin.app/Contents/arin",
            "/Applications/Arin.app/MacOS/arin",
            "/Applications/Arin/Contents/MacOS/arin",
        ] {
            let error = in_bundle(Path::new(path)).expect_err("{path} is not in a bundle");
            assert!(
                error.to_string().contains("is not Contents/MacOS"),
                "{path} was refused for the wrong reason: {error}"
            );
        }

        // Right shape, nothing there. A different failure, and it has to say so.
        let error = in_bundle(Path::new(
            "/Applications/NoSuchArin.app/Contents/MacOS/arin",
        ))
        .expect_err("the bundle does not exist");
        assert!(error.to_string().contains("does not exist"), "got: {error}");
    }

    #[test]
    fn a_field_is_read_from_what_launchctl_prints() {
        let printed = "\
com.anistark.arin = {
	path = /Users/someone/Library/LaunchAgents/com.anistark.arin.plist
	state = running
	program = /opt/homebrew/opt/arin/Arin.app/Contents/MacOS/arin
	pid = 35284
	last exit code = (never exited)
	pid-local endpoints = {
		state = active
	}
}
";
        assert_eq!(field(printed, "state"), Some("running"));
        assert_eq!(field(printed, "pid"), Some("35284"));
        assert_eq!(field(printed, "last exit code"), Some("(never exited)"));
        assert_eq!(field(printed, "nothing here"), None);
    }

    /// The nested `state` under an endpoint is not the service's state, and reporting
    /// `active` for a job that is not running would be worse than reporting nothing.
    #[test]
    fn the_services_own_state_wins_over_a_nested_one() {
        let printed = "\
	state = not running
	pid-local endpoints = {
		state = active
	}
";
        assert_eq!(field(printed, "state"), Some("not running"));
    }

    #[test]
    fn an_agent_that_is_not_there_is_not_mistaken_for_a_nix_one() {
        assert!(!nix_managed(Path::new(
            "/nonexistent/Library/LaunchAgents/com.anistark.arin.plist"
        )));
    }
}
