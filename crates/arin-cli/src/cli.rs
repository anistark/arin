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

    /// Run the daemon in the foreground. The same as `arin daemon`.
    ///
    /// Starting the daemon is the first thing anyone does after installing, so it has a
    /// short spelling. It runs in the foreground and stops on Ctrl-C: starting at login is
    /// the launchd agent's job, deliberately, so that `-d` never leaves something behind
    /// after the terminal closes.
    ///
    /// It takes no options of its own. Anything beyond a default daemon wants the
    /// subcommand, as in `arin daemon --resolver local`. Keeping the flag bare is what
    /// stops it drifting into a second copy of the daemon's command line.
    #[arg(short = 'd', long = "daemon")]
    pub(crate) daemon: bool,

    /// Optional, so bare `arin` prints help instead of failing.
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

/// The bundle identifier in `packaging/macos/Info.plist`.
///
/// Duplicated here because the Rust side has to recognise its own bundle at runtime and
/// cannot read a plist it may not be inside of. `the_bundle_identifier_matches_the_plist`
/// keeps the two honest.
///
/// Gated to exactly where it is read. `Launch::detect` only consults it on macOS, and the
/// drift test is worth running everywhere because comparing two strings needs no window
/// server. Without the `test` arm this is dead code on Linux, which CI denies.
#[cfg(any(target_os = "macos", test))]
pub(crate) const BUNDLE_ID: &str = "com.anistark.arin";

/// How this process was started, which is what a bare `arin` means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Launch {
    /// Typed at a shell. A bare invocation is somebody looking around, so print help.
    Shell,
    /// Opened by LaunchServices, from Finder, Spotlight, or `open -a Arin`. Nobody is
    /// reading a terminal, so a bare invocation is the app being opened and it runs the
    /// daemon.
    Bundle,
}

impl Launch {
    /// Tell a double click apart from a shell prompt.
    ///
    /// Both arrive as the same binary with an empty argument list, so something outside the
    /// arguments has to decide. LaunchServices puts the identifier of whatever it opened
    /// into the environment and a shell does not.
    ///
    /// That variable is undocumented, so it is matched against our own identifier rather
    /// than merely tested for presence, and a terminal on stdin overrides it. Both guards
    /// fail in the same direction: printing help into a window nobody is looking at is a
    /// wasted double click, while starting a daemon somebody did not ask for leaves a
    /// process running with a Screen Recording grant behind it.
    #[cfg(target_os = "macos")]
    pub(crate) fn detect() -> Self {
        use std::io::IsTerminal;

        let opened = std::env::var("__CFBundleIdentifier").is_ok_and(|id| id == BUNDLE_ID);
        if opened && !std::io::stdin().is_terminal() {
            Self::Bundle
        } else {
            Self::Shell
        }
    }

    /// No bundles anywhere else, so nothing can be opened rather than typed.
    #[cfg(not(target_os = "macos"))]
    pub(crate) fn detect() -> Self {
        Self::Shell
    }
}

impl Cli {
    /// Reconcile `-d`, the subcommand, and how the process was started into the one thing
    /// to do.
    ///
    /// `Ok(None)` means nothing was asked for and the caller should print help. That is a
    /// success rather than an error: someone typing `arin` to see what it is has not made
    /// a mistake.
    pub(crate) fn action(self, launch: Launch) -> Result<Option<Command>> {
        match (self.daemon, self.command) {
            (true, Some(_)) => bail!(
                "-d already runs the daemon, so it cannot be combined with a subcommand. \
                 Use `arin -d` on its own, or name the subcommand you meant."
            ),
            (true, None) => Self::daemon_defaults().map(Some),
            // Opening the app is asking for the daemon. There is no other reason to open
            // something with no Dock icon and no window.
            (false, None) if launch == Launch::Bundle => Self::daemon_defaults().map(Some),
            (false, command) => Ok(command),
        }
    }

    /// `-d` is `arin daemon` with nothing else said, and it has to stay exactly that.
    ///
    /// Built by parsing the subcommand rather than by constructing the variant, so that the
    /// environment variables clap reads for `--resolver`, `--color` and the rest reach both
    /// spellings. A hand-built default would ignore `ARIN_RESOLVER` while `arin daemon`
    /// honoured it, which is the kind of difference nobody finds until it matters.
    fn daemon_defaults() -> Result<Command> {
        Self::try_parse_from(["arin", "daemon"])
            .context("building the default daemon command for -d")?
            .command
            .context("`arin daemon` parsed without a subcommand")
    }
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
        ///
        /// `local` grounds against a model served on this machine and sends nothing
        /// anywhere. `claude` grounds against a hosted model and uploads a screenshot of
        /// the display on every query that carries one.
        #[arg(long, value_name = "NAME", env = "ARIN_RESOLVER")]
        resolver: Option<String>,

