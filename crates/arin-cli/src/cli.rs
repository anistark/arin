//! The command surface: what `arin` accepts, and how it is spelled.
//!
//! Kept apart from the code that acts on it so that the shape of the tool can be read in
//! one sitting. Most of the length here is documentation rather than logic, because these
//! doc comments are the `--help` output and the only explanation most people will read.

use anyhow::{Context, Result, bail};
use arin_protocol::DisplayId;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

/// An annotation layer any agent can draw on.
#[derive(Debug, Parser)]
#[command(name = "arin", version, about, long_about = None)]
pub(crate) struct Cli {
    /// Override the daemon socket path.
    #[arg(long, global = true, env = "ARIN_SOCKET")]
    pub(crate) socket: Option<PathBuf>,

    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Run the daemon.
    Daemon {
        /// Run without a renderer.
        ///
        /// The socket, the protocol, and the whole state machine work, but nothing is drawn.
        /// This is how to exercise the daemon before the platform backend exists, and on
        /// platforms that do not have one yet.
        #[arg(long)]
        headless: bool,

        /// Ground natural language targets with this resolver. `arin resolvers` lists them.
        ///
        /// Off unless named. A resolver may send screenshots of your screen to a third
        /// party, so it is never turned on by inference: having an API key in your
        /// environment is not the same as asking for one to be used.
        #[arg(long, value_name = "NAME", env = "ARIN_RESOLVER")]
        resolver: Option<String>,
    },

    /// Serve MCP on stdio, for an agent to launch as a subprocess.
    ///
    /// Not something to run by hand. An MCP client starts it, speaks MCP on stdin and
    /// stdout, and closes stdin when it is done. Point a client at it with
    /// `claude mcp add arin -- arin mcp`.
    ///
    /// This was a separate `arin-mcp` binary. One binary means one path in an agent's
    /// config and one version to keep straight, which matters because that config is
    /// written once and then outlives several updates.
    Mcp,

    /// List the resolvers this build can ground queries with.
    Resolvers,

    /// Put the orb on a point.
    ///
    /// Three ways to say where. `arin point 412 88` gives coordinates, `arin point "the
    /// Submit button"` describes the target and needs a resolver, and `--at top-left`
    /// names a position on the display.
    Point {
        /// Horizontal position in logical points, or a description of the target.
        ///
        /// A number here wants a `y` after it. Anything else is read as a description and
        /// grounded by the daemon's resolver, which has to have been configured.
        #[arg(value_name = "X | DESCRIPTION")]
        position: Option<String>,
        /// Vertical position in logical points. Omit when describing a target or using `--at`.
        y: Option<f64>,
        /// A position named relative to the display instead of `x y`.
        ///
        /// One of `top-left`, `top`, `top-right`, `left`, `center`, `right`,
        /// `bottom-left`, `bottom`, `bottom-right`, or a pair like `50%,30%`.
        #[arg(long, value_name = "POSITION", conflicts_with_all = ["position", "y"])]
        at: Option<String>,
        /// Short caption to render next to the orb.
        #[arg(long)]
        label: Option<String>,
        #[command(flatten)]
        target: Target,
    },

