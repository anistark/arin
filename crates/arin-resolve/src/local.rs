//! Grounding against a model running on this machine.
//!
//! The adapter 0.4 exists for. It removes the API key and, more to the point, it removes
//! the one path by which a screenshot of the user's display leaves the machine. It reports
//! [`Resolver::is_remote`] as `false`, and that claim is enforced rather than asserted: the
//! endpoint is checked at construction and anything that is not loopback is refused, so a
//! resolver reporting `false` cannot be pointed at a remote server by editing an
//! environment variable.
//!
//! # Why an OpenAI shaped request rather than a named runtime
//!
//! There is no standard way to run a vision model locally and there are five popular ones,
//! but they agree on an HTTP surface: LM Studio, vLLM, llama.cpp's server, SGLang and
//! Ollama all serve `/v1/chat/completions` with the same body. Writing to that covers all
//! of them and leaves the choice of runtime to the person who has to install it. What it
//! costs is a default port that is right for only one of them, which is why the failure
//! message names the others.
//!
//! # What a UI TARS class model answers with
//!
//! Two shapes, and both are accepted.
//!
//! A general vision model constrained to a schema answers with the same JSON object the
//! hosted adapter asks for, confidence included. That is the better path and it is what is
//! requested by default.
//!
//! A UI TARS checkpoint is fine tuned to emit an action instead, `click(point='<point>512
//! 384</point>')`, and will do that whatever the prompt says. It is the format the model
//! was trained on, so it is also where the model is most accurate. Refusing it would mean
//! this adapter did not support the class of model it was built for.
//!
//! The catch is the one already recorded in `AGENTS.md` as the reason the computer use tool
//! was rejected for grounding: **an action carries no confidence.** The daemon chooses
//! between a precise mark and a cautious region on that number, and there is nothing
//! honest to put there. So an answer with no confidence gets
//! [`ASSUMED_CONFIDENCE`], which sits below the threshold on purpose: an answer nobody can
//! rate draws a region. That is a placeholder for a measurement, and the eval set this
//! cycle owes is what should replace it.

use crate::grounding::{self, Grounding};
use crate::screenshot::{self, Encoded};
use arin_core::{Error, Frame, Resolution, Resolver, Result};
use futures::future::BoxFuture;
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// Where a local server is looked for when nothing says otherwise.
///
/// LM Studio's port, because it is the shortest path from nothing to a vision model
/// serving on a Mac. Every other runtime uses a different one and none of them is more
/// standard than the rest, so this is a guess that has to be easy to correct: see
/// [`OTHER_PORTS`], which is what the failure message lists.
pub const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:1234/v1/chat/completions";

/// Ports the runtimes this adapter speaks to listen on out of the box.
///
/// Only ever printed. Probing them would make the daemon guess which model the user meant,
/// and starting a resolve against whatever happened to answer is not a decision this
/// should make on someone's behalf.
pub const OTHER_PORTS: &[(&str, u16)] = &[
    ("LM Studio", 1234),
    ("Ollama", 11434),
    ("vLLM and SGLang", 8000),
    ("llama.cpp server", 8080),
];

/// The model asked for when configuration names none.
///
/// A name rather than a file. Every runtime here identifies a loaded model by string, and
/// several ignore the field entirely and serve whatever is loaded, which is why a wrong
/// value fails with a list of what the server does have rather than silently.
pub const DEFAULT_MODEL: &str = "ui-tars-1.5-7b";

/// Confidence given to an answer that reported none.
///
/// Below [`arin_core::policy::HIGH_CONFIDENCE`] deliberately, so a model that answers with
/// an action rather than a rated JSON object draws a region. **This is not a measurement.**
/// It is the value that makes an unrated answer behave the way an uncertain one does, which
/// is the conservative reading and the one that cannot put a confident mark on the wrong
/// button.
pub const ASSUMED_CONFIDENCE: f64 = 0.5;

/// Longest edge sent to a local model, in pixels.
///
/// Lower than the hosted ceiling, and for a different reason. A hosted model has a hard
/// limit and no marginal cost to the user under it. A local one has no limit and nothing
/// but marginal cost: pixels are seconds of the machine's own GPU, on a resolve somebody is
/// watching an orb wait through. 1280 keeps interface text legible on a laptop display.
/// Whether it keeps it legible enough is the question the eval set answers, so this is a
/// starting point rather than a finding.
pub const MAX_EDGE: u32 = 1280;

