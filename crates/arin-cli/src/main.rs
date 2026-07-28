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

    /// List the displays the overlay covers, with the id to pass to `--display`.
    #[cfg(target_os = "macos")]
    Displays,

    /// Take one frame and report what came back. Needs Screen Recording.
    #[cfg(target_os = "macos")]
    Capture {
        /// The display to capture.
        #[arg(long, default_value_t = 1)]
        display: u32,
        /// Report the pixel at a logical point, as `x,y`.
        #[arg(long)]
        probe: Option<String>,
    },
}

#[derive(Debug, Args)]
struct Target {
    /// The display to draw on. `arin displays` lists the ids.
    #[arg(long, default_value_t = 1)]
    display: u32,

    /// Keep the mark on screen until interrupted.
    ///
    /// Annotations live as long as the session that made them, and a one-shot command
    /// ends its session the moment it exits. Holding the connection open is what makes a
    /// mark stay up long enough to look at.
    #[arg(long)]
    hold: bool,
}

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
        Command::Daemon { headless } => start_daemon(config, headless),
        #[cfg(target_os = "macos")]
        Command::Displays => list_displays(),
        #[cfg(target_os = "macos")]
        Command::Capture { display, probe } => capture_once(display, probe),
        other => block_on(run_client(config, other)),
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

/// Start the daemon, taking over the main thread first where the platform demands it.
fn start_daemon(config: Config, headless: bool) -> Result<()> {
    #[cfg(target_os = "macos")]
    if !headless {
        // Diverges: the AppKit event loop runs until the process exits, so the daemon
        // gets a thread of its own.
        arin_mac::launch(move |renderer, _capture| {
            // Scroll detection samples a coarse grid, so it asks for a small frame
            // rather than twenty megabytes of Retina pixels twice a second.
            let capture = arin_mac::MacCapture::downscaled(512);
            std::thread::Builder::new()
                .name("arin-daemon".into())
                .spawn(move || {
                    let outcome = block_on(serve(config, Arc::new(renderer), Arc::new(capture)));
                    if let Err(e) = outcome {
                        tracing::error!(error = %e, "daemon stopped");
                        std::process::exit(1);
                    }
                    std::process::exit(0);
                })
                .expect("spawn the daemon thread");
        });
    }

    let (renderer, capture) = backends(headless)?;
    block_on(serve(config, renderer, capture))
}

