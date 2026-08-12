//! Whether this build has an identity a Screen Recording grant can stick to.
//!
//! # The failure this exists to name
//!
//! TCC does not remember "Arin". It remembers a code signature, and it checks the running
//! process against the one it stored. A build whose signature does not verify has nothing
//! for it to store, so the grant either never persists or persists against something that
//! is no longer what is running. Either way the daemon comes up reporting the permission as
//! missing, on a machine where System Settings shows Arin sitting in the list.
//!
//! That is unfalsifiable from the user's side. They granted it, the row is there, the switch
//! looks on, and every start throws them back into System Settings. It cost a day before it
//! was understood on 2026-08-11, and the cause was two lines of packaging: `bundle.sh` only
//! ran `codesign` when it was handed an identity, so every install built from source shipped
//! a bundle carrying nothing but the linker's ad-hoc signature. `codesign --verify` on it
//! fails outright with "code has no resources but signature indicates they must be present",
//! its identifier is the linker's `arin-<hash>` rather than `com.anistark.arin`, and its
//! Info.plist is not bound to the signature at all.
//!
//! # Why this shells out to codesign
//!
//! Answering it properly means `SecStaticCodeCheckValidity`, which no crate in the tree
//! binds and which would mean taking the whole Security framework as a dependency to run one
//! check on one failure path. `codesign` ships with macOS, is the tool whose output every
//! report about this will quote, and runs once per process behind a [`OnceLock`]. When
//! something in the tree needs Security for another reason, this should move onto it.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// What macOS can pin a permission to for this build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    /// Signed with a certificate, so the designated requirement names the team.
    ///
    /// The only shape whose grant survives an upgrade. A new build signed by the same team
    /// satisfies the stored requirement, so the user grants Arin once and never again.
    Certified {
        /// The signing identifier, which for a correctly built bundle is its bundle id.
        identifier: String,
        /// The team the certificate belongs to.
        team: String,
    },
    /// A valid ad-hoc signature, which pins the grant to this exact binary.
    ///
    /// The designated requirement is a bare `cdhash`, so the grant holds across restarts and
    /// is void the moment the binary changes. Good enough that a daemon stops asking every
    /// time it starts, and not good enough to survive `brew upgrade`.
    AdHoc {
        /// The signing identifier, which for a correctly built bundle is its bundle id.
        identifier: String,
    },
    /// Nothing a grant can be kept against.
    Unstable(Fault),
}

/// Why a build has no identity to hold a permission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// Running as a bare binary rather than from an app bundle.
    ///
    /// Expected for `cargo run`, and the reason the launch agent is pointed inside the
    /// bundle rather than at the symlink on PATH.
    NotBundled,
    /// There is a bundle and its signature does not verify.
    Unsealed {
        /// What `codesign --verify` said, which is the line worth quoting in a bug report.
        reason: String,
    },
}

impl Identity {
    /// One line saying what is wrong and what clears it.
    pub fn explain(&self) -> String {
        match self {
            Self::Certified { identifier, team } => {
                format!("signed as {identifier} by team {team}, so a grant survives upgrades")
            }
            Self::AdHoc { identifier } => format!(
                "ad-hoc signed as {identifier}, so a grant holds until this binary changes \
                 and has to be given again after an upgrade"
            ),
            Self::Unstable(Fault::NotBundled) => {
                "screen recording is not granted, and this is a bare binary rather than Arin.app, \
                 so macOS has no bundle to attach a grant to. Whatever you grant will be \
                 attributed to whatever started this. Run the binary inside Arin.app."
                    .into()
            }
            Self::Unstable(Fault::Unsealed { reason }) => format!(
                "screen recording cannot be granted to this build. Its signature does not \
                 verify ({reason}), so macOS has no identity to remember a grant against and \
                 will keep reporting the permission as missing however many times you switch \
                 it on. Fix the build rather than the setting:\n  \
                 codesign --force --sign - /path/to/Arin.app\n  \
                 tccutil reset ScreenCapture com.anistark.arin\n\
                 then start Arin and grant it once more."
            ),
        }
    }

    /// Whether a grant made against this build will still be there at the next start.
    pub fn holds_a_grant(&self) -> bool {
        !matches!(self, Self::Unstable(_))
    }
}

/// What macOS makes of the running build. Answered once per process.
pub fn of_this_build() -> Identity {
    static IDENTITY: OnceLock<Identity> = OnceLock::new();
    IDENTITY
        .get_or_init(|| match bundle_of_running_binary() {
            Some(bundle) => inspect(&bundle),
            None => Identity::Unstable(Fault::NotBundled),
        })
        .clone()
}

/// The `.app` the running binary sits inside.
///
/// Deliberately does not resolve symlinks. A Homebrew install is reached through
/// `<prefix>/opt/arin`, and resolving that lands in the versioned keg, which is a different
/// path for the same bundle and not the one anything else in Arin talks about.
fn bundle_of_running_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let macos = exe.parent()?;
    (macos.file_name()? == "MacOS").then_some(())?;
    let contents = macos.parent()?;
    (contents.file_name()? == "Contents").then_some(())?;
    let app = contents.parent()?;
    (app.extension()? == "app").then_some(())?;
    Some(app.to_path_buf())
}

