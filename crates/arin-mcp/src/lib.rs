//! MCP server for Arin.
//!
//! Translates MCP tool calls into protocol messages on the daemon socket. It holds no
//! state of its own beyond a session: it is a socket client like any other, and adds no
//! capability the protocol does not already expose.
//!
//! # One session for the process
//!
//! Annotations live as long as the session that made them, so the server opens one on
//! startup and keeps it for as long as the client holds the server open. That is what
//! lets an agent leave a mark up across several turns of a conversation. The CLI's
//! `--hold` exists because a one-shot command has the opposite problem.
//!
//! # Why the tools are not named after the messages
//!
//! `point_at` rather than `point`, `annotate` rather than `textbox`. A tool name is read
//! by a model deciding what to reach for, so it is named after the intent. Mapping those
//! onto wire messages is this crate's job, and nothing above it needs to know.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::sync::Arc;

use arin_core::Client;
use arin_protocol::{
    Anchor, Clear, ClientMessage, DaemonMessage, DisplayId, Highlight, LogicalRect, Point, Textbox,
};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{Implementation, ServerCapabilities, ServerInfo};
use rmcp::{ErrorData, ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

/// The MCP tool names this server exposes.
///
/// Named after what an agent is trying to do, not after the protocol message underneath,
/// so intents phrase naturally in a model's own words. `point_at` reads better in a tool
/// call than `point`, and `annotate` is what an agent reaches for when it wants to leave
/// an explanation rather than a mark.
pub mod tools {
    /// Put the orb on a target. Maps to `point`.
    pub const POINT_AT: &str = "point_at";
    /// Outline a region. Maps to `highlight`.
    pub const HIGHLIGHT: &str = "highlight";
    /// Place explanatory text. Maps to `textbox`.
    pub const ANNOTATE: &str = "annotate";
    /// Remove annotations. Maps to `clear`.
    pub const CLEAR: &str = "clear";

    /// Every tool name, for registration and for tests.
    pub const ALL: &[&str] = &[POINT_AT, HIGHLIGHT, ANNOTATE, CLEAR];
}

/// The name this server reports to the daemon on `session_start`.
pub const CLIENT_NAME: &str = "arin-mcp";

/// What a drawing tool hands back.
///
/// The annotation id, so a later `clear` can name this mark specifically, and the display
/// it landed on. The display carries the scale, which is what a client working from a
/// screenshot needs in order to send logical points next time rather than pixels.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct Drawn {
    /// Identifies this annotation, for a later `clear`.
    pub annotation_id: String,
    /// Logical width of the display it was drawn on.
    pub display_width: Option<f64>,
    /// Logical height of the display it was drawn on.
    pub display_height: Option<f64>,
    /// Backing scale. Divide screenshot pixels by this to get logical points.
    pub display_scale: Option<f64>,
    /// Marks of yours that went away since your last call, and why.
    ///
    /// Empty almost always. A non-empty list means the screen moved on without you: the
    /// user scrolled, a time to live ran out, or they cleared the overlay themselves.
    /// Anything you were relying on being visible is not.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gone: Vec<Gone>,
}

/// A mark that is no longer on screen.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct Gone {
    /// The annotation that went away, if it was a single one.
    pub annotation_id: Option<String>,
    /// Why: `scroll`, `ttl`, `cleared`, `display_change`, or `session_end`.
    pub reason: String,
}

/// What `clear` hands back.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct Cleared {
    /// Whether the daemon accepted the request.
    pub cleared: bool,
    /// Marks of yours that had already gone away on their own. See [`Drawn::gone`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gone: Vec<Gone>,
}

/// Arguments to `point_at`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PointAtArgs {
    /// Horizontal position in logical points, from the left of the display. Pair with
    /// `y`, or omit both and use `at`.
    #[serde(default)]
    pub x: Option<f64>,
    /// Vertical position in logical points, from the top of the display. Pair with `x`,
    /// or omit both and use `at`.
    #[serde(default)]
    pub y: Option<f64>,
    /// A position named relative to the display, for when you have not measured it: one
    /// of "top-left", "top", "top-right", "left", "center", "right", "bottom-left",
    /// "bottom", "bottom-right", or a pair like "50%,30%". Use instead of x and y.
    #[serde(default)]
    pub at: Option<String>,
    /// Short caption drawn beside the orb, for example "Save".
    #[serde(default)]
    pub label: Option<String>,
    /// Remove the mark automatically after this many seconds. Omit to leave it up until
    /// you clear it or the session ends.
    #[serde(default)]
    pub ttl_seconds: Option<f64>,
    /// Display to draw on. Omit for the primary display.
    #[serde(default)]
    pub display: Option<u32>,
}

