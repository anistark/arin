//! The scripting client: one shot subcommands that speak the protocol a client would.
//!
//! These exist to be the quickest way to check that a change actually works, which means
//! they go through the socket like any other client rather than reaching into the daemon.

use crate::cli::{Command, parse_point};
use anyhow::{Context, Result, bail};
use arin_core::{Client, Config};
use arin_protocol::{
    Anchor, Clear, ClientMessage, DaemonMessage, DisplayId, Draw, Highlight, LogicalRect, Point,
    StrokeStyle, Textbox,
};

// client
pub(crate) async fn run_client(config: Config, command: Command) -> Result<()> {
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

    client.start_session("arin").await?;

    let mut hold = false;

    let message = match command {
        Command::Point {
            position,
            y,
            at,
            label,
            target,
        } => {
            hold = target.hold;
            let display = DisplayId(target.display);
            let mut point = match (position, y, at) {
                (Some(x), Some(y), None) => match x.parse::<f64>() {
                    Ok(x) => Point::at(x, y, display),
                    Err(_) => bail!(
                        "{x:?} is not a coordinate. A description goes on its own: \
                         `arin point \"the Submit button\"`"
                    ),
                },
                // One argument. A number is half a pair, anything else is a description.
                (Some(query), None, None) => {
                    if query.parse::<f64>().is_ok() {
                        bail!("point wants both `x` and `y`, or a description in quotes");
                    }
                    Point::query(query, display)
                }
                (None, None, Some(at)) => Point::named(at, display),
                // Clap rejects a target alongside `--at`, so what is left is a stray `y`.
                _ => bail!("point wants `x y`, a description in quotes, or `--at <POSITION>`"),
            };
            point.label = label;
            point.ttl_ms = target.ttl_ms()?;
            ClientMessage::Point(point)
        }

        Command::Highlight {
            region,
            y,
            width,
            height,
            label,
            target,
        } => {
            hold = target.hold;
            let display = DisplayId(target.display);
            let mut highlight = match (region, y, width, height) {
                (Some(x), Some(y), Some(width), Some(height)) => match x.parse::<f64>() {
                    Ok(x) => Highlight::over(LogicalRect::new(x, y, width, height), display),
                    Err(_) => bail!(
                        "{x:?} is not a coordinate. A description goes on its own: \
                         `arin highlight \"the error message\"`"
                    ),
                },
                (Some(query), None, None, None) => {
                    if query.parse::<f64>().is_ok() {
                        bail!("highlight wants `x y width height`, or a description in quotes");
                    }
                    Highlight::query(query, display)
                }
                _ => bail!("highlight wants `x y width height`, or a description in quotes"),
            };
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
        Command::Daemon { .. }
        | Command::Resolvers
        | Command::Update
        | Command::Status
        | Command::Mcp
        | Command::Diagnose { .. } => {
            unreachable!("handled above")
        }
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