async fn serve(
    config: Config,
    renderer: Arc<dyn Renderer>,
    capture: Arc<dyn Capture>,
) -> Result<()> {
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

/// The no-op backends, plus a refusal for any platform without a renderer.
///
/// The macOS path never reaches here: it is handled in `start_daemon`, before the
/// runtime exists, because AppKit has to claim the main thread first.
fn backends(headless: bool) -> Result<(Arc<dyn Renderer>, Arc<dyn Capture>)> {
    if headless {
        tracing::warn!("running headless: the protocol works, nothing will be drawn");
        return Ok((Arc::new(NoopRenderer::new()), Arc::new(NoopCapture)));
    }

    bail!(
        "no renderer for this platform yet: Linux lands in 0.4 and Windows in 0.6. \
         Run `arin daemon --headless` to exercise the protocol in the meantime."
    )
}

/// Print the displays the overlay would cover.
///
/// Needs the main thread for AppKit, but not the event loop, so it enumerates and exits.
/// This is how to find the id to pass to `--display`, since ids are the ones macOS
/// assigns rather than a count from one.
#[cfg(target_os = "macos")]
fn list_displays() -> Result<()> {
    let mtm = objc2::MainThreadMarker::new().context("must run on the main thread")?;
    for screen in arin_mac::known_screens(mtm) {
        let info = screen.info;
        println!(
            "{}\t{:.0}x{:.0} at {}x",
            info.id, info.logical_size[0], info.logical_size[1], info.scale
        );
    }
    Ok(())
}

/// Capture one frame and describe it.
///
/// Runs off the main thread on purpose. Capture blocks until ScreenCaptureKit answers,
/// and its handlers want a thread that is not sitting in a join.
#[cfg(target_os = "macos")]
fn capture_once(display: u32, probe: Option<String>) -> Result<()> {
    use arin_core::Capture as _;

    // Two frames a moment apart, so the report says not just what one looks like but how
    // much a still screen drifts between captures. That number is what scroll detection
    // has to see past.
    let (frame, second) = std::thread::spawn(move || {
        let backend = arin_mac::MacCapture::default();
        let first = backend.capture(DisplayId(display))?;
        std::thread::sleep(std::time::Duration::from_millis(400));
        let second = backend.capture(DisplayId(display))?;
        Ok::<_, arin_core::Error>((first, second))
    })
    .join()
    .map_err(|_| anyhow::anyhow!("the capture thread panicked"))??;

    let expected = frame.width as usize * frame.height as usize * 4;
    println!("display     {}", frame.display);
    println!("physical    {}x{}", frame.width, frame.height);
    println!(
        "logical     {:.0}x{:.0} at {}x",
        frame.logical_size[0], frame.logical_size[1], frame.scale
    );
    println!("bytes       {} (expected {})", frame.pixels.len(), expected);
    let drift = frame.signature().drift(&second.signature());
    println!(
        "drift       {:.3}% of cells over 400ms on a still screen ({})",
        drift * 100.0,
        if second.signature().moved_from(&frame.signature()) {
            "reads as movement"
        } else {
            "reads as still"
        }
    );

    let non_zero = frame.pixels.iter().filter(|b| **b != 0).count();
    println!(
        "non zero    {non_zero} of {} bytes ({:.1}%)",
        frame.pixels.len(),
        100.0 * non_zero as f64 / frame.pixels.len().max(1) as f64
    );

    if let Some(probe) = probe {
        let (x, y) = probe
            .split_once(',')
            .context("probe wants `x,y` in logical points")?;
        let x: f64 = x.trim().parse().context("probe x")?;
        let y: f64 = y.trim().parse().context("probe y")?;
        let px = (x * frame.scale) as usize;
        let py = (y * frame.scale) as usize;
        let idx = (py * frame.width as usize + px) * 4;
        match frame.pixels.get(idx..idx + 4) {
            Some(p) => println!(
                "probe       logical {x},{y} -> physical {px},{py} = [{}, {}, {}, {}]",
                p[0], p[1], p[2], p[3]
            ),
            None => println!("probe       logical {x},{y} is outside the frame"),
        }
    }

    Ok(())
}

/// Poll for content movement and drop annotations that no longer point at anything.
async fn watch_for_scrolling(daemon: Arc<Daemon>) {
    let mut watcher = ScrollWatcher::new(Arc::clone(&daemon));
    let mut ticker = tokio::time::interval(daemon.config().scroll_tick);
    // A slow tick should not queue up a burst of catch-up captures.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;

        // Capture blocks, and on the first call it blocks for as long as the permission
        // dialog is up, so it cannot run on a runtime worker. The watcher moves onto a
        // blocking thread and back again each tick.
        let handle = tokio::task::spawn_blocking(move || {
            let invalidated = watcher.tick();
            (watcher, invalidated)
        });

        match handle.await {
            Ok((returned, invalidated)) => {
                watcher = returned;
                for one in invalidated {
                    tracing::debug!(?one, "annotation invalidated");
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "scroll watcher stopped");
                return;
            }
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

    let mut hold = false;

    let message = match command {
        Command::Point {
            x,
            y,
            label,
            target,
        } => {
            hold = target.hold;
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
            hold = target.hold;
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

        #[cfg(target_os = "macos")]
        Command::Displays | Command::Capture { .. } => unreachable!("handled above"),
        Command::Daemon { .. } | Command::Status => unreachable!("handled above"),
    };

    let reply = client.send(message).await?;

    if hold {
        if let DaemonMessage::Ack(ack) = &reply
            && let Some(id) = &ack.annotation_id
        {
            println!("{id}");
        }
        eprintln!("holding the mark on screen, interrupt to clear");
        tokio::signal::ctrl_c().await.ok();
        return Ok(());
    }

    match reply {
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
