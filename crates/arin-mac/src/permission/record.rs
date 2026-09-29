//! What the daemon remembers about Screen Recording between runs.
//!
//! Two facts, each kept against the build it is about, because a grant is too. Whether this
//! build has asked macOS already, so the dialog comes up once per build rather than at every
//! start. And which build capture last worked for, which is the only thing that explains
//! System Settings showing Arin switched on while the permission reads as missing.
//!
//! Both live in `~/Library/Application Support/Arin`. Losing either costs one more dialog or
//! one explanation, and never a wrong answer about the permission, which is always read from
//! macOS.

use std::path::PathBuf;

/// Records that this build has asked macOS.
const ASKED: &str = "screen-recording-prompted";

/// Records the build capture last worked for.
const HOLDER: &str = "screen-recording-granted";

/// A build that capture worked for, as remembered by an earlier run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    /// The version it reported, which is what a person recognises.
    pub version: String,
    key: String,
}

impl Holder {
    /// The binary it ran from, which tells apart two builds of one version.
    pub fn binary(&self) -> &str {
        self.key
            .rsplit_once(' ')
            .map_or(self.key.as_str(), |(path, _)| path)
    }

    /// Why the permission is missing for this build, and how to move it here.
    ///
    /// Phrased so it stays true if the user switched the grant off since, which this record
    /// cannot know about.
    pub fn explain(&self) -> String {
        format!(
            "screen recording last worked for Arin {} at {}, which is not this build. macOS \
             keeps one Screen Recording grant for Arin and pins it to the exact build it was \
             given to, so System Settings can show Arin switched on while this build is \
             refused. To move the grant here, run `tccutil reset ScreenCapture {}`, then \
             choose Screen Recording from Arin's menu bar item, switch Arin on, and restart \
             Arin.",
            self.version,
            self.binary(),
            super::BUNDLE_ID,
        )
    }
}

/// Whether this build has already asked macOS for the permission.
pub fn asked() -> bool {
    read(ASKED).is_some_and(|recorded| recorded.trim() == build_key())
}

/// Note that this build has asked.
///
/// Called before asking rather than after, so a failure to remember costs one extra dialog
/// rather than one at every start, which is the failure this exists to stop.
pub fn remember_asked() {
    write(ASKED, &build_key());
}

/// The build capture last worked for, when that is not this one.
pub fn holder_elsewhere() -> Option<Holder> {
    elsewhere(parse(&read(HOLDER)?)?, &build_key())
}

/// Note that capture works for this build.
pub fn remember_holder() {
    write(
        HOLDER,
        &format!("{}\n{}", env!("CARGO_PKG_VERSION"), build_key()),
    );
}

fn parse(recorded: &str) -> Option<Holder> {
    let (version, key) = recorded.trim().split_once('\n')?;
    Some(Holder {
        version: version.trim().to_owned(),
        key: key.trim().to_owned(),
    })
}

fn elsewhere(holder: Holder, this_build: &str) -> Option<Holder> {
    (holder.key != this_build).then_some(holder)
}

/// What identifies this build for the purpose of the permission.
///
/// The binary's path and modification time rather than its `cdhash`, which is what macOS
/// actually compares and would cost a `codesign` run on a path that is meant to be cheap.
/// Both change when a build is replaced, which is the only property needed.
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

fn path(name: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join("Library/Application Support/Arin")
            .join(name),
    )
}

fn read(name: &str) -> Option<String> {
    std::fs::read_to_string(path(name)?).ok()
}

fn write(name: &str, contents: &str) {
    let Some(path) = path(name) else {
        return;
    };
    let written = path
        .parent()
        .map(std::fs::create_dir_all)
        .transpose()
        .and_then(|_| std::fs::write(&path, contents));
    if let Err(e) = written {
        // Not worth a warning. The cost is one more dialog or one explanation, and whoever
        // is reading the log has a real problem in front of them already.
        tracing::debug!(error = %e, path = %path.display(), "could not record the permission");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key that did not change between two builds would let a replaced binary inherit the
    /// old one's record, which is the mistake this whole module is here to avoid.
    #[test]
    fn the_build_key_names_the_running_binary() {
        let key = build_key();
        assert!(key.contains(char::is_whitespace), "no timestamp in {key}");
        assert_eq!(key, build_key(), "the key has to be stable within a run");
    }

    #[test]
    fn a_holder_names_its_version_and_binary() {
        let holder =
            parse("0.6.0\n/opt/homebrew/opt/arin/Arin.app/Contents/MacOS/arin 1789000000\n")
                .expect("a holder");
        assert_eq!(holder.version, "0.6.0");
        assert_eq!(
            holder.binary(),
            "/opt/homebrew/opt/arin/Arin.app/Contents/MacOS/arin"
        );
    }

    /// What an upgrade looks like from here. Homebrew keeps the path through `opt`, so only
    /// the modification time says the binary under it was replaced.
    #[test]
    fn only_a_different_build_holds_the_grant_elsewhere() {
        let holder = parse("0.6.0\n/opt/homebrew/opt/arin/Arin.app/Contents/MacOS/arin 100")
            .expect("a holder");

        let same = "/opt/homebrew/opt/arin/Arin.app/Contents/MacOS/arin 100";
        assert_eq!(elsewhere(holder.clone(), same), None);

        let upgraded = "/opt/homebrew/opt/arin/Arin.app/Contents/MacOS/arin 200";
        assert_eq!(elsewhere(holder.clone(), upgraded), Some(holder));
    }

    /// The switch reads as on, so the message has to say why that does not help and hand
    /// over the command that does, rather than send anyone back to the switch.
    #[test]
    fn a_grant_held_elsewhere_names_the_build_and_the_fix() {
        let explained = parse("0.6.0\n/opt/homebrew/opt/arin/Arin.app/Contents/MacOS/arin 100")
            .expect("a holder")
            .explain();
        assert!(explained.contains("Arin 0.6.0"), "{explained}");
        assert!(
            explained.contains("tccutil reset ScreenCapture com.anistark.arin"),
            "{explained}"
        );
        assert!(explained.contains("menu bar"), "{explained}");
    }

    #[test]
    fn a_record_that_is_not_a_holder_is_ignored() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("0.6.0"), None);
    }
}