/// How long to wait before giving up on a resolve.
///
/// Longer than the hosted adapter's, because the failure it guards against is different. A
/// hosted request that has not answered in thirty seconds has gone wrong. A 7B model on a
/// laptop that has not answered in thirty seconds may simply be a 7B model on a laptop.
const TIMEOUT: Duration = Duration::from_secs(120);

/// How long to wait for the server to accept a connection at construction.
///
/// Short on purpose. This runs while the daemon is starting up and the only question it
/// asks is whether anything is listening, which loopback answers immediately or not at all.
const PROBE_TIMEOUT: Duration = Duration::from_millis(300);

/// Room for the answer.
///
/// Smaller than the hosted adapter's, which has to leave room for thinking. Nothing here
/// thinks: the answer is one JSON object or one action, both a few dozen tokens, and a
/// local model given four thousand tokens of rope will use them.
const MAX_TOKENS: u32 = 512;

/// What the numbers in an answer are measured in.
///
/// The one thing about this class of model that cannot be inferred. UI TARS 1.5 reports
/// pixels of the image it was given. The 1.0 checkpoints report thousandths of the image's
/// width and height, and both answer in the same shape, so a wrong setting here is a mark
/// in the top left corner rather than an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CoordinateSpace {
    /// Pixels of the image the model was sent. What the prompt asks for.
    #[default]
    Pixels,
    /// Thousandths of the image's width and height, as UI TARS 1.0 reports.
    Normalized,
}

impl CoordinateSpace {
    /// Read a space from configuration.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "pixels" | "pixel" | "absolute" => Ok(Self::Pixels),
            "normalized" | "normalised" | "thousandths" => Ok(Self::Normalized),
            other => Err(Error::Resolver(format!(
                "{other:?} is not a coordinate space. Use `pixels` or `normalized`"
            ))),
        }
    }

    /// Convert an answer into pixels of the image the model was sent.
    fn into_pixels(self, mut answer: Grounding, image: &Encoded) -> Grounding {
        if self == Self::Pixels {
            return answer;
        }
        let per_x = f64::from(image.width) / 1000.0;
        let per_y = f64::from(image.height) / 1000.0;
        answer.x *= per_x;
        answer.y *= per_y;
        answer.width *= per_x;
        answer.height *= per_y;
        answer
    }
}

/// Grounds queries against a model served on this machine.
pub struct LocalResolver {
    client: reqwest::Client,
    endpoint: String,
    address: SocketAddr,
    model: String,
    space: CoordinateSpace,
    structured: bool,
    max_edge: u32,
}

