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
    Anchor, Clear, ClientMessage, DaemonMessage, DisplayId, Draw, Highlight, LogicalRect, Point,
    StrokeStyle, Textbox,
};
#[cfg(target_os = "macos")]
mod hotkey;

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
        /// Horizontal position in logical points. Omit when using `--at`.
        x: Option<f64>,
        /// Vertical position in logical points. Omit when using `--at`.
        y: Option<f64>,
        /// A position named relative to the display instead of `x y`.
        ///
        /// One of `top-left`, `top`, `top-right`, `left`, `center`, `right`,
        /// `bottom-left`, `bottom`, `bottom-right`, or a pair like `50%,30%`.
        #[arg(long, value_name = "POSITION", conflicts_with_all = ["x", "y"])]
        at: Option<String>,
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

    /// Place a block of explanatory text. Display only, never an input.
    Annotate {
        /// Left edge in logical points.
        x: f64,
        /// Top edge in logical points.
        y: f64,
        /// Width in logical points.
        width: f64,
        /// Height in logical points.
        height: f64,
        /// The text to render.
        #[arg(long)]
        text: String,
        #[command(flatten)]
        target: Target,
    },

    /// Draw a freehand path through a list of `x,y` points.
    Draw {
        /// Points in logical coordinates, for example `100,200 140,210 180,190`.
        #[arg(required = true, num_args = 2..)]
        points: Vec<String>,
        /// Stroke width in logical points.
        #[arg(long)]
        width: Option<f64>,
        /// Stroke colour as `#RRGGBB`. Omit to let the daemon choose.
        #[arg(long)]
        color: Option<String>,
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

    /// Report whether screen capture works, and help fix it if it does not.
    ///
    /// Exits non-zero when capture is not working, so it can gate a script.
    #[cfg(target_os = "macos")]
    Permissions {
        /// Open the Screen Recording list in System Settings instead of reporting.
        #[arg(long)]
        open: bool,
    },

    /// Take one frame and report what came back. Needs Screen Recording.
    #[cfg(target_os = "macos")]
    Capture {
        /// The display to capture.
        #[arg(long, default_value_t = DisplayId::DEFAULT.0)]
        display: u32,
        /// Report the pixel at a logical point, as `x,y`.
        #[arg(long)]
        probe: Option<String>,
    },
}

#[derive(Debug, Args)]
struct Target {
    /// The display to draw on. `arin displays` lists the ids.
    #[arg(long, default_value_t = DisplayId::DEFAULT.0)]
    display: u32,

    /// Keep the mark on screen until interrupted.
    ///
    /// Annotations live as long as the session that made them, and a one-shot command
    /// ends its session the moment it exits. Holding the connection open is what makes a
    /// mark stay up long enough to look at.
    #[arg(long)]
    hold: bool,

    /// Remove the mark after this many seconds.
    ///
    /// Seconds here, milliseconds on the wire. Combine with `--hold` to watch a mark
    /// expire, since otherwise the session ends first and takes it away regardless.
    #[arg(long, value_name = "SECONDS")]
    ttl: Option<f64>,
}

impl Target {
    /// The time to live in the milliseconds the protocol carries.
    ///
    /// Rounds up, so a sub-millisecond TTL asks for the shortest life the wire can
    /// express rather than zero, which is refused.
    fn ttl_ms(&self) -> Result<Option<u64>> {
        let Some(seconds) = self.ttl else {
            return Ok(None);
        };
        if !seconds.is_finite() || seconds <= 0.0 {
            bail!("--ttl wants a positive number of seconds, got {seconds}");
        }
        Ok(Some(((seconds * 1000.0).ceil() as u64).max(1)))
    }
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
        Command::Permissions { open } => check_permissions(&config, open),
        #[cfg(target_os = "macos")]
        Command::Capture { display, probe } => capture_once(display, probe),
        other => block_on(run_client(config, other)),
    }
}

