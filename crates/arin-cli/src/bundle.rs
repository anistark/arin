//! The bug report bundle.
//!
//! Arin collects no telemetry and never will, so when something goes wrong there is
//! nothing on our side to look at. This is the replacement: the user runs one command, gets
//! a plain text report, reads it, and decides whether to send it. Nothing leaves the
//! machine at any point, and that is the entire design constraint.
//!
//! Which is why it prints to stdout by default rather than writing a file. A report you
//! have to open to see is one most people will attach unread, and a report anyone can read
//! is one this module has to be careful with. Every value that could carry a secret is
//! reported as present or absent rather than quoted, whether or not Arin is the thing that
//! put it there.
//!
//! # What this cannot tell you
//!
//! The configuration of a *running* daemon. There is no wire message that asks, so what is
//! reported here is what a daemon started from this shell right now would use. When the two
//! differ, the daemon was started with different arguments or a different environment, and
//! that is worth knowing rather than papering over, so the report says which it is showing.

use anyhow::{Context, Result};
use arin_core::Config;
use std::fmt::Write as _;
use std::path::Path;

/// Environment variables Arin reads, which are also how it is configured.
///
/// Listed rather than discovered by prefix, so a report says `not set` for something
/// missing. Half the value of this section is in what is absent: a resolver that will not
/// start because `ARIN_LOCAL_ENDPOINT` points somewhere stale reads as a mystery until the
/// variable is on the page.
const READ_VARIABLES: &[&str] = &[
    "ARIN_SOCKET",
    "ARIN_RESOLVER",
    "ARIN_COLOR",
    "ARIN_PALETTE",
    "ARIN_LOCAL_ENDPOINT",
    "ARIN_LOCAL_MODEL",
    "ARIN_LOCAL_COORDS",
    "ARIN_LOCAL_STRUCTURED",
    "ANTHROPIC_API_KEY",
    "RUST_LOG",
    "XDG_RUNTIME_DIR",
];

/// Substrings that mean a value is never printed, only counted.
///
/// Matched on the variable's name rather than on its contents, because a heuristic over
/// values would have to decide what a secret looks like and it would eventually be wrong in
/// the direction that matters.
const SECRET_MARKERS: &[&str] = &["KEY", "TOKEN", "SECRET", "PASSWORD", "CREDENTIAL"];

/// Write a report about this machine, for attaching to a bug report.
///
/// Prints to stdout unless `output` names a file, because a report nobody reads is one
/// people attach without knowing what is in it.
pub(crate) fn diagnose(config: &Config, output: Option<&Path>) -> Result<()> {
    let report = collect(config);

    match output {
        Some(path) => {
            std::fs::write(path, &report)
                .with_context(|| format!("could not write {}", path.display()))?;
            println!("wrote {}", path.display());
            println!("Read it before you send it. Nothing was uploaded.");
        }
        None => print!("{report}"),
    }
    Ok(())
}

fn collect(config: &Config) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# arin diagnose");
    let _ = writeln!(
        out,
        "\nNothing here has been sent anywhere. Arin collects no telemetry, so this is the\n\
         whole of what a bug report can carry. Read it before attaching it."
    );

    build(&mut out);
    daemon(&mut out, config);
    settings(&mut out, config);
    resolvers(&mut out);
    platform(&mut out, config);
    environment(&mut out);
    out
}

fn section(out: &mut String, name: &str) {
    let _ = writeln!(out, "\n## {name}\n");
}

