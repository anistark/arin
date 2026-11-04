//! `arin update` and `arin upgrade`, run for real.
//!
//! The real binary is copied to where an install would put it, so what is detected is what a
//! user would get. `brew` and the `arin` it leaves behind are shell scripts alone on PATH that
//! log what they were asked, and releases come from a stub on loopback through the one
//! environment variable that can name one. Nothing here runs a package manager, and nothing
//! reaches a copy of Arin installed on this machine: PATH and HOME both point inside the test.
//!
//! Versions are worked out from the package version, so a release bump changes nothing here.
#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const CURRENT: &str = env!("CARGO_PKG_VERSION");

fn parts() -> [u64; 3] {
    let parsed: Vec<u64> = CURRENT
        .split('.')
        .map(|part| part.parse().expect("the package version is numeric"))
        .collect();
    parsed
        .try_into()
        .expect("the package version has three parts")
}

fn next_patch() -> String {
    let [major, minor, patch] = parts();
    format!("{major}.{minor}.{}", patch + 1)
}

fn next_minor() -> String {
    let [major, minor, _] = parts();
    format!("{major}.{}.0", minor + 1)
}

fn next_major() -> String {
    let [major, _, _] = parts();
    format!("{}.0.0", major + 1)
}

/// A directory of its own for one test, gone when it finishes.
///
/// Under cargo's target directory rather than the system's, so a copy of the binary is a clone
/// on the same volume rather than tens of megabytes per test.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("update-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the test directory");
        Self(path.canonicalize().expect("resolve the test directory"))
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }

    /// A `tool` alone on a PATH of its own, logging its arguments to the path returned.
    fn logging_tool(&self, tool: &str) -> PathBuf {
        let log = self.path(&format!("{tool}.log"));
        write_script(
            &self.path(&format!("tools/{tool}")),
            &format!("echo \"$*\" >> '{}'", log.display()),
        );
        log
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Put a copy of the real binary at `path`.
///
/// A copy rather than a hard link, which would be quicker. macOS verifies a file the first time
/// it runs, at about a second a copy, but links share one file between tests that start it, and
/// remove it, at the same moment, and macOS SIGKILLs some of those processes.
fn install_binary(path: &Path) {
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("create the install path");
    std::fs::copy(env!("CARGO_BIN_EXE_arin"), path).expect("copy the binary");
}

fn write_script(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("create the tool directory");
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write the script");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

/// The layout the formula really leaves: `bin/arin` links into the keg, and `opt/arin` is the
/// alias Homebrew re-points.
struct Homebrew {
    scratch: Scratch,
}

impl Homebrew {
    fn new(name: &str) -> Self {
        let scratch = Scratch::new(name);
        let keg = format!("homebrew/Cellar/arin/{CURRENT}");
        install_binary(&scratch.path(&format!("{keg}/Arin.app/Contents/MacOS/arin")));

        std::fs::create_dir_all(scratch.path("homebrew/opt")).expect("create opt");
        std::fs::create_dir_all(scratch.path("homebrew/bin")).expect("create bin");
        symlink(
            format!("../Cellar/arin/{CURRENT}"),
            scratch.path("homebrew/opt/arin"),
        )
        .expect("link opt");
        symlink(
            format!("../Cellar/arin/{CURRENT}/Arin.app/Contents/MacOS/arin"),
            scratch.path("homebrew/bin/arin"),
        )
        .expect("link bin");
        Self { scratch }
    }

    /// A `brew` that logs its arguments, then does `then`.
    fn brew(&self, then: &str) {
        write_script(
            &self.scratch.path("homebrew/bin/brew"),
            &format!("echo \"$*\" >> '{}'\n{then}", self.log_path().display()),
        );
    }

    /// A `brew` that does what an upgrade does to the things this can see: removes the keg
    /// the running process started from, and leaves a new `arin` reporting `version`.
    fn brew_installing(&self, version: &str) {
        let bin = self.scratch.path("homebrew/bin/arin");
        let cellar = self.scratch.path("homebrew/Cellar/arin");
        self.brew(&format!(
            "/bin/rm -rf '{cellar}' '{bin}'\n\
             echo '#!/bin/sh' > '{bin}'\n\
             echo 'echo arin {version}' >> '{bin}'\n\
             /bin/chmod +x '{bin}'",
            cellar = cellar.display(),
            bin = bin.display(),
        ));
    }

    fn log_path(&self) -> PathBuf {
        self.scratch.path("brew.log")
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.log_path())
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn command(&self) -> Command {
        command(
            &self.scratch.path("homebrew/bin/arin"),
            &self.scratch.path("homebrew/bin"),
            &self.scratch.0,
        )
    }

    fn run(&self, stub: &Stub, args: &[&str]) -> Output {
        self.command()
            .args(args)
            .env("ARIN_RELEASES_API", &stub.url)
            .output()
            .expect("run arin")
    }
}

/// A binary with nothing inherited but what is set here.
fn command(binary: &Path, path: &Path, home: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .env_clear()
        .env("PATH", path)
        .env("HOME", home)
        .stdin(Stdio::null());
    command
}

/// Run a copy that is not in a keg, with `tools` as its whole PATH.
fn run_copy(scratch: &Scratch, binary: &Path, stub: &Stub, args: &[&str]) -> Output {
    command(binary, &scratch.path("tools"), &scratch.0)
        .args(args)
        .env("ARIN_RELEASES_API", &stub.url)
        .output()
        .expect("run arin")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Everything the command said and how it ended, for failures that have to show all of it.
fn said(output: &Output) -> String {
    format!("{}\n{}{}", output.status, stdout(output), stderr(output))
}

/// A stand-in for GitHub's API, counting what it is asked.
struct Stub {
    url: String,
    requests: Arc<AtomicUsize>,
}

impl Stub {
    fn answering(response: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the stub");
        let url = format!(
            "http://{}/repos/anistark/arin/releases/latest",
            listener.local_addr().expect("the stub's address")
        );
        let requests = Arc::new(AtomicUsize::new(0));
        let counted = requests.clone();

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut request = Vec::new();
                let mut buffer = [0u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    match stream.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => request.extend_from_slice(&buffer[..read]),
                    }
                }
                counted.fetch_add(1, Ordering::SeqCst);
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self { url, requests }
    }

    fn release(version: &str) -> Self {
        let body = format!(r#"{{"tag_name":"v{version}"}}"#);
        Self::answering(format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ))
    }

    fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

#[test]
fn update_hands_over_to_homebrew_and_reads_back_the_version_it_left() {
    let target = next_minor();
    let brew = Homebrew::new("hands-over");
    brew.brew_installing(&target);
    let stub = Stub::release(&target);

    let output = brew.run(&stub, &["update"]);

    assert!(output.status.success(), "{}", said(&output));
    assert_eq!(brew.calls(), ["upgrade anistark/tools/arin"]);
    let told = stdout(&output);
    assert!(told.contains(&format!("Arin is now {target}.")), "{told}");
    assert!(told.contains("arin service restart"), "{told}");
    assert_eq!(stub.requests(), 1, "one request, however it goes");
}

#[test]
fn a_check_on_either_command_changes_nothing() {
    let target = next_patch();
    let brew = Homebrew::new("check");
    brew.brew_installing(&target);
    let stub = Stub::release(&target);

    for command in ["update", "upgrade"] {
        let output = brew.run(&stub, &[command, "--check"]);
        assert!(output.status.success(), "{}", said(&output));
        let told = stdout(&output);
        assert!(told.contains("Homebrew"), "{told}");
        assert!(told.contains(&target), "{told}");
        assert!(told.contains("brew upgrade anistark/tools/arin"), "{told}");
    }
    assert!(brew.calls().is_empty(), "--check ran {:?}", brew.calls());
    assert_eq!(stub.requests(), 2, "one request each");
}

#[test]
fn update_holds_back_at_a_new_major_version_and_points_at_upgrade() {
    let target = next_major();
    let brew = Homebrew::new("held-back");
    brew.brew_installing(&target);
    let stub = Stub::release(&target);

    let output = brew.run(&stub, &["update"]);

    assert!(output.status.success(), "{}", said(&output));
    assert!(brew.calls().is_empty(), "ran {:?}", brew.calls());
    let told = stdout(&output);
    assert!(told.contains("new major version"), "{told}");
    assert!(told.contains("arin upgrade"), "{told}");
    assert!(told.contains("nothing was changed"), "{told}");
}

#[test]
fn upgrade_crosses_a_new_major_version() {
    let target = next_major();
    let brew = Homebrew::new("crosses");
    brew.brew_installing(&target);
    let stub = Stub::release(&target);

    let output = brew.run(&stub, &["upgrade"]);

    assert!(output.status.success(), "{}", said(&output));
    assert_eq!(brew.calls(), ["upgrade anistark/tools/arin"]);
    assert!(stdout(&output).contains(&format!("Arin is now {target}.")));
}

#[test]
fn upgrade_takes_the_newest_release_when_no_new_major_version_exists() {
    let target = next_minor();
    let brew = Homebrew::new("no-major");
    brew.brew_installing(&target);
    let stub = Stub::release(&target);

    let output = brew.run(&stub, &["upgrade"]);

    assert!(output.status.success(), "{}", said(&output));
    assert_eq!(brew.calls(), ["upgrade anistark/tools/arin"]);
}

#[test]
fn being_on_the_latest_release_runs_nothing() {
    let brew = Homebrew::new("latest");
    brew.brew("");
    let stub = Stub::release(CURRENT);

    for command in ["update", "upgrade"] {
        let output = brew.run(&stub, &[command]);
        assert!(output.status.success(), "{}", said(&output));
        assert_eq!(
            stdout(&output).trim(),
            format!("Arin {CURRENT} is the latest.")
        );
    }
    assert!(brew.calls().is_empty(), "ran {:?}", brew.calls());
}

/// The tap follows a release by a pull request. brew exits happily with nothing new, and that
/// is neither a failure nor a success to claim.
#[test]
fn a_tap_that_lags_the_release_is_said_plainly_and_is_not_an_error() {
    let target = next_minor();
    let brew = Homebrew::new("lags");
    brew.brew("");
    let stub = Stub::release(&target);

    let output = brew.run(&stub, &["update"]);

    assert!(output.status.success(), "{}", said(&output));
    assert_eq!(brew.calls(), ["upgrade anistark/tools/arin"]);
    let told = stdout(&output);
    assert!(
        told.contains(&format!("still {CURRENT}, not {target}")),
        "{told}"
    );
    assert!(!told.contains("is now"), "claimed success: {told}");
}

#[test]
fn a_failed_install_is_a_failure_and_claims_nothing() {
    let target = next_minor();
    let brew = Homebrew::new("fails");
    brew.brew("exit 3");
    let stub = Stub::release(&target);

    let output = brew.run(&stub, &["update"]);

    assert!(!output.status.success(), "{}", said(&output));
    assert!(stderr(&output).contains("failed"), "{}", said(&output));
    assert!(!said(&output).contains("is now"), "{}", said(&output));
}

#[test]
fn homebrew_missing_from_path_says_what_to_run() {
    let target = next_minor();
    let brew = Homebrew::new("no-brew");
    let stub = Stub::release(&target);

    let output = brew.run(&stub, &["update"]);

    assert!(!output.status.success(), "{}", said(&output));
    let told = stderr(&output);
    assert!(told.contains("not on your PATH"), "{}", said(&output));
    assert!(told.contains("brew upgrade anistark/tools/arin"), "{told}");
}

/// Terminal in, terminal out: whatever Homebrew says or asks has to reach the person.
#[test]
fn the_install_command_inherits_the_terminal() {
    let target = next_minor();
    let brew = Homebrew::new("terminal");
    brew.brew("read answer\necho \"brew heard $answer\"");
    let stub = Stub::release(&target);

    let child = brew
        .command()
        .arg("update")
        .env("ARIN_RELEASES_API", &stub.url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run arin");
    child
        .stdin
        .as_ref()
        .expect("piped stdin")
        .write_all(b"yes\n")
        .expect("answer");
    let output = child.wait_with_output().expect("wait for arin");

    assert!(
        stdout(&output).contains("brew heard yes"),
        "{}",
        said(&output)
    );
}

/// The failure this detection exists for: a clone in a folder that sounds like the tap, with a
/// working `brew` right there on PATH.
#[test]
fn a_checkout_named_like_the_tap_is_not_homebrew() {
    let scratch = Scratch::new("checkout");
    let binary = scratch.path("homebrew-tools/target/release/arin");
    install_binary(&binary);
    let log = scratch.logging_tool("brew");
    let stub = Stub::release(&next_minor());

    let output = run_copy(&scratch, &binary, &stub, &["update"]);

    assert_eq!(output.status.code(), Some(1), "{}", said(&output));
    assert!(!log.exists(), "brew was run for a checkout");
    assert!(stdout(&output).contains("cannot tell"), "{}", said(&output));
}

/// A folder called `Cellar` with the right shape under it, and no `opt` alias vouching for it.
#[test]
fn a_cellar_homebrew_does_not_vouch_for_is_not_homebrew() {
    let scratch = Scratch::new("false-cellar");
    let binary = scratch.path(&format!(
        "work/Cellar/arin/{CURRENT}/Arin.app/Contents/MacOS/arin"
    ));
    install_binary(&binary);
    let log = scratch.logging_tool("brew");
    let stub = Stub::release(&next_minor());

    let output = run_copy(&scratch, &binary, &stub, &["update"]);

    assert_eq!(output.status.code(), Some(1), "{}", said(&output));
    assert!(!log.exists(), "brew was run for a keg nothing vouches for");
}

#[test]
fn an_app_from_the_dmg_changes_nothing_and_exits_non_zero() {
    let scratch = Scratch::new("dmg");
    let binary = scratch.path("Applications/Arin.app/Contents/MacOS/arin");
    install_binary(&binary);
    let log = scratch.logging_tool("brew");
    let target = next_minor();
    let stub = Stub::release(&target);

    let output = run_copy(&scratch, &binary, &stub, &["upgrade"]);

    assert_eq!(output.status.code(), Some(1), "{}", said(&output));
    assert!(!log.exists(), "brew was run for the dmg");
    let told = stdout(&output);
    assert!(told.contains("nothing was changed"), "{told}");
    assert!(told.contains("releases/latest"), "{told}");

    // `--check` keeps the report-only use: the same facts, and asking is not a failure.
    let output = run_copy(&scratch, &binary, &stub, &["upgrade", "--check"]);
    assert!(output.status.success(), "{}", said(&output));
    assert!(stdout(&output).contains(&target), "{}", said(&output));
}

/// `cargo` is on PATH here to prove it is left alone.
#[test]
fn a_cargo_install_is_told_the_pinned_command_and_nothing_is_run() {
    let scratch = Scratch::new("cargo");
    let binary = scratch.path(".cargo/bin/arin");
    install_binary(&binary);
    let log = scratch.logging_tool("cargo");
    let target = next_minor();
    let stub = Stub::release(&target);

    let output = run_copy(&scratch, &binary, &stub, &["update"]);

    assert_eq!(output.status.code(), Some(1), "{}", said(&output));
    assert!(!log.exists(), "cargo was run");
    let told = stdout(&output);
    assert!(
        told.contains(&format!("--tag v{target} arin-cli")),
        "{told}"
    );
}

/// A spent quota really does happen: the limit is sixty an hour per address, shared by
/// everything behind that address.
#[test]
fn a_spent_rate_limit_is_explained_and_runs_nothing() {
    for status in ["403 Forbidden", "429 Too Many Requests"] {
        let brew = Homebrew::new("rate-limit");
        brew.brew("");
        let stub = Stub::answering(format!(
            "HTTP/1.1 {status}\r\nx-ratelimit-remaining: 0\r\nContent-Length: 0\r\n\
             Connection: close\r\n\r\n"
        ));

        let output = brew.run(&stub, &["update"]);

        assert!(!output.status.success(), "{}", said(&output));
        assert!(stderr(&output).contains("rate limit"), "{}", said(&output));
        assert!(!stderr(&output).contains(&status[..3]), "{}", said(&output));
        assert!(brew.calls().is_empty(), "ran {:?}", brew.calls());
        assert_eq!(stub.requests(), 1, "a refusal is not retried");
    }
}

/// The override is an environment variable and nothing else, and it cannot leave this machine.
#[test]
fn the_release_source_cannot_be_pointed_off_this_machine() {
    let brew = Homebrew::new("off-machine");
    brew.brew("");

    let output = brew
        .command()
        .arg("update")
        .env("ARIN_RELEASES_API", "https://example.com/releases/latest")
        .output()
        .expect("run arin");

    assert!(!output.status.success(), "{}", said(&output));
    assert!(
        stderr(&output).contains("not on this machine"),
        "{}",
        said(&output)
    );
    assert!(brew.calls().is_empty(), "ran {:?}", brew.calls());
}
