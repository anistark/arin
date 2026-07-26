//! The `arin` binary.
//!
//! Runs the daemon, and doubles as a scripting client for driving it from a shell. The
//! client subcommands speak the same protocol an agent would, which makes them the
//! quickest way to check that a change actually works.

use anyhow::{Context, Result, bail};
use arin_core::{
    Capture, Client, Config, Daemon, NoopCapture, NoopRenderer, Renderer, ScrollWatcher, Server,
};
use arin_protocol::{
    Clear, ClientMessage, DaemonMessage, DisplayId, Highlight, LogicalRect, Point,
};
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;

/// An annotation layer any agent can draw on.
#[derive(Debug, Parser)]
#[command(name = "arin", version, about, long_about = None)]
struct Cli {
    /// Override the daemon socket path.
    #[arg(long, global = true, env = "ARIN_SOCKET")]
    socket: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the daemon.
    Daemon {
        /// Run without a renderer.
        ///
        /// The socket, the protocol, and the whole state machine work, but nothing is drawn.
        /// This is how to exercise the daemon before the platform backend exists, and on
        /// platforms that do not have one yet.
        #[arg(long)]
        headless: bool,
    },

    /// Put the orb on a point.
    Point {
        /// Horizontal position in logical points.
        x: f64,
        /// Vertical position in logical points.
        y: f64,
        /// Short caption to render next to the orb.
        #[arg(long)]
        label: Option<String>,
        #[command(flatten)]
        target: Target,
    },

    /// Outline a region.
    Highlight {
        /// Left edge in logical points.
        x: f64,
        /// Top edge in logical points.
        y: f64,
        /// Width in logical points.
        width: f64,
        /// Height in logical points.
        height: f64,
        /// Short caption to render against the region.
        #[arg(long)]
        label: Option<String>,
        #[command(flatten)]
        target: Target,
    },

    /// Remove annotations.
    Clear {
        /// The annotation to clear. Omit to clear everything in the session.
        annotation_id: Option<String>,
    },

    /// Report whether the daemon is reachable.
    Status,
}

#[derive(Debug, Args)]
struct Target {
    /// The display to draw on.
    #[arg(long, default_value_t = 1)]
    display: u32,
}

#[tokio::main]
async fn main() -> Result<()> {
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
        Command::Daemon { headless } => run_daemon(config, headless).await,
        other => run_client(config, other).await,
    }
}

// daemon
async fn run_daemon(config: Config, headless: bool) -> Result<()> {
    let (renderer, capture) = backends(headless)?;
    let daemon = Arc::new(Daemon::new(config, renderer, capture));
    let server = Server::bind(Arc::clone(&daemon)).context("could not bind the socket")?;

    tracing::info!(socket = %server.socket_path().display(), "arin daemon ready");

    let watcher = tokio::spawn(watch_for_scrolling(Arc::clone(&daemon)));

    tokio::select! {
        result = server.run() => result.context("the socket server stopped")?,
        _ = tokio::signal::ctrl_c() => tracing::info!("interrupted, shutting down"),
    }

    watcher.abort();
    Ok(())
}

/// Choose the platform backends, or the no-op ones.
fn backends(headless: bool) -> Result<(Arc<dyn Renderer>, Arc<dyn Capture>)> {
    if headless {
        tracing::warn!("running headless: the protocol works, nothing will be drawn");
        return Ok((Arc::new(NoopRenderer::new()), Arc::new(NoopCapture)));
    }

    #[cfg(target_os = "macos")]
    {
        let renderer = arin_mac::MacRenderer::new()
            .context("the macOS renderer is still a scaffold; try `arin daemon --headless`")?;
        let capture = arin_mac::MacCapture::new().context(
            "the macOS capture backend is still a scaffold; try `arin daemon --headless`",
        )?;
        Ok((Arc::new(renderer), Arc::new(capture)))
    }

    #[cfg(not(target_os = "macos"))]
    {
        bail!(
            "no renderer for this platform yet: Linux lands in 0.4 and Windows in 0.6. \
             Run `arin daemon --headless` to exercise the protocol in the meantime."
        )
    }
}

/// Poll for content movement and drop annotations that no longer point at anything.
async fn watch_for_scrolling(daemon: Arc<Daemon>) {
    let mut watcher = ScrollWatcher::new(Arc::clone(&daemon));
    let mut ticker = tokio::time::interval(daemon.config().scroll_tick);
    // A slow tick should not queue up a burst of catch-up captures.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        // TODO(0.1): capture blocks, so this belongs on `spawn_blocking` once a real
        // capture backend exists. With the no-op backend it returns immediately.
        for invalidated in watcher.tick() {
            tracing::debug!(?invalidated, "annotation invalidated");
        }
    }
}

// client
async fn run_client(config: Config, command: Command) -> Result<()> {
    let mut client = Client::connect_to(&config.socket_path)
        .await
        .with_context(|| {
            format!(
                "could not reach the daemon on {}; is `arin daemon` running?",
                config.socket_path.display()
            )
        })?;

    if matches!(command, Command::Status) {
        println!("daemon reachable on {}", config.socket_path.display());
        return Ok(());
    }

    client.start_session("arin-cli").await?;

    let message = match command {
        Command::Point {
            x,
            y,
            label,
            target,
        } => {
            let mut point = Point::at(x, y, DisplayId(target.display));
            point.label = label;
            ClientMessage::Point(point)
        }

        Command::Highlight {
            x,
            y,
            width,
            height,
            label,
            target,
        } => {
            let mut highlight = Highlight::over(
                LogicalRect::new(x, y, width, height),
                DisplayId(target.display),
            );
            highlight.label = label;
            ClientMessage::Highlight(highlight)
        }

        Command::Clear { annotation_id } => ClientMessage::Clear(match annotation_id {
            Some(id) => Clear::one(arin_protocol::AnnotationId::new(id)),
            None => Clear::all(),
        }),

        Command::Daemon { .. } | Command::Status => unreachable!("handled above"),
    };

    match client.send(message).await? {
        DaemonMessage::Ack(ack) => {
            if let Some(id) = ack.annotation_id {
                println!("{id}");
            }
            Ok(())
        }
        DaemonMessage::Error(e) => bail!("{}: {}", e.code, e.msg),
        DaemonMessage::Invalidated(inv) => {
            bail!("annotation invalidated immediately: {:?}", inv.reason)
        }
    }

    // The connection drops here, which ends the session and clears its annotations five
    // seconds later. A one-shot CLI mark is meant to be transient.
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn point_parses_the_documented_invocation() {
        let cli = Cli::parse_from([
            "arin",
            "point",
            "412",
            "88",
            "--display",
            "1",
            "--label",
            "Save",
        ]);
        let Command::Point {
            x,
            y,
            label,
            target,
        } = cli.command
        else {
            panic!("expected point");
        };
        assert_eq!((x, y), (412.0, 88.0));
        assert_eq!(target.display, 1);
        assert_eq!(label.as_deref(), Some("Save"));
    }

    #[test]
    fn clear_defaults_to_everything() {
        let cli = Cli::parse_from(["arin", "clear"]);
        let Command::Clear { annotation_id } = cli.command else {
            panic!("expected clear");
        };
        assert_eq!(annotation_id, None);
    }
}