/// An `s` when there is not exactly one of something.
#[cfg(target_os = "macos")]
fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// What the menu bar says the daemon is doing.
///
/// Counts rather than a health check. Whether the daemon is running is answered by the
/// icon being in the menu bar at all, so the useful thing to report is what it is holding
/// on the user's behalf, which is also what tells them whether Clear will do anything.
#[cfg(target_os = "macos")]
fn status_line(marks: usize, sessions: usize) -> String {
    match (marks, sessions) {
        (0, 0) => "idle, no clients".to_owned(),
        (0, s) => format!("{s} client{}, nothing drawn", plural(s)),
        (m, s) => format!("{m} mark{} from {s} client{}", plural(m), plural(s)),
    }
}

/// Parse an `x,y` pair from the command line.
fn parse_point(raw: &str) -> Result<[f64; 2]> {
    let (x, y) = raw
        .split_once(',')
        .with_context(|| format!("{raw:?} is not an `x,y` pair"))?;
    Ok([
        x.trim()
            .parse()
            .with_context(|| format!("bad x in {raw:?}"))?,
        y.trim()
            .parse()
            .with_context(|| format!("bad y in {raw:?}"))?,
    ])
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

    // The menu bar is built before the daemon exists, so the actions arrive now rather
    // than at construction.
    #[cfg(target_os = "macos")]
    {
        let clearing = Arc::clone(&daemon);
        arin_mac::on_clear(move || {
            let cleared = clearing.clear_everything();
            if !cleared.is_empty() {
                tracing::info!(count = cleared.len(), "cleared from the menu bar");
            }
        });

        // Runs on the main thread while the menu is opening, so it only reads counters.
        let reporting = Arc::clone(&daemon);
        arin_mac::on_status(move || {
            status_line(reporting.annotation_count(), reporting.session_count())
        });
    }

    // Quitting from the menu bar used to go straight to `NSApplication terminate:`, which
    // ends the process without unwinding this function and so without unlinking the
    // socket. It now asks the daemon to stop the same way a signal does.
    let quit = Arc::new(tokio::sync::Notify::new());
    #[cfg(target_os = "macos")]
    {
        let quit = Arc::clone(&quit);
        arin_mac::on_quit(move || quit.notify_one());
    }

    let server = Server::bind(Arc::clone(&daemon)).context("could not bind the socket")?;

    tracing::info!(socket = %server.socket_path().display(), "arin daemon ready");

    let watcher = tokio::spawn(watch_for_scrolling(Arc::clone(&daemon)));
    let expiry = tokio::spawn(expire_annotations(Arc::clone(&daemon)));

    // Held for as long as the daemon runs. Dropping the manager unregisters the chord,
    // so the binding is tied to the daemon's lifetime rather than leaking past it.
    #[cfg(target_os = "macos")]
    let _hotkey = match hotkey::listen(Arc::clone(&daemon)) {
        Ok(manager) => Some(manager),
        Err(e) => {
            // Losing the hotkey costs the user their escape hatch, but the daemon is
            // still useful without it and refusing to start would be worse.
            tracing::warn!(error = %e, "clear hotkey unavailable");
            None
        }
    };

    tokio::select! {
        result = server.run() => result.context("the socket server stopped")?,
        reason = stop_requested(Arc::clone(&quit)) => tracing::info!(%reason, "shutting down"),
    }

    // Dropping the server here is what unlinks the socket, so every path out of the
    // select above has to return rather than exit the process.
    watcher.abort();
    expiry.abort();
    Ok(())
}