/// Arguments to `highlight`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct HighlightArgs {
    /// Left edge in logical points.
    pub x: f64,
    /// Top edge in logical points.
    pub y: f64,
    /// Width in logical points.
    pub width: f64,
    /// Height in logical points.
    pub height: f64,
    /// Short caption drawn against the region.
    #[serde(default)]
    pub label: Option<String>,
    /// Remove the mark automatically after this many seconds. Omit to leave it up until
    /// you clear it or the session ends.
    #[serde(default)]
    pub ttl_seconds: Option<f64>,
    /// Display to draw on. Omit for the primary display.
    #[serde(default)]
    pub display: Option<u32>,
}

/// Arguments to `annotate`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AnnotateArgs {
    /// Left edge in logical points.
    pub x: f64,
    /// Top edge in logical points.
    pub y: f64,
    /// Width in logical points.
    pub width: f64,
    /// Height in logical points.
    pub height: f64,
    /// The text to display. Rendered as a read-only panel, never an input.
    pub text: String,
    /// Remove the mark automatically after this many seconds. Omit to leave it up until
    /// you clear it or the session ends.
    #[serde(default)]
    pub ttl_seconds: Option<f64>,
    /// Display to draw on. Omit for the primary display.
    #[serde(default)]
    pub display: Option<u32>,
}

/// Arguments to `clear`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ClearArgs {
    /// The annotation to remove. Omit to remove everything this session drew.
    #[serde(default)]
    pub annotation_id: Option<String>,
}