impl LocalResolver {
    /// Build a resolver against a local endpoint.
    ///
    /// Fails when the endpoint is not loopback, which is what makes [`Resolver::is_remote`]
    /// returning `false` a fact about this resolver rather than a claim it makes.
    pub fn new(endpoint: impl Into<String>) -> Result<Self> {
        let endpoint = endpoint.into();
        let address = loopback_address(&endpoint)?;
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(TIMEOUT)
                // Nothing on loopback needs one, and a proxy configured for the wider
                // network is a way for a request meant to stay here to leave.
                .no_proxy()
                .build()
                .unwrap_or_default(),
            endpoint,
            address,
            model: DEFAULT_MODEL.to_owned(),
            space: CoordinateSpace::default(),
            structured: true,
            max_edge: MAX_EDGE,
        })
    }

    /// Read the configuration from the environment, and check something is there.
    ///
    /// The connection check is the point. A daemon told to ground queries and unable to
    /// should say so at startup rather than the first time a client asks, and unlike an API
    /// key the failure here is nearly always "the server is not running", which is worth
    /// hearing while the terminal that started it is still open.
    pub fn from_env() -> Result<Self> {
        let endpoint = std::env::var("ARIN_LOCAL_ENDPOINT")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_ENDPOINT.to_owned());

        let mut resolver = Self::new(endpoint.trim())?;

        if let Ok(model) = std::env::var("ARIN_LOCAL_MODEL")
            && !model.trim().is_empty()
        {
            resolver.model = model.trim().to_owned();
        }
        if let Ok(space) = std::env::var("ARIN_LOCAL_COORDS")
            && !space.trim().is_empty()
        {
            resolver.space = CoordinateSpace::parse(&space)?;
        }
        if let Ok(structured) = std::env::var("ARIN_LOCAL_STRUCTURED") {
            resolver.structured = !matches!(
                structured.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            );
        }

        resolver.check_reachable()?;
        Ok(resolver)
    }

    /// Ask whether anything is listening, without speaking HTTP to it.
    ///
    /// A blocking connect rather than a request, so this stays callable from the
    /// synchronous builder the registry holds. It is one connection to loopback with a
    /// short timeout, and it answers the only question worth asking this early.
    pub fn check_reachable(&self) -> Result<()> {
        TcpStream::connect_timeout(&self.address, PROBE_TIMEOUT).map_err(|e| {
            let ports = OTHER_PORTS
                .iter()
                .map(|(name, port)| format!("{name} {port}"))
                .collect::<Vec<_>>()
                .join(", ");
            Error::Resolver(format!(
                "nothing is listening at {} ({e}). Start your model server, or set \
                 ARIN_LOCAL_ENDPOINT if it is somewhere else. The usual ports are {ports}",
                self.address
            ))
        })?;
        Ok(())
    }

    /// Ask for a different model.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Read answers in a different coordinate space.
    #[must_use]
    pub fn with_coordinate_space(mut self, space: CoordinateSpace) -> Self {
        self.space = space;
        self
    }

    /// Stop asking the server to constrain the answer to the schema.
    ///
    /// Some runtimes reject `response_format` outright rather than ignoring it. Turning it
    /// off costs the confidence field, which puts every answer on [`ASSUMED_CONFIDENCE`]
    /// and therefore draws regions.
    #[must_use]
    pub fn with_structured_output(mut self, structured: bool) -> Self {
        self.structured = structured;
        self
    }

    /// Send a different amount of detail.
    #[must_use]
    pub fn with_max_edge(mut self, pixels: u32) -> Self {
        self.max_edge = pixels.max(1);
        self
    }

    /// The endpoint this resolver talks to.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The request body for one grounding call.
    fn body(&self, query: &str, image: &Encoded) -> serde_json::Value {
        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            // Locating something in an image has one right answer, so there is nothing for
            // sampling to explore. Unlike the hosted adapter, which is talking to a model
            // that refuses the parameter, every runtime here expects it.
            "temperature": 0.0,
            "stream": false,
            "messages": [
                { "role": "system", "content": *grounding::SYSTEM },
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "image_url",
                            "image_url": {
                                "url": format!("data:image/png;base64,{}", image.base64),
                            },
                        },
                        { "type": "text", "text": format!("Locate: {query}") },
                    ],
                },
            ],
        });

        if self.structured {
            body["response_format"] = serde_json::json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "grounding",
                    "strict": true,
                    "schema": grounding::schema(),
                },
            });
        }
        body
    }

    async fn ask(&self, query: &str, image: &Encoded) -> Result<Resolution> {
        let response = self
            .client
            .post(&self.endpoint)
            .json(&self.body(query, image))
            .send()
            .await
            .map_err(|e| Error::Resolver(format!("could not reach {}: {e}", self.endpoint)))?;

        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| Error::Resolver(format!("could not read the reply: {e}")))?;

        if !status.is_success() {
            return Err(Error::Resolver(
                self.describe_failure(status.as_u16(), &text),
            ));
        }

        let (answer, rated) = read_answer(&text)?;
        let confidence = answer.confidence.unwrap_or(ASSUMED_CONFIDENCE);
        if !rated {
            tracing::debug!(
                assumed = ASSUMED_CONFIDENCE,
                "the model answered with an action and no confidence, so this draws a region"
            );
        }
        let answer = self.space.into_pixels(answer, image);
        check_within(&answer, image, self.space)?;
        grounding::into_resolution(answer, confidence, image)
    }

    /// Say what an HTTP failure means in terms of what to do about it.
    ///
    /// Every one of these is a local misconfiguration rather than a service problem, so the
    /// message names the setting to change rather than suggesting a retry.
    fn describe_failure(&self, status: u16, body: &str) -> String {
        let detail = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| {
                v.get("error")
                    .and_then(|e| e.get("message").or(Some(e)))
                    .and_then(|m| m.as_str().map(str::to_owned))
            })
            .unwrap_or_else(|| grounding::snippet(body));

        match status {
            400 if self.structured => format!(
                "the server refused the request ({status}): {detail}. If it does not \
                 support `response_format`, set ARIN_LOCAL_STRUCTURED=0. That costs the \
                 confidence field, so marks become regions"
            ),
            404 => format!(
                "no model named {:?} at {} ({status}): {detail}. Set ARIN_LOCAL_MODEL to \
                 one the server has loaded",
                self.model, self.endpoint
            ),
            _ => format!("the server refused the request ({status}): {detail}"),
        }
    }
}