/// Resolve when something asks the daemon to stop, naming what did.
///
/// Ctrl-C alone is not enough. A `SIGTERM` from a service manager or a `SIGHUP` when the
/// terminal goes away would otherwise kill the process where it stands, leaving the
/// socket file behind for the next start to notice and clear. That is untidy rather than
/// broken, but it is untidy every single time.
async fn stop_requested(quit: Arc<tokio::sync::Notify>) -> &'static str {
    let menu = async move {
        quit.notified().await;
        "quit"
    };

    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        // A handler that will not register is not worth refusing to start over: Ctrl-C
        // still works, and the socket is still recovered on the next run.
        let mut terminate = signal(SignalKind::terminate())
            .inspect_err(|e| tracing::warn!(error = %e, "no SIGTERM handler"))
            .ok();
        let mut hangup = signal(SignalKind::hangup())
            .inspect_err(|e| tracing::warn!(error = %e, "no SIGHUP handler"))
            .ok();

        // A signal that could not be registered simply never arrives.
        let sigterm = async {
            match terminate.as_mut() {
                Some(signal) => signal.recv().await,
                None => std::future::pending().await,
            }
        };
        let sighup = async {
            match hangup.as_mut() {
                Some(signal) => signal.recv().await,
                None => std::future::pending().await,
            }
        };

        tokio::select! {
            _ = tokio::signal::ctrl_c() => "interrupt",
            _ = sigterm => "SIGTERM",
            _ = sighup => "SIGHUP",
            reason = menu => reason,
        }
    }

    #[cfg(not(unix))]
    tokio::select! {
        _ = tokio::signal::ctrl_c() => "interrupt",
        reason = menu => reason,
    }
}

/// Sweep away annotations whose time to live has run out.
///
/// Its own task rather than a step inside the scroll watcher, because that one skips a
/// tick when nothing is drawn and does a screen capture when something is. Expiry has to
/// keep its own schedule and costs nothing to run.
async fn expire_annotations(daemon: Arc<Daemon>) {
    let mut ticker = tokio::time::interval(daemon.config().expiry_tick);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        for expired in daemon.expire_annotations() {
            tracing::debug!(?expired, "annotation expired");
        }
    }
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