/// The MCP server.
#[derive(Clone)]
pub struct Arin {
    /// One connection, shared across tool calls.
    ///
    /// The socket is a request and reply stream with no message ids, so a second call
    /// writing while the first is waiting would read the other's reply. The lock is what
    /// makes concurrent tool calls safe, and it is held only across a single round trip.
    client: Arc<Mutex<Client>>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Arin {
    /// Wrap a client that already has a session open.
    pub fn new(client: Client) -> Self {
        Self {
            client: Arc::new(Mutex::new(client)),
            tool_router: Self::tool_router(),
        }
    }

    /// Put the orb on a point on screen.
    #[tool(
        name = "point_at",
        description = "Point at something on the user's screen. Puts a glowing orb on the \
                       given position with an optional caption. Give either x and y in \
                       logical points from the top-left of the display, which is \
                       screenshot pixels divided by the display scale, or `at` with a \
                       named position like \"top-right\" or \"50%,30%\" when you have \
                       not measured the screen."
    )]
    async fn point_at(
        &self,
        Parameters(args): Parameters<PointAtArgs>,
    ) -> Result<Json<Drawn>, ErrorData> {
        let mut point = match (args.x, args.y, args.at) {
            (Some(x), Some(y), None) => Point::at(x, y, display(args.display)),
            (None, None, Some(at)) => Point::named(at, display(args.display)),
            _ => {
                return Err(ErrorData::invalid_params(
                    "point_at wants either x and y together, or at on its own",
                    None,
                ));
            }
        };
        point.label = args.label;
        point.ttl_ms = ttl_ms(args.ttl_seconds)?;
        self.draw(ClientMessage::Point(point)).await
    }

    /// Outline a region on screen.
    #[tool(
        name = "highlight",
        description = "Outline a rectangular region of the user's screen, with an \
                       optional caption. Use this for an area and point_at for a spot. \
                       Coordinates are logical points from the top-left of the display."
    )]
    async fn highlight(
        &self,
        Parameters(args): Parameters<HighlightArgs>,
    ) -> Result<Json<Drawn>, ErrorData> {
        let mut highlight = Highlight::over(
            LogicalRect::new(args.x, args.y, args.width, args.height),
            display(args.display),
        );
        highlight.label = args.label;
        highlight.ttl_ms = ttl_ms(args.ttl_seconds)?;
        self.draw(ClientMessage::Highlight(highlight)).await
    }

    /// Place a block of explanatory text.
    #[tool(
        name = "annotate",
        description = "Place a block of explanatory text on the user's screen. Display \
                       only: it is never an input, and the user cannot click or type into \
                       it. Use it for a sentence or two of explanation, placed next to \
                       whatever it describes."
    )]
    async fn annotate(
        &self,
        Parameters(args): Parameters<AnnotateArgs>,
    ) -> Result<Json<Drawn>, ErrorData> {
        let anchor = Anchor::new(
            LogicalRect::new(args.x, args.y, args.width, args.height),
            display(args.display),
        );
        let textbox = Textbox::new(anchor, args.text).with_ttl_ms(ttl_ms(args.ttl_seconds)?);
        self.draw(ClientMessage::Textbox(textbox)).await
    }

    /// Remove annotations this session drew.
    #[tool(
        name = "clear",
        description = "Remove annotations. Clears one mark by id, or every mark this \
                       session drew when no id is given. Only ever affects your own marks."
    )]
    async fn clear(
        &self,
        Parameters(args): Parameters<ClearArgs>,
    ) -> Result<Json<Cleared>, ErrorData> {
        let clear = match args.annotation_id {
            Some(id) => Clear::one(arin_protocol::AnnotationId::new(id)),
            None => Clear::all(),
        };

        match self.round_trip(ClientMessage::Clear(clear)).await? {
            DaemonMessage::Ack(_) => Ok(Json(Cleared {
                cleared: true,
                gone: self.gone().await,
            })),
            other => Err(refused(other)),
        }
    }

    /// Drain whatever the daemon pushed while we were not looking.
    ///
    /// MCP has no way for a server to interrupt a model, so an invalidation cannot be
    /// delivered when it happens. It rides along with the next tool result instead, which
    /// is the first moment the model is listening anyway.
    async fn gone(&self) -> Vec<Gone> {
        self.client
            .lock()
            .await
            .take_invalidations()
            .into_iter()
            .map(|event| Gone {
                annotation_id: event.annotation_id.map(|id| id.to_string()),
                reason: format!("{:?}", event.reason).to_lowercase(),
            })
            .collect()
    }

    /// Send a drawing message and describe what landed.
    async fn draw(&self, message: ClientMessage) -> Result<Json<Drawn>, ErrorData> {
        match self.round_trip(message).await? {
            DaemonMessage::Ack(ack) => {
                let display = ack.display;
                let gone = self.gone().await;
                Ok(Json(Drawn {
                    // A drawing ack always carries an id. Reporting an empty one beats
                    // failing a call whose mark is already on screen.
                    annotation_id: ack
                        .annotation_id
                        .map(|id| id.to_string())
                        .unwrap_or_default(),
                    display_width: display.map(|d| d.logical_size[0]),
                    display_height: display.map(|d| d.logical_size[1]),
                    display_scale: display.map(|d| d.scale),
                    gone,
                }))
            }
            other => Err(refused(other)),
        }
    }

    /// One request and its reply, holding the socket for the round trip.
    async fn round_trip(&self, message: ClientMessage) -> Result<DaemonMessage, ErrorData> {
        self.client
            .lock()
            .await
            .send(message)
            .await
            .map_err(|e| ErrorData::internal_error(format!("the arin daemon: {e}"), None))
    }
}