        /// Draw marks in this colour, as `#RRGGBB`.
        ///
        /// The daemon still moves off it when it would be invisible against what is under
        /// the mark, falling back through the rest of the palette. `--palette` replaces
        /// that fallback set, and `--no-adaptive-color` turns the whole thing off and
        /// always uses this.
        ///
        /// Blue is refused. It belongs to the orb, and a mark in the orb's own colour
        /// reads as part of the orb rather than as a separate mark.
        #[arg(long, value_name = "HEX", env = "ARIN_COLOR")]
        color: Option<String>,

        /// The full set of colours to choose between, preferred first.
        ///
        /// A comma separated list of `#RRGGBB`, for example
        /// `#FFB020,#FF3B30,#30D158`. The first is what marks are drawn in and the rest
        /// are what the daemon may fall back to. Replaces the built-in palette rather than
        /// adding to it, and takes precedence over `--color`.
        #[arg(long, value_name = "HEX,HEX,...", env = "ARIN_PALETTE")]
        palette: Option<String>,

        /// Whether a client may make Arin look at the screen to ground a query.
        ///
        /// `ask` prompts you the first time and remembers your answer for as long as you
        /// said. `always` never prompts, which is what an unattended daemon wants. `never`
        /// refuses every query while leaving the resolver configured.
        ///
        /// Drawing is never gated. A process running as you could open its own window and
        /// draw on it anyway, so gating that would cost every client a setup step and buy
        /// nothing. Grounding is different: it is backed by the Screen Recording permission,
        /// which Arin holds and your clients do not.
        ///
        /// With `ask` and no way to prompt, such as `--headless`, the answer is no.
        #[arg(long, value_name = "ask|always|never", env = "ARIN_GROUNDING_CONSENT")]
        grounding_consent: Option<String>,

        /// Check GitHub once a day for a newer Arin, and say so in the menu bar.
        ///
        /// Off unless asked for, because it is the only thing besides a remote resolver
        /// that makes the daemon talk to the network at all. One request a day, carrying
        /// nothing but a version string.
        ///
        /// It only ever reports. Nothing is downloaded and nothing is installed.
        /// `arin update` asks the same question once, without turning anything on.
        #[arg(long, env = "ARIN_CHECK_UPDATES")]
        check_updates: bool,

        /// Never look at the screen to choose a colour.
        ///
        /// Marks are always drawn in the preferred colour. Saves a capture per positioned
        /// annotation, and costs the record of what each mark was drawn over, so marks
        /// following a scroll are then checked against the display-wide movement alone.
        #[arg(long)]
        no_adaptive_color: bool,
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

    /// Check whether a newer Arin has been released.
    ///
    /// Reports and stops. Nothing is downloaded and nothing is installed: builds are
    /// unsigned, so fetching one would put you in front of Gatekeeper, and replacing a
    /// running app without a signature to check is not something to do quietly. It prints
    /// the upgrade command instead.
    ///
    /// This makes one request to GitHub. Running it is how you ask for that, which is why
    /// there is no flag to turn it on: nothing here checks on its own.
    Update,

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