/// Report whether screen capture works, and offer the way to fix it.
///
/// Separate from `arin status`, which asks whether the daemon is reachable. This asks
/// something else: a daemon with no permission is running perfectly, it just cannot see
/// the screen.
///
/// Proving the permission means taking a frame, and only one process can do that at a
/// time. So when the daemon is up it is the authority and this reports what it can check
/// without capturing, rather than taking its own failure to capture as a denial. Those
/// two look identical from here and mean opposite things.
#[cfg(target_os = "macos")]
fn check_permissions(config: &Config, open: bool) -> Result<()> {
    if open {
        if !arin_mac::open_screen_recording_settings() {
            bail!("could not open System Settings");
        }
        println!(
            "opened System Settings, now {}",
            arin_mac::SCREEN_RECORDING_HELP
        );
        return Ok(());
    }

    if std::os::unix::net::UnixStream::connect(&config.socket_path).is_ok() {
        if !arin_mac::screen_recording_granted() {
            println!("{}", arin_mac::ScreenRecording::Missing.explain());
            println!("`arin permissions --open` goes straight to the switch");
            std::process::exit(1);
        }
        println!("screen recording is granted");
        println!(
            "the daemon is running, and it is the one process that can take a frame. \
             Its log says on startup whether capture actually works. To prove it from \
             here, stop the daemon and run this again."
        );
        return Ok(());
    }

    // Capture must never run on the main thread.
    let state = std::thread::spawn(arin_mac::screen_recording)
        .join()
        .map_err(|_| anyhow::anyhow!("the permission check panicked"))?;

    println!("{}", state.explain());

    if state != arin_mac::ScreenRecording::Working {
        println!("`arin permissions --open` goes straight to the switch");
        std::process::exit(1);
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
            at,
            label,
            target,
        } => {
            hold = target.hold;
            let display = DisplayId(target.display);
            let mut point = match (x, y, at) {
                (Some(x), Some(y), None) => Point::at(x, y, display),
                (None, None, Some(at)) => Point::named(at, display),
                // Clap rejects `x` alongside `--at`, so what is left is half a pair or
                // nothing at all.
                _ => bail!("point wants `x y`, or `--at <POSITION>`"),
            };
            point.label = label;
            point.ttl_ms = target.ttl_ms()?;
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
            highlight.ttl_ms = target.ttl_ms()?;
            ClientMessage::Highlight(highlight)
        }

        Command::Annotate {
            x,
            y,
            width,
            height,
            text,
            target,
        } => {
            hold = target.hold;
            ClientMessage::Textbox(
                Textbox::new(
                    Anchor::new(
                        LogicalRect::new(x, y, width, height),
                        DisplayId(target.display),
                    ),
                    text,
                )
                .with_ttl_ms(target.ttl_ms()?),
            )
        }

        Command::Draw {
            points,
            width,
            color,
            target,
        } => {
            hold = target.hold;
            let path = points
                .iter()
                .map(|p| parse_point(p))
                .collect::<Result<Vec<_>>>()?;
            let mut draw = Draw::new(DisplayId(target.display), path).with_ttl_ms(target.ttl_ms()?);
            if width.is_some() || color.is_some() {
                draw.style = Some(StrokeStyle { width, color });
            }
            ClientMessage::Draw(draw)
        }

        Command::Clear { annotation_id } => ClientMessage::Clear(match annotation_id {
            Some(id) => Clear::one(arin_protocol::AnnotationId::new(id)),
            None => Clear::all(),
        }),

        #[cfg(target_os = "macos")]
        Command::Displays | Command::Capture { .. } | Command::Permissions { .. } => {
            unreachable!("handled above")
        }
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
        // Reporting what the daemon pushes is most of what `--hold` is for: it is how you
        // watch a time to live run out, or see a scroll take a mark away, without reading
        // the daemon's own log.
        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => return Ok(()),
                event = client.next_invalidation() => match event {
                    Ok(event) => match event.annotation_id {
                        Some(id) => eprintln!("{id} went away: {:?}", event.reason),
                        None => eprintln!("marks went away: {:?}", event.reason),
                    },
                    // The daemon hung up. Nothing left to hold.
                    Err(e) => {
                        eprintln!("daemon closed the connection: {e}");
                        return Ok(());
                    }
                },
            }
        }
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
            at,
            label,
            target,
        } = cli.command
        else {
            panic!("expected point");
        };
        assert_eq!((x, y), (Some(412.0), Some(88.0)));
        assert_eq!(at, None);
        assert_eq!(target.display, 1);
        assert_eq!(label.as_deref(), Some("Save"));
    }

    #[test]
    fn a_named_position_replaces_the_coordinates() {
        let cli = Cli::parse_from(["arin", "point", "--at", "top-left"]);
        let Command::Point { x, y, at, .. } = cli.command else {
            panic!("expected point");
        };
        assert_eq!((x, y), (None, None));
        assert_eq!(at.as_deref(), Some("top-left"));
    }

    /// Coordinates and a name are two answers to the same question, so clap refuses them
    /// together rather than leaving the daemon to pick one.
    #[test]
    fn coordinates_and_a_name_cannot_both_be_given() {
        assert!(Cli::try_parse_from(["arin", "point", "412", "88", "--at", "top-left"]).is_err());
    }

    /// The menu line is the only place the daemon reports itself to the user, and it
    /// cannot be checked by opening the menu from a script: driving the menu bar needs
    /// Accessibility, which is the one permission Arin promises never to require.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_status_line_reads_as_a_sentence() {
        assert_eq!(super::status_line(0, 0), "idle, no clients");
        assert_eq!(super::status_line(0, 1), "1 client, nothing drawn");
        assert_eq!(super::status_line(0, 3), "3 clients, nothing drawn");
        assert_eq!(super::status_line(1, 1), "1 mark from 1 client");
        assert_eq!(super::status_line(4, 2), "4 marks from 2 clients");
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