    /// Outline a region.
    ///
    /// `arin highlight 100 200 340 90` gives the rectangle. `arin highlight "the error
    /// message"` describes it and needs a resolver.
    Highlight {
        /// Left edge in logical points, or a description of the region.
        #[arg(value_name = "X | DESCRIPTION")]
        region: Option<String>,
        /// Top edge in logical points.
        y: Option<f64>,
        /// Width in logical points.
        width: Option<f64>,
        /// Height in logical points.
        height: Option<f64>,
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
pub(crate) struct Target {
    /// The display to draw on. `arin displays` lists the ids.
    #[arg(long, default_value_t = DisplayId::DEFAULT.0)]
    pub(crate) display: u32,

    /// Keep the mark on screen until interrupted.
    ///
    /// Annotations live as long as the session that made them, and a one-shot command
    /// ends its session the moment it exits. Holding the connection open is what makes a
    /// mark stay up long enough to look at.
    #[arg(long)]
    pub(crate) hold: bool,

    /// Remove the mark after this many seconds.
    ///
    /// Seconds here, milliseconds on the wire. Combine with `--hold` to watch a mark
    /// expire, since otherwise the session ends first and takes it away regardless.
    #[arg(long, value_name = "SECONDS")]
    pub(crate) ttl: Option<f64>,
}

impl Target {
    /// The time to live in the milliseconds the protocol carries.
    ///
    /// Rounds up, so a sub-millisecond TTL asks for the shortest life the wire can
    /// express rather than zero, which is refused.
    pub(crate) fn ttl_ms(&self) -> Result<Option<u64>> {
        let Some(seconds) = self.ttl else {
            return Ok(None);
        };
        if !seconds.is_finite() || seconds <= 0.0 {
            bail!("--ttl wants a positive number of seconds, got {seconds}");
        }
        Ok(Some(((seconds * 1000.0).ceil() as u64).max(1)))
    }
}

/// Parse an `x,y` pair from the command line.
pub(crate) fn parse_point(raw: &str) -> Result<[f64; 2]> {
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
            position,
            y,
            at,
            label,
            target,
        } = cli.command
        else {
            panic!("expected point");
        };
        assert_eq!((position.as_deref(), y), (Some("412"), Some(88.0)));
        assert_eq!(at, None);
        assert_eq!(target.display, 1);
        assert_eq!(label.as_deref(), Some("Save"));
    }

    #[test]
    fn a_named_position_replaces_the_coordinates() {
        let cli = Cli::parse_from(["arin", "point", "--at", "top-left"]);
        let Command::Point {
            position, y, at, ..
        } = cli.command
        else {
            panic!("expected point");
        };
        assert_eq!((position, y), (None, None));
        assert_eq!(at.as_deref(), Some("top-left"));
    }

    /// The acceptance criterion for 0.3, as it is actually typed.
    #[test]
    fn a_description_is_accepted_where_coordinates_go() {
        let cli = Cli::parse_from(["arin", "point", "the Submit button"]);
        let Command::Point {
            position, y, at, ..
        } = cli.command
        else {
            panic!("expected point");
        };
        assert_eq!(position.as_deref(), Some("the Submit button"));
        assert_eq!(y, None);
        assert_eq!(at, None);
    }

    #[test]
    fn a_description_is_accepted_for_a_region_too() {
        let cli = Cli::parse_from(["arin", "highlight", "the error message"]);
        let Command::Highlight {
            region,
            y,
            width,
            height,
            ..
        } = cli.command
        else {
            panic!("expected highlight");
        };
        assert_eq!(region.as_deref(), Some("the error message"));
        assert_eq!((y, width, height), (None, None, None));
    }

    /// One number is the ambiguous case: it could be half a coordinate pair or a very
    /// strange description. It is read as the former and refused, because a client that
    /// meant the latter can always add a word and one that dropped a `y` gets told.
    #[test]
    fn half_a_coordinate_pair_is_refused_rather_than_grounded() {
        let cli = Cli::parse_from(["arin", "point", "412"]);
        let Command::Point { position, .. } = cli.command else {
            panic!("expected point");
        };
        // Clap accepts it. The refusal is the client's, where the message can explain.
        assert_eq!(position.as_deref(), Some("412"));
    }

    #[test]
    fn a_description_and_a_named_position_cannot_both_be_given() {
        assert!(
            Cli::try_parse_from(["arin", "point", "the Submit button", "--at", "top-left"])
                .is_err()
        );
    }

    /// The daemon has to be told to ground, and the flag reads as a switch rather than as
    /// a model name so that turning it on is a decision about egress rather than a tweak.
    #[test]
    fn the_daemon_takes_a_named_resolver_and_defaults_to_none() {
        let cli = Cli::parse_from(["arin", "daemon"]);
        let Command::Daemon { resolver, .. } = cli.command else {
            panic!("expected daemon");
        };
        assert_eq!(
            resolver, None,
            "a daemon started with no argument must not ground anything"
        );

        let cli = Cli::parse_from(["arin", "daemon", "--resolver", "claude"]);
        let Command::Daemon { resolver, .. } = cli.command else {
            panic!("expected daemon");
        };
        assert_eq!(resolver.as_deref(), Some("claude"));
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
    fn clear_defaults_to_everything() {
        let cli = Cli::parse_from(["arin", "clear"]);
        let Command::Clear { annotation_id } = cli.command else {
            panic!("expected clear");
        };
        assert_eq!(annotation_id, None);
    }
}