fn build(out: &mut String) {
    section(out, "build");
    let _ = writeln!(out, "version     {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(
        out,
        "target      {}-{}",
        std::env::consts::ARCH,
        std::env::consts::OS
    );
    let _ = writeln!(
        out,
        "profile     {}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    // The version the crates carry, which is the protocol's rather than the cycle's, and
    // the one that decides whether a client can talk to this daemon.
    let _ = writeln!(out, "protocol    {}", arin_protocol::PROTOCOL_VERSION);
}

fn daemon(out: &mut String, config: &Config) {
    section(out, "daemon");
    let path = &config.socket_path;
    let _ = writeln!(out, "socket      {}", path.display());

    let reachable = std::os::unix::net::UnixStream::connect(path).is_ok();
    let _ = writeln!(out, "reachable   {}", if reachable { "yes" } else { "no" });

    match std::fs::metadata(path) {
        Ok(meta) => {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = writeln!(
                out,
                "mode        {:04o} (0600 expected)",
                meta.permissions().mode() & 0o7777
            );
        }
        Err(e) if !reachable => {
            let _ = writeln!(out, "mode        no socket file: {e}");
        }
        Err(e) => {
            let _ = writeln!(out, "mode        unreadable: {e}");
        }
    }

    // A socket path is a hard limit rather than a preference, the error the OS gives is
    // useless, and a long temp directory blows through it without warning.
    let _ = writeln!(
        out,
        "path bytes  {} (the OS limit is about 104)",
        path.as_os_str().len()
    );
}

/// What a daemon started right now would use.
///
/// Not what the running one is using. See the module docs: nothing on the wire asks, and
/// pretending otherwise would make this section actively misleading in exactly the case
/// somebody is filing a bug about.
fn settings(out: &mut String, config: &Config) {
    section(out, "settings a daemon started here would use");
    let _ = writeln!(
        out,
        "resolver    {}",
        config.resolver.as_deref().unwrap_or("none, grounding off")
    );
    let _ = writeln!(out, "adaptive    {}", config.adaptive_color);
    let _ = writeln!(
        out,
        "palette     {}{}",
        config
            .palette
            .candidates()
            .iter()
            .map(|color| color.to_hex())
            .collect::<Vec<_>>()
            .join(","),
        if config.palette.is_builtin() {
            " (built in)"
        } else {
            ""
        }
    );
    let _ = writeln!(
        out,
        "default ttl {}",
        match config.default_ttl {
            Some(ttl) => format!("{}ms", ttl.as_millis()),
            None => "none, marks live until cleared".to_owned(),
        }
    );
    let _ = writeln!(out, "linger      {}ms", config.linger.as_millis());
    let _ = writeln!(out, "scroll tick {}ms", config.scroll_tick.as_millis());
    let _ = writeln!(out, "same uid    {}", config.require_same_uid);
}

fn resolvers(out: &mut String) {
    section(out, "resolvers");
    let registry = arin_resolve::Registry::with_builtins();
    for name in registry.names() {
        // Built rather than described, so a resolver that will not start says why here.
        match registry.build(name) {
            Ok(resolver) if resolver.is_remote() => {
                let _ = writeln!(out, "{name:<12}ready, SENDS SCREENSHOTS OFF THIS MACHINE");
            }
            Ok(_) => {
                let _ = writeln!(out, "{name:<12}ready, stays on this machine");
            }
            Err(e) => {
                let _ = writeln!(out, "{name:<12}unavailable: {e}");
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn platform(out: &mut String, config: &Config) {
    section(out, "macos");

    if let Some(version) = product_version() {
        let _ = writeln!(out, "os          {version}");
    }

    let _ = writeln!(
        out,
        "capture     {}",
        // Only one process can hold the capture stream, so a daemon that is up is the
        // authority. Taking our own failure as a denial would report the opposite of the
        // truth in the one case where the daemon is working perfectly.
        if std::os::unix::net::UnixStream::connect(&config.socket_path).is_ok() {
            if arin_mac::screen_recording_granted() {
                "granted. The daemon holds the stream, so this was not proven by capturing"
                    .to_owned()
            } else {
                "NOT GRANTED".to_owned()
            }
        } else {
            match std::thread::spawn(arin_mac::screen_recording).join() {
                Ok(state) => state.explain().to_owned(),
                Err(_) => "the permission check panicked".to_owned(),
            }
        }
    );

    match objc2::MainThreadMarker::new() {
        Some(mtm) => {
            let screens = arin_mac::known_screens(mtm);
            let _ = writeln!(out, "displays    {}", screens.len());
            for screen in screens {
                let info = screen.info;
                let _ = writeln!(
                    out,
                    "  {}\t{:.0}x{:.0} at {}x",
                    info.id, info.logical_size[0], info.logical_size[1], info.scale
                );
            }
        }
        None => {
            let _ = writeln!(
                out,
                "displays    not enumerated, this is not the main thread"
            );
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn platform(out: &mut String, _config: &Config) {
    section(out, "platform");
    let _ = writeln!(
        out,
        "There is no renderer for {} yet, so the daemon runs headless here.",
        std::env::consts::OS
    );
}

/// The product version, which is what a bug report needs and `std` does not carry.
#[cfg(target_os = "macos")]
fn product_version() -> Option<String> {
    let output = std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn environment(out: &mut String) {
    section(out, "environment");
    for name in READ_VARIABLES {
        let _ = writeln!(out, "{name:<24}{}", describe_variable(name));
    }
}

/// What to print for one variable.
///
/// A secret is reported as set or not set and never quoted. The length goes with it, since
/// "set, 12 characters" is often enough to spot a truncated key without the key being on
/// the page.
fn describe_variable(name: &str) -> String {
    let Ok(value) = std::env::var(name) else {
        return "not set".to_owned();
    };
    if SECRET_MARKERS
        .iter()
        .any(|marker| name.to_ascii_uppercase().contains(marker))
    {
        return format!("set, {} characters, not shown", value.chars().count());
    }
    if value.is_empty() {
        return "set but empty".to_owned();
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use arin_core::Palette;

    /// The one thing this module must never do. A bundle exists to be pasted into a public
    /// issue, and an API key in it is a key that has to be rotated.
    #[test]
    fn a_secret_is_never_quoted_however_it_is_spelled() {
        for name in [
            "ANTHROPIC_API_KEY",
            "SOME_TOKEN",
            "app_secret",
            "DB_PASSWORD",
            "AWS_CREDENTIALS",
        ] {
            // Safety: single threaded test, and the variable is removed before it returns.
            unsafe { std::env::set_var(name, "sk-ant-not-a-real-key-0123456789") };
            let described = describe_variable(name);
            unsafe { std::env::remove_var(name) };

            assert!(
                !described.contains("sk-ant"),
                "{name} leaked its value: {described}"
            );
            assert!(described.contains("not shown"), "{name} gave {described}");
            assert!(
                described.contains("32 characters"),
                "the length is what spots a truncated key, {name} gave {described}"
            );
        }
    }

    #[test]
    fn an_ordinary_variable_is_quoted_and_a_missing_one_says_so() {
        unsafe { std::env::set_var("ARIN_TEST_ORDINARY", "http://127.0.0.1:1234") };
        assert_eq!(
            describe_variable("ARIN_TEST_ORDINARY"),
            "http://127.0.0.1:1234"
        );
        unsafe { std::env::remove_var("ARIN_TEST_ORDINARY") };

        assert_eq!(describe_variable("ARIN_TEST_ABSENT"), "not set");
    }

    /// An empty variable and an absent one behave differently everywhere in this codebase,
    /// so a report that renders them the same hides the difference that matters.
    #[test]
    fn an_empty_variable_is_distinguishable_from_a_missing_one() {
        unsafe { std::env::set_var("ARIN_TEST_EMPTY", "") };
        assert_eq!(describe_variable("ARIN_TEST_EMPTY"), "set but empty");
        unsafe { std::env::remove_var("ARIN_TEST_EMPTY") };
    }

    #[test]
    fn the_key_arin_actually_reads_is_on_the_list_and_is_treated_as_a_secret() {
        assert!(READ_VARIABLES.contains(&"ANTHROPIC_API_KEY"));
        assert!(
            SECRET_MARKERS
                .iter()
                .any(|marker| "ANTHROPIC_API_KEY".contains(marker))
        );
    }

    #[test]
    fn a_report_carries_the_sections_a_bug_report_needs() {
        let report = collect(&Config::default());
        for heading in ["## build", "## daemon", "## resolvers", "## environment"] {
            assert!(report.contains(heading), "no {heading} in\n{report}");
        }
        assert!(report.contains(env!("CARGO_PKG_VERSION")));
        assert!(
            report.contains("no telemetry"),
            "the report has to say what it is, since its whole job is being read"
        );
    }

    /// The section that is easiest to get quietly wrong: reporting the built-in palette
    /// while the daemon draws something else, or the reverse.
    #[test]
    fn a_configured_palette_reaches_the_report() {
        let config = Config {
            palette: Palette::parse("#FF2D95,#30D158").expect("both are colours"),
            ..Config::default()
        };

        let report = collect(&config);
        assert!(report.contains("#FF2D95,#30D158"), "got\n{report}");
        assert!(
            !report.contains("(built in)"),
            "a configured palette must not be reported as the built-in one"
        );

        assert!(collect(&Config::default()).contains("(built in)"));
    }
}
