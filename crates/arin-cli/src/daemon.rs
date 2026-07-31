//! Running the daemon: wiring the backends, serving, and stopping cleanly.

use crate::block_on;
use anyhow::{Context, Result, bail};
use arin_core::{
    Capture, Config, Daemon, NoopCapture, NoopRenderer, Renderer, ScrollWatcher, Server,
};
use std::sync::Arc;

/// Start the daemon, taking over the main thread first where the platform demands it.
pub(crate) fn start_daemon(config: Config, headless: bool) -> Result<()> {
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
                    // There is a menu bar here, so there is somebody to ask about
                    // grounding. This is the only configuration in which that is true.
                    let outcome = block_on(serve(
                        config,
                        Arc::new(renderer),
                        Arc::new(capture),
                        Some(arin_mac::approver()),
                    ));
                    if let Err(e) = outcome {
                        tracing::error!(error = %e, "daemon stopped");
                        std::process::exit(1);
                    }
                    std::process::exit(0);
                })
                .expect("spawn the daemon thread");
        });
    }

    // No renderer means no interface, so there is nothing to prompt through. Core refuses
    // grounding under `ask` rather than assuming yes, which is what makes handing it
    // `None` here safe to do.
    let (renderer, capture) = backends(headless)?;
    block_on(serve(config, renderer, capture, None))
}

/// Build the configured resolver, and say plainly what enabling it means.
///
/// The security model is unresolved and consent is one of the open questions in it. Until
/// that is settled, the honest minimum is that the daemon cannot be made to send anything
/// off the machine without someone having named the thing that does it, and that it says
/// so where the person who started it will see it.
fn wire_resolver(config: &Config) -> Result<Option<Arc<dyn arin_core::Resolver>>> {
    let Some(name) = config.resolver.as_deref() else {
        return Ok(None);
    };

    let resolver = arin_resolve::Registry::with_builtins()
        .build(name)
        .with_context(|| format!("could not start the {name:?} resolver"))?;

    if resolver.is_remote() {
        tracing::warn!(
            resolver = name,
            "grounding is on, and this resolver sends screenshots of your screen to a \
             third party. Every `point` or `highlight` carrying a query captures the \
             display and uploads it. Stop the daemon or drop --resolver to turn this off."
        );
    } else {
        tracing::info!(
            resolver = name,
            "grounding is on, and stays on this machine"
        );
    }
    Ok(Some(resolver))
}

/// Say what will happen the first time a client asks Arin to look at the screen.
///
/// Only when there is a resolver, since with none configured the question never arises and
/// a line about consent would be noise. `always` is a real loosening rather than a
/// convenience, so it is a warning, and `ask` with nobody to ask is a configuration that
/// refuses everything, which is worth saying before somebody spends an afternoon on it.
fn report_consent(config: &Config, has_resolver: bool, can_ask: bool) {
    use arin_core::Consent;

    if !has_resolver {
        return;
    }
    match config.grounding {
        Consent::Always => tracing::warn!(
            "grounding is allowed without asking. Any program running as you can make Arin \
             read your screen and tell it what is there. Drop --grounding-consent to be \
             asked instead."
        ),
        Consent::Never => tracing::info!(
            "grounding is refused. A resolver is configured but no client can use it."
        ),
        Consent::Ask if can_ask => tracing::info!(
            "grounding will ask before it reads your screen, and remember your answer for \
             as long as you say"
        ),
        Consent::Ask => tracing::warn!(
            "grounding will be refused: this daemon has no way to ask you, so the answer \
             is no. Start it with --grounding-consent always if you meant to allow \
             grounding unattended."
        ),
    }
}

/// Say what colour marks will come out, when it is not the usual one.
///
/// Only when it has been configured. A line on every start saying marks are amber is noise,
/// and the reason to print this at all is that a palette is set once and then forgotten:
/// months later, "why are my marks green" wants an answer the daemon has and the person
/// asking does not.
fn report_palette(palette: &arin_core::Palette, adaptive: bool) {
    if palette.is_builtin() && adaptive {
        return;
    }
    let colors: Vec<String> = palette
        .candidates()
        .iter()
        .map(|color| color.to_hex())
        .collect();
    tracing::info!(
        preferred = %palette.preferred(),
        palette = colors.join(","),
        adaptive,
        "drawing marks from a configured palette"
    );
}