// Pointed at the stored router rather than the macro's default, which is to build a
// fresh one on every call. The tool set is fixed at construction, so rebuilding it per
// request is work with no result.
#[tool_handler(router = self.tool_router)]
impl ServerHandler for Arin {
    fn get_info(&self) -> ServerInfo {
        // Named and versioned here rather than through `Implementation::from_build_env`,
        // which reads the environment of whichever crate expanded it and so reports the
        // SDK's own version rather than Arin's.
        //
        // `ServerInfo` is `#[non_exhaustive]`, so it is filled in rather than built from
        // a struct literal. New fields then arrive with the SDK's own defaults.
        let server_info = Implementation::new(CLIENT_NAME, env!("CARGO_PKG_VERSION"));

        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = server_info;
        info.instructions = Some(
            "Arin draws on the user's screen so you can show them what you mean \
                 instead of describing it. Point at a line of code as you explain it, \
                 outline the region you are discussing, annotate what is on screen. It \
                 draws only: it never clicks, types, or scrolls, so use it to direct \
                 attention rather than to act. Coordinates are logical points from the \
                 top-left of a display, so if you are working from a screenshot, divide \
                 pixel coordinates by the display scale reported back to you. Clear your \
                 marks once they no longer describe what is on screen."
                .into(),
        );
        info
    }
}

/// The display a client named, or the default when it named none.
fn display(display: Option<u32>) -> DisplayId {
    display.map_or(DisplayId::DEFAULT, DisplayId)
}

/// Convert a time to live from the seconds a model thinks in to the milliseconds the
/// protocol carries.
///
/// Refused here rather than at the daemon so the model is told which argument was wrong
/// and in what unit. A NaN or a negative number reaching the wire would come back as a
/// schema error naming `ttl_ms`, a field the tool does not have.
fn ttl_ms(seconds: Option<f64>) -> Result<Option<u64>, ErrorData> {
    let Some(seconds) = seconds else {
        return Ok(None);
    };
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err(ErrorData::invalid_params(
            format!("ttl_seconds must be a positive number of seconds, got {seconds}"),
            None,
        ));
    }
    // Rounds up, so a sub-millisecond request asks for the shortest life the wire can
    // express rather than zero, which the daemon refuses.
    Ok(Some(((seconds * 1000.0).ceil() as u64).max(1)))
}

/// Turn a refusal into an MCP error the agent can act on.
///
/// The daemon's own message is passed through rather than summarised. Its errors say what
/// was wrong with the request, which is exactly what a model needs in order to send a
/// better one, and rewording it here would only lose detail.
fn refused(reply: DaemonMessage) -> ErrorData {
    match reply {
        DaemonMessage::Error(e) => {
            ErrorData::invalid_params(format!("arin refused this: {} ({})", e.msg, e.code), None)
        }
        DaemonMessage::Invalidated(inv) => ErrorData::internal_error(
            format!("the mark went away immediately: {:?}", inv.reason),
            None,
        ),
        DaemonMessage::Ack(_) => {
            ErrorData::internal_error("the daemon acked twice, which should not happen", None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_are_unique() {
        let mut sorted = tools::ALL.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), tools::ALL.len());
    }

    #[test]
    fn tool_names_are_snake_case() {
        for name in tools::ALL {
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{name} is not snake_case"
            );
        }
    }

    /// What is registered and what is documented must not drift apart.
    #[test]
    fn every_documented_tool_is_registered() {
        let mut registered: Vec<String> = Arin::tool_router()
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        registered.sort();

        let mut documented: Vec<String> = tools::ALL.iter().map(|&t| t.to_string()).collect();
        documented.sort();

        assert_eq!(registered, documented);
    }

    /// Every tool has to describe itself, since the description is what a model reads
    /// when it decides whether this is the tool it wants.
    #[test]
    fn every_tool_describes_itself() {
        for tool in Arin::tool_router().list_all() {
            let described = tool.description.as_ref().is_some_and(|d| d.len() > 40);
            assert!(described, "{} needs a description", tool.name);
        }
    }

    /// An agent that omits the display should still draw somewhere.
    #[test]
    fn an_unnamed_display_falls_back_to_the_default() {
        assert_eq!(display(None), DisplayId::DEFAULT);
        assert_eq!(display(Some(214)), DisplayId(214));
    }

    #[test]
    fn a_refusal_carries_the_daemons_own_words() {
        let error = refused(DaemonMessage::Error(arin_protocol::ProtocolError::new(
            arin_protocol::ErrorCode::UnknownDisplay,
            "no display with id 9",
        )));
        assert!(
            error.message.contains("no display with id 9"),
            "the daemon's message should survive, got {:?}",
            error.message
        );
        assert!(
            error.message.contains("unknown_display"),
            "the wire code should survive, got {:?}",
            error.message
        );
    }
}