impl Resolver for LocalResolver {
    fn name(&self) -> &str {
        "local"
    }

    /// Nothing leaves the machine. Checked at construction, not promised here.
    fn is_remote(&self) -> bool {
        false
    }

    fn detail(&self) -> u32 {
        self.max_edge
    }

    fn resolve<'a>(
        &'a self,
        query: &'a str,
        frame: &'a Frame,
    ) -> BoxFuture<'a, Result<Resolution>> {
        Box::pin(async move {
            let image = screenshot::encode_within(frame, self.max_edge)?;
            tracing::debug!(
                query,
                width = image.width,
                height = image.height,
                model = %self.model,
                endpoint = %self.endpoint,
                "grounding against a model on this machine"
            );
            self.ask(query, &image).await
        })
    }
}

/// Resolve an endpoint to the loopback address it names, or refuse it.
///
/// A hostname that resolves to loopback today is still refused, because what it resolves to
/// is not this process's to control. The check has to be about the address written down,
/// not the one DNS currently answers with.
fn loopback_address(endpoint: &str) -> Result<SocketAddr> {
    let url = reqwest::Url::parse(endpoint)
        .map_err(|e| Error::Resolver(format!("{endpoint:?} is not a url: {e}")))?;

    // Brackets because a url spells an IPv6 host `[::1]`, and `IpAddr` does not.
    let host = url.host_str().unwrap_or_default();
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let local = bare
        .parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or_else(|_| bare.eq_ignore_ascii_case("localhost"));
    if !local {
        return Err(Error::Resolver(format!(
            "{endpoint:?} is not on this machine. The local resolver reports that it sends \
             nothing off the machine, so it will only talk to 127.0.0.1, ::1, or localhost. \
             Use the claude resolver if you meant to ground against a hosted model"
        )));
    }

    let port = url
        .port_or_known_default()
        .ok_or_else(|| Error::Resolver(format!("{endpoint:?} names no port")))?;

    url.socket_addrs(|| Some(port))
        .ok()
        .and_then(|addrs| addrs.into_iter().find(|addr| addr.ip().is_loopback()))
        .ok_or_else(|| {
            Error::Resolver(format!(
                "{endpoint:?} does not resolve to a loopback address"
            ))
        })
}

/// Pull an answer out of a chat completions reply, and say whether the model rated it.
///
/// The boolean is what separates a confidence the model reported from one this adapter
/// substituted, which is a distinction worth keeping: one is evidence and the other is a
/// default.
fn read_answer(body: &str) -> Result<(Grounding, bool)> {
    let reply: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| Error::Resolver(format!("the reply was not json: {e}")))?;

    let choice = reply
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|choices| choices.first())
        .ok_or_else(|| Error::Resolver("the reply carried no answer".into()))?;

    if choice.get("finish_reason").and_then(|r| r.as_str()) == Some("length") {
        return Err(Error::Resolver(
            "the model ran out of room before finishing its answer".into(),
        ));
    }

    let text = choice
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| Error::Resolver("the reply carried no answer".into()))?;

    // The action format first. A UI TARS checkpoint emits it whatever the prompt asked
    // for, and trying JSON on it produces a parse error that says nothing useful.
    if let Some(action) = read_action(text) {
        return Ok((action, false));
    }
    let answer = grounding::read_json(text)?;
    let rated = answer.confidence.is_some();
    Ok((answer, rated))
}

