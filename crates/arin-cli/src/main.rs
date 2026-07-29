//! The `arin` binary.
//!
//! Runs the daemon, and doubles as a scripting client for driving it from a shell. The
//! client subcommands speak the same protocol an agent would, which makes them the
//! quickest way to check that a change actually works.

mod cli;
mod client;
mod daemon;
mod diagnose;
#[cfg(target_os = "macos")]
mod hotkey;

use anyhow::{Context, Result};
use arin_core::Config;
use clap::Parser;
use cli::{Cli, Command};

/// Not `#[tokio::main]` on purpose.
///
/// AppKit owns the main thread, so on macOS the daemon runs on a worker and the main
/// thread belongs to the overlay's event loop. Every other path builds its own runtime
/// here instead.
fn main() -> Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "arin=info,arin_core=info".into()),
        )
        .init();

    let mut config = Config::default();
    if let Some(path) = cli.socket {
        config.socket_path = path;
    }

    match cli.command {
        Command::Daemon { headless, resolver } => {
            config.resolver = resolver;
            daemon::start_daemon(config, headless)
        }
        Command::Resolvers => diagnose::list_resolvers(),
        // Its own session, its own transport, and stdout it must not share. Logging is
        // already on stderr above, which is what makes that safe.
        Command::Mcp => block_on(arin_mcp::serve(&config.socket_path)),
        #[cfg(target_os = "macos")]
        Command::Displays => diagnose::list_displays(),
        #[cfg(target_os = "macos")]
        Command::Permissions { open } => diagnose::check_permissions(&config, open),
        #[cfg(target_os = "macos")]
        Command::Capture { display, probe } => diagnose::capture_once(display, probe),
        other => block_on(client::run_client(config, other)),
    }
}

fn block_on<F: std::future::Future<Output = Result<()>>>(future: F) -> Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("could not start the async runtime")?
        .block_on(future)
}

// daemon
