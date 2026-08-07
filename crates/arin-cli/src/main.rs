//! The `arin` binary.
//!
//! Runs the daemon, and doubles as a scripting client for driving it from a shell. The
//! client subcommands speak the same protocol an agent would, which makes them the
//! quickest way to check that a change actually works.

mod bundle;
mod cli;
mod client;
mod daemon;
mod diagnose;
#[cfg(target_os = "macos")]
mod hotkey;
mod update;

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
    if let Some(path) = cli.socket.clone() {
        config.socket_path = path;
    }

    // Nothing asked for. Say what the tool is rather than failing, which is the whole
    // reason the subcommand is optional. Opening the app bundle lands here too, and means
    // the opposite: run the daemon.
    let Some(command) = cli.action(cli::Launch::detect())? else {
        use clap::CommandFactory;
        Cli::command().print_help()?;
        return Ok(());
    };

    match command {
        Command::Daemon {
            headless,
            resolver,
            color,
            palette,
            grounding_consent,
            no_adaptive_color,
            check_updates,
        } => {
            config.resolver = resolver;
            config.adaptive_color = !no_adaptive_color;
            config.palette = configured_palette(color.as_deref(), palette.as_deref())?;
            if let Some(consent) = grounding_consent {
                config.grounding =
                    arin_core::Consent::parse(&consent).map_err(|e| anyhow::anyhow!(e))?;
            }
            daemon::start_daemon(config, headless, check_updates)
        }
        Command::Resolvers => diagnose::list_resolvers(),
        Command::Update => update::check(),
        // Reads the same settings a daemon started here would, so a palette or a resolver
        // named in the environment shows up in the report rather than being invisible to
        // it. Nothing is sent anywhere.
        Command::Diagnose { output } => {
            config.resolver = std::env::var("ARIN_RESOLVER").ok();
            config.palette = configured_palette(
                std::env::var("ARIN_COLOR").ok().as_deref(),
                std::env::var("ARIN_PALETTE").ok().as_deref(),
            )
            .unwrap_or_default();
            bundle::diagnose(&config, output.as_deref())
        }
        // Its own session, its own transport, and stdout it must not share. Logging is
        // already on stderr above, which is what makes that safe.
        Command::Mcp => block_on(arin_mcp::serve(&config.socket_path)),
        #[cfg(target_os = "macos")]
        Command::Displays => diagnose::list_displays(),
        #[cfg(target_os = "macos")]
        Command::Permissions { open } => diagnose::check_permissions(&config, open),
        #[cfg(target_os = "macos")]
        Command::Capture {
            display,
            probe,
            save,
        } => diagnose::capture_once(display, probe, save.as_deref()),
        other => block_on(client::run_client(config, other)),
    }
}

/// Work out the palette from what was asked for on the command line.
///
/// A full palette wins over a single colour, because someone who wrote both has said the
/// more specific thing. A single colour keeps the built-in fallbacks and only moves to the
/// front of them, which is what naming one colour nearly always means: draw my marks in
/// this, not give up every alternative when this cannot be seen.
///
/// A refusal is fatal rather than a warning. Starting anyway would mean drawing in a colour
/// nobody chose, and the person who typed it is watching this terminal right now.
fn configured_palette(color: Option<&str>, palette: Option<&str>) -> Result<arin_core::Palette> {
    use arin_core::{Palette, Rgb};

    if let Some(spec) = palette {
        return Ok(Palette::parse(spec)?);
    }
    let Some(color) = color else {
        return Ok(Palette::default());
    };
    let parsed = Rgb::parse(color)
        .with_context(|| format!("{color:?} is not a colour. Colours are written #RRGGBB"))?;
    Ok(Palette::preferring(parsed)?)
}

fn block_on<F: std::future::Future<Output = Result<()>>>(future: F) -> Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("could not start the async runtime")?
        .block_on(future)
}

// daemon