async fn serve(
    config: Config,
    renderer: Arc<dyn Renderer>,
    capture: Arc<dyn Capture>,
    approver: Option<Arc<dyn arin_core::Approver>>,
) -> Result<()> {
    let resolver = wire_resolver(&config)?;
    report_consent(&config, resolver.is_some(), approver.is_some());

    let daemon = Daemon::new(config, renderer, capture);
    let daemon = match resolver {
        Some(resolver) => daemon.with_resolver(resolver),
        None => daemon,
    };
    let daemon = match approver {
        Some(approver) => daemon.with_approver(approver),
        None => daemon,
    };
    let daemon = Arc::new(daemon);

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

        // The consent prompt says a grant can be taken back from the menu bar. This is
        // what makes that true, and a promise like that has to be kept or the prompt
        // becomes a reason not to grant anything.
        let reading = Arc::clone(&daemon);
        let revoking = Arc::clone(&daemon);
        arin_mac::on_grounding(
            move || reading.grounding_grant(),
            move || {
                revoking.revoke_grounding();
            },
        );

        // Panels have already been rebuilt by this point, which takes their layers with
        // them, so whatever survives has to be drawn again.
        let arranging = Arc::clone(&daemon);
        arin_mac::on_displays_changed(move || {
            let dropped = arranging.reconcile_displays();
            let redrawn = arranging.redraw_all();
            if !dropped.is_empty() || redrawn > 0 {
                tracing::info!(
                    dropped = dropped.len(),
                    redrawn,
                    "displays changed, marks reconciled"
                );
            }
        });
    }

    // The menu bar asks the daemon to stop the same way a signal does. `NSApplication
    // terminate:` would end the process without unwinding, leaving the socket file.
    let quit = Arc::new(tokio::sync::Notify::new());
    #[cfg(target_os = "macos")]
    {
        let quit = Arc::clone(&quit);
        arin_mac::on_quit(move || quit.notify_one());
    }

    let server = Server::bind(Arc::clone(&daemon)).context("could not bind the socket")?;

    tracing::info!(socket = %server.socket_path().display(), "arin daemon ready");
    report_palette(&daemon.config().palette, daemon.config().adaptive_color);

    let watcher = tokio::spawn(watch_for_scrolling(Arc::clone(&daemon)));
    let expiry = tokio::spawn(expire_annotations(Arc::clone(&daemon)));

    // Held for as long as the daemon runs. Dropping the manager unregisters the chord,
    // so the binding is tied to the daemon's lifetime rather than leaking past it.
    #[cfg(target_os = "macos")]
    let _hotkey = match crate::hotkey::listen(Arc::clone(&daemon)) {
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

/// What the menu bar says the daemon is doing.
///
/// Counts rather than a health check. Whether the daemon is running is answered by the
/// icon being in the menu bar at all, so the useful thing to report is what it is holding
/// on the user's behalf, which is also what tells them whether Clear will do anything.
#[cfg(target_os = "macos")]
pub(crate) fn status_line(marks: usize, sessions: usize) -> String {
    match (marks, sessions) {
        (0, 0) => "idle, no clients".to_owned(),
        (0, s) => format!("{s} client{}, nothing drawn", plural(s)),
        (m, s) => format!("{m} mark{} from {s} client{}", plural(m), plural(s)),
    }
}

/// An `s` when there is not exactly one of something.
#[cfg(target_os = "macos")]
fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

// The status line is the menu bar's, so it only exists where there is a menu bar.
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn the_status_line_reads_as_a_sentence() {
        assert_eq!(status_line(0, 0), "idle, no clients");
        assert_eq!(status_line(0, 1), "1 client, nothing drawn");
        assert_eq!(status_line(0, 3), "3 clients, nothing drawn");
        assert_eq!(status_line(1, 1), "1 mark from 1 client");
        assert_eq!(status_line(4, 2), "4 marks from 2 clients");
    }
}