fn inspect(bundle: &Path) -> Identity {
    // `--strict` because the default is willing to overlook exactly the kind of damage this
    // is looking for. Without it a bundle with no sealed resources can still pass.
    let verified = Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict"])
        .arg(bundle)
        .output();

    match verified {
        Ok(output) if !output.status.success() => {
            return Identity::Unstable(Fault::Unsealed {
                reason: first_line(&output.stderr, bundle),
            });
        }
        // codesign is part of macOS, so failing to run it means something is wrong that this
        // check is not equipped to describe. Saying nothing beats inventing a diagnosis.
        Err(e) => {
            tracing::debug!(error = %e, "could not run codesign, skipping the identity check");
            return Identity::AdHoc {
                identifier: "unknown".into(),
            };
        }
        Ok(_) => {}
    }

    let described = Command::new("/usr/bin/codesign")
        .args(["--display", "--verbose=2"])
        .arg(bundle)
        .output();
    // codesign writes the description to stderr, not stdout.
    let described = described
        .map(|output| String::from_utf8_lossy(&output.stderr).into_owned())
        .unwrap_or_default();

    let identifier = field(&described, "Identifier")
        .unwrap_or("unknown")
        .to_owned();
    match field(&described, "TeamIdentifier") {
        Some(team) if team != "not set" => Identity::Certified {
            identifier,
            team: team.to_owned(),
        },
        _ => Identity::AdHoc { identifier },
    }
}

/// Read one `Key=value` out of what `codesign --display` printed.
fn field<'a>(described: &'a str, key: &str) -> Option<&'a str> {
    described.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        (name.trim() == key).then(|| value.trim())
    })
}

/// The useful line of a codesign failure, with the path it repeats trimmed off.
fn first_line(stderr: &[u8], bundle: &Path) -> String {
    let text = String::from_utf8_lossy(stderr);
    let line = text.lines().next().unwrap_or("it would not say why").trim();
    line.strip_prefix(&format!("{}: ", bundle.display()))
        .unwrap_or(line)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_is_read_from_what_codesign_prints() {
        let described = "\
Executable=/opt/homebrew/opt/arin/Arin.app/Contents/MacOS/arin
Identifier=com.anistark.arin
Format=app bundle with Mach-O thin (arm64)
Signature=adhoc
TeamIdentifier=not set
";
        assert_eq!(field(described, "Identifier"), Some("com.anistark.arin"));
        assert_eq!(field(described, "TeamIdentifier"), Some("not set"));
        assert_eq!(field(described, "Nothing"), None);
    }

    /// codesign repeats the path back before the reason, and the path is already known to
    /// whoever is reading the message.
    #[test]
    fn a_failure_reason_drops_the_path_codesign_repeats() {
        let bundle = Path::new("/opt/homebrew/opt/arin/Arin.app");
        let stderr = b"/opt/homebrew/opt/arin/Arin.app: code has no resources but signature indicates they must be present\n";
        assert_eq!(
            first_line(stderr, bundle),
            "code has no resources but signature indicates they must be present"
        );
    }

    #[test]
    fn every_identity_says_something_and_only_a_signed_one_holds_a_grant() {
        let cases = [
            Identity::Certified {
                identifier: "com.anistark.arin".into(),
                team: "ABCDE12345".into(),
            },
            Identity::AdHoc {
                identifier: "com.anistark.arin".into(),
            },
            Identity::Unstable(Fault::NotBundled),
            Identity::Unstable(Fault::Unsealed {
                reason: "code has no resources".into(),
            }),
        ];
        for identity in &cases {
            assert!(
                !identity.explain().is_empty(),
                "{identity:?} explains nothing"
            );
        }

        assert!(cases[0].holds_a_grant());
        assert!(cases[1].holds_a_grant());
        assert!(!cases[2].holds_a_grant());
        assert!(!cases[3].holds_a_grant());
    }

    /// The unsealed case is the one nobody can guess, so it has to hand over the two commands
    /// that clear it rather than describing them.
    #[test]
    fn the_unsealed_message_carries_the_commands_that_fix_it() {
        let explained = Identity::Unstable(Fault::Unsealed {
            reason: "code has no resources".into(),
        })
        .explain();

        assert!(
            explained.contains("codesign --force --sign -"),
            "{explained}"
        );
        assert!(
            explained.contains("tccutil reset ScreenCapture com.anistark.arin"),
            "{explained}"
        );
    }

    /// Whatever this machine is running, the check has to produce an answer rather than
    /// panic or hang, and `cargo test` runs from a bare binary.
    #[test]
    fn the_running_build_gets_an_answer() {
        assert_eq!(of_this_build(), Identity::Unstable(Fault::NotBundled));
    }
}