/// Read a UI TARS action, if that is what this is.
///
/// `click(point='<point>512 384</point>')` and `click(start_box='(496,370,528,398)')` are
/// the two shapes in the wild, with the box variant reporting corners rather than a centre.
/// Returns `None` for anything that is not an action, so the JSON path gets a look.
fn read_action(text: &str) -> Option<Grounding> {
    const MARKERS: &[&str] = &["point='", "start_box='", "start_point='", "<point>"];

    let (at, marker) = MARKERS
        .iter()
        .filter_map(|marker| text.find(marker).map(|at| (at, *marker)))
        .min_by_key(|(at, _)| *at)?;

    let rest = &text[at + marker.len()..];
    let end = rest
        .find('\'')
        .or_else(|| rest.find("</point>"))
        .unwrap_or(rest.len());
    let numbers: Vec<f64> = rest[..end]
        .replace("<|box_start|>", " ")
        .replace("<|box_end|>", " ")
        .replace(['(', ')', '[', ']', '<', '>', ','], " ")
        .split_whitespace()
        .filter_map(|token| token.parse::<f64>().ok())
        .collect();

    // Whatever the model said before the action, which is the closest thing to a reason it
    // offers. Capped, because a checkpoint asked for JSON and answering with an action has
    // usually written a paragraph about it first.
    let before = text.find("Action:").map_or(&text[..at], |i| &text[..i]);
    let reasoning = grounding::snippet(before.trim().trim_start_matches("Thought:").trim());

    match numbers[..] {
        [x, y] => Some(Grounding {
            found: true,
            x,
            y,
            width: 0.0,
            height: 0.0,
            confidence: None,
            reasoning,
        }),
        [x1, y1, x2, y2] => Some(Grounding {
            found: true,
            x: (x1 + x2) / 2.0,
            y: (y1 + y2) / 2.0,
            width: (x2 - x1).abs(),
            height: (y2 - y1).abs(),
            confidence: None,
            reasoning,
        }),
        _ => None,
    }
}