    /// Write a report about this machine, for attaching to a bug report.
    ///
    /// Arin collects no telemetry, so there is nothing on our side to look at when
    /// something goes wrong. This is the replacement, and you run it rather than it running
    /// itself. Nothing is uploaded: it prints to your terminal so you can read it before
    /// deciding to send any of it.
    ///
    /// Secrets are never quoted. An API key is reported as set or not set, with its length.
    Diagnose {
        /// Write to a file instead of printing.
        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,
    },

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
        /// Also write the frame into this directory, for a grounding corpus.
        ///
        /// Three files: the raw pixels a resolver would be handed, a manifest describing
        /// them, and a PNG to open. Label targets by reading coordinates off the PNG into
        /// `cases.json`, then score a resolver with
        /// `cargo run --release -p arin-resolve --example eval -- <dir> <resolver>`.
        ///
        /// These are raw captures of your screen. Put them somewhere you are happy to have
        /// them, and not in a repository.
        #[arg(long, value_name = "DIR")]
        save: Option<PathBuf>,
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
        } = cli.command.expect("a subcommand was given")
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
        } = cli.command.expect("a subcommand was given")
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
        } = cli.command.expect("a subcommand was given")
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
        } = cli.command.expect("a subcommand was given")
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
        let Command::Point { position, .. } = cli.command.expect("a subcommand was given") else {
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

    /// The runtime cannot read the plist of a bundle it may not be inside, so the
    /// identifier is written twice. This is the only thing stopping the copies drifting,
    /// and a drift would be silent: `Launch::detect` would stop recognising its own bundle
    /// and opening the app from Spotlight would quietly print help to nowhere.
    #[test]
    fn the_bundle_identifier_matches_the_plist() {
        let plist = include_str!("../../../packaging/macos/Info.plist");
        let marker = "<key>CFBundleIdentifier</key>";
        let rest = plist
            .split_once(marker)
            .expect("Info.plist declares a bundle identifier")
            .1;
        let declared = rest
            .split_once("<string>")
            .and_then(|(_, tail)| tail.split_once("</string>"))
            .expect("the identifier is a string value")
            .0
            .trim();

        assert_eq!(
            declared, BUNDLE_ID,
            "packaging/macos/Info.plist and cli::BUNDLE_ID disagree, so the app would not \
             recognise itself when opened from Finder"
        );
    }

    /// Opening something with no Dock icon and no window is asking for the daemon. There
    /// is nothing else it could mean, and no terminal to print help into.
    #[test]
    fn opening_the_app_runs_the_daemon_where_a_shell_would_get_help() {
        let opened = Cli::parse_from(["arin"])
            .action(Launch::Bundle)
            .expect("opening the app is a valid way to start")
            .expect("opening the app runs something");
        let Command::Daemon { .. } = opened else {
            panic!("opening the app has to reach the daemon");
        };

        assert!(
            Cli::parse_from(["arin"])
                .action(Launch::Shell)
                .expect("bare `arin` is not an error")
                .is_none(),
            "the same empty command line typed at a shell still prints help"
        );
    }

    /// Someone typing `arin` to find out what it is has not made a mistake, so this parses
    /// and the caller prints help. The check is that no subcommand is demanded.
    #[test]
    fn bare_arin_is_not_an_error() {
        let cli = Cli::try_parse_from(["arin"]).expect("bare `arin` must parse");
        assert!(cli.command.is_none());
        assert!(!cli.daemon);
        assert!(
            cli.action(Launch::Shell)
                .expect("bare `arin` is not an error")
                .is_none(),
            "nothing was asked for, so there is nothing to run"
        );
    }

    /// The two spellings must not drift. Built by parsing rather than by constructing the
    /// variant, so this also covers the environment variables reaching both.
    #[test]
    fn dash_d_is_exactly_the_daemon_subcommand() {
        let short = Cli::parse_from(["arin", "-d"])
            .action(Launch::Shell)
            .expect("-d is a valid line")
            .expect("-d runs something");
        let long = Cli::parse_from(["arin", "daemon"])
            .action(Launch::Shell)
            .expect("the subcommand is a valid line")
            .expect("the subcommand runs something");

        let (Command::Daemon { .. }, Command::Daemon { .. }) = (&short, &long) else {
            panic!("both spellings must reach the daemon");
        };
        assert_eq!(
            format!("{short:?}"),
            format!("{long:?}"),
            "`arin -d` and `arin daemon` must agree on every option, or one of them is a \
             second daemon command line that nobody is maintaining"
        );
    }

    /// `-d` runs the daemon, and the one shot commands connect, send, and exit. Combining
    /// them asks for two different programs at once, so it is refused rather than
    /// resolved by precedence.
    #[test]
    fn dash_d_refuses_to_be_a_modifier_on_a_subcommand() {
        let cli = Cli::parse_from(["arin", "-d", "clear"]);
        let error = cli
            .action(Launch::Shell)
            .expect_err("-d with a subcommand is refused");
        assert!(
            error.to_string().contains("cannot be combined"),
            "the error has to say what is wrong with the line, got: {error}"
        );
    }

    /// The daemon has to be told to ground, and the flag reads as a switch rather than as
    /// a model name so that turning it on is a decision about egress rather than a tweak.
    #[test]
    fn the_daemon_takes_a_named_resolver_and_defaults_to_none() {
        let cli = Cli::parse_from(["arin", "daemon"]);
        let Command::Daemon { resolver, .. } = cli.command.expect("a subcommand was given") else {
            panic!("expected daemon");
        };
        assert_eq!(
            resolver, None,
            "a daemon started with no argument must not ground anything"
        );

        let cli = Cli::parse_from(["arin", "daemon", "--resolver", "claude"]);
        let Command::Daemon { resolver, .. } = cli.command.expect("a subcommand was given") else {
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
        let Command::Clear { annotation_id } = cli.command.expect("a subcommand was given") else {
            panic!("expected clear");
        };
        assert_eq!(annotation_id, None);
    }
}