/// Refuse an answer that falls outside the image it is supposed to describe.
///
/// This is what catches the one setting that cannot be inferred. A model answering in
/// thousandths while [`CoordinateSpace::Pixels`] is configured puts every mark in the top
/// left ninth of the screen, which looks like bad grounding rather than a misconfiguration.
/// The reverse overshoots the image, and that is detectable, so it is detected and named.
fn check_within(answer: &Grounding, image: &Encoded, space: CoordinateSpace) -> Result<()> {
    let (width, height) = (f64::from(image.width), f64::from(image.height));
    if answer.x >= 0.0 && answer.x <= width && answer.y >= 0.0 && answer.y <= height {
        return Ok(());
    }
    Err(Error::Resolver(format!(
        "the model answered {:.0},{:.0}, which is outside the {width:.0}x{height:.0} image \
         it was sent. Coordinates are being read as {space:?}: set ARIN_LOCAL_COORDS to the \
         other one if the model reports thousandths",
        answer.x, answer.y
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use arin_protocol::DisplayId;
    use std::sync::Arc;

    /// A frame that encodes one to one, so image pixels and logical points coincide and a
    /// conversion error shows up as a difference rather than hiding in a ratio.
    fn image() -> Encoded {
        let frame = Frame {
            display: DisplayId(1),
            scale: 2.0,
            logical_size: [1000.0, 500.0],
            width: 1000,
            height: 500,
            pixels: Arc::from(vec![0u8; 1000 * 500 * 4]),
        };
        screenshot::encode_within(&frame, 1000).expect("a well formed frame encodes")
    }

    fn reply(content: &str) -> String {
        serde_json::json!({
            "choices": [{
                "finish_reason": "stop",
                "message": { "role": "assistant", "content": content },
            }],
        })
        .to_string()
    }

    /// The claim the whole adapter rests on, and the only one that has to be enforced
    /// rather than documented. A resolver that answers `false` here while making a request
    /// off the machine defeats the entire privacy story.
    #[test]
    fn it_refuses_to_be_pointed_anywhere_but_this_machine() {
        for endpoint in [
            "http://api.example.com/v1/chat/completions",
            "https://api.anthropic.com/v1/messages",
            "http://192.168.1.10:1234/v1/chat/completions",
            "http://10.0.0.1:8000/v1/chat/completions",
        ] {
            let error = LocalResolver::new(endpoint)
                .err()
                .unwrap_or_else(|| panic!("{endpoint} must be refused"))
                .to_string();
            assert!(error.contains("not on this machine"), "got {error}");
        }
    }

    #[test]
    fn loopback_in_its_several_spellings_is_accepted() {
        for endpoint in [
            "http://127.0.0.1:1234/v1/chat/completions",
            "http://localhost:11434/v1/chat/completions",
            "http://[::1]:8000/v1/chat/completions",
            "http://127.0.0.2:8080/v1/chat/completions",
        ] {
            let resolver = LocalResolver::new(endpoint)
                .unwrap_or_else(|e| panic!("{endpoint} should be accepted, got {e}"));
            assert!(!resolver.is_remote());
            assert_eq!(resolver.name(), "local");
        }
    }

    #[test]
    fn a_url_with_no_scheme_is_refused_rather_than_guessed_at() {
        assert!(LocalResolver::new("127.0.0.1:1234").is_err());
    }

    /// The format a UI TARS checkpoint actually emits, whatever the prompt asked for.
    #[test]
    fn a_ui_tars_point_action_is_read() {
        let action = read_action("Thought: The Submit button is at the bottom right.\nAction: click(point='<point>512 384</point>')")
            .expect("an action is recognised");
        assert_eq!((action.x, action.y), (512.0, 384.0));
        assert_eq!(
            action.confidence, None,
            "an action carries no confidence, and inventing one here would be a lie"
        );
        assert!(action.reasoning.contains("Submit button"));
    }

    #[test]
    fn a_box_action_reports_a_centre_and_a_size() {
        let action = read_action("Action: click(start_box='(496,370,528,398)')")
            .expect("a box action is recognised");
        assert_eq!((action.x, action.y), (512.0, 384.0));
        assert_eq!((action.width, action.height), (32.0, 28.0));
    }

    #[test]
    fn the_special_tokens_some_checkpoints_wrap_a_box_in_are_stripped() {
        let action =
            read_action("click(start_box='<|box_start|>(100,200)<|box_end|>')").expect("read");
        assert_eq!((action.x, action.y), (100.0, 200.0));
    }

    #[test]
    fn json_is_not_mistaken_for_an_action() {
        assert!(read_action(r#"{"found":true,"x":10,"y":20,"confidence":0.9}"#).is_none());
    }

    /// Both answer shapes reach a resolution, and only one of them carries a rating.
    #[test]
    fn a_rated_answer_keeps_its_confidence_and_an_action_gets_the_assumed_one() {
        let (rated, was_rated) = read_answer(&reply(
            r#"{"found":true,"x":500,"y":250,"width":80,"height":40,"confidence":0.93,"reasoning":"the Submit button"}"#,
        ))
        .unwrap();
        assert!(was_rated);
        assert_eq!(rated.confidence, Some(0.93));

        let (action, was_rated) =
            read_answer(&reply("Action: click(point='<point>500 250</point>')")).unwrap();
        assert!(!was_rated);
        assert_eq!(action.confidence, None);
        assert_eq!(
            arin_core::Rendering::for_confidence(ASSUMED_CONFIDENCE),
            arin_core::Rendering::Region,
            "an unrated answer has to draw a region, which is what this constant is for"
        );
    }

    #[test]
    fn thousandths_convert_against_the_image_that_was_sent() {
        let image = image();
        let answer = Grounding {
            found: true,
            x: 500.0,
            y: 500.0,
            width: 100.0,
            height: 100.0,
            confidence: None,
            reasoning: String::new(),
        };
        let converted = CoordinateSpace::Normalized.into_pixels(answer, &image);
        // Half of a 1000 pixel width and all of a 500 pixel height.
        assert_eq!((converted.x, converted.y), (500.0, 250.0));
        assert_eq!((converted.width, converted.height), (100.0, 50.0));
    }

    /// The failure a wrong coordinate space produces in the detectable direction. The other
    /// direction is undetectable, which is why this one is worth naming precisely.
    #[test]
    fn an_answer_outside_the_image_names_the_setting_that_explains_it() {
        let image = image();
        let outside = Grounding {
            found: true,
            x: 940.0,
            y: 780.0,
            width: 0.0,
            height: 0.0,
            confidence: None,
            reasoning: String::new(),
        };
        let error = check_within(&outside, &image, CoordinateSpace::Pixels).unwrap_err();
        assert!(
            error.to_string().contains("ARIN_LOCAL_COORDS"),
            "got {error}"
        );

        let inside = Grounding {
            y: 400.0,
            ..outside
        };
        assert!(check_within(&inside, &image, CoordinateSpace::Pixels).is_ok());
    }

    #[test]
    fn a_coordinate_space_is_read_from_configuration() {
        assert_eq!(
            CoordinateSpace::parse("pixels").unwrap(),
            CoordinateSpace::Pixels
        );
        assert_eq!(
            CoordinateSpace::parse(" Normalised ").unwrap(),
            CoordinateSpace::Normalized
        );
        assert!(CoordinateSpace::parse("percent").is_err());
    }

    #[test]
    fn the_request_carries_the_image_the_query_and_the_schema() {
        let resolver = LocalResolver::new(DEFAULT_ENDPOINT).unwrap();
        let image = image();
        let body = resolver.body("the Submit button", &image);

        assert_eq!(body["model"], DEFAULT_MODEL);
        assert_eq!(body["temperature"], 0.0);
        assert_eq!(body["response_format"]["type"], "json_schema");

        let content = &body["messages"][1]["content"];
        assert_eq!(content[0]["type"], "image_url");
        assert!(
            content[0]["image_url"]["url"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,"),
            "runtimes here want a data url rather than a bare base64 field"
        );
        assert!(content[1]["text"].as_str().unwrap().contains("Submit"));
    }

    /// A runtime that rejects `response_format` is the likeliest first failure, and the fix
    /// is one environment variable, so the error says which one.
    #[test]
    fn a_refused_response_format_says_how_to_turn_it_off() {
        let resolver = LocalResolver::new(DEFAULT_ENDPOINT).unwrap();
        let described = resolver.describe_failure(400, r#"{"error":{"message":"unknown field"}}"#);
        assert!(
            described.contains("ARIN_LOCAL_STRUCTURED=0"),
            "got {described}"
        );

        let plain = resolver
            .with_structured_output(false)
            .describe_failure(400, "{}");
        assert!(
            !plain.contains("ARIN_LOCAL_STRUCTURED"),
            "suggesting a setting that is already off is noise, got {plain}"
        );
    }

    #[test]
    fn a_missing_model_names_the_setting_that_chooses_one() {
        let resolver = LocalResolver::new(DEFAULT_ENDPOINT).unwrap();
        let described =
            resolver.describe_failure(404, r#"{"error":{"message":"model not found"}}"#);
        assert!(described.contains("ARIN_LOCAL_MODEL"), "got {described}");
    }

    #[test]
    fn a_truncated_answer_says_so_rather_than_failing_to_parse() {
        let body = serde_json::json!({
            "choices": [{
                "finish_reason": "length",
                "message": { "content": "{\"found\": tr" },
            }],
        })
        .to_string();
        let error = read_answer(&body).unwrap_err();
        assert!(error.to_string().contains("ran out of room"), "got {error}");
    }

    #[test]
    fn a_reply_that_is_not_json_is_reported_as_such() {
        let error = read_answer("<html>502 Bad Gateway</html>").unwrap_err();
        assert!(error.to_string().contains("not json"), "got {error}");
    }

    /// The message someone sees before they have anything running, which is the first thing
    /// most people will hit.
    #[test]
    fn an_unreachable_server_says_where_it_looked_and_what_else_to_try() {
        // Port 1 on loopback, which nothing serves.
        let resolver = LocalResolver::new("http://127.0.0.1:1/v1/chat/completions").unwrap();
        let error = resolver
            .check_reachable()
            .expect_err("nothing listens on port 1")
            .to_string();
        assert!(error.contains("127.0.0.1:1"), "got {error}");
        assert!(error.contains("Ollama"), "got {error}");
        assert!(error.contains("ARIN_LOCAL_ENDPOINT"), "got {error}");
    }
}
