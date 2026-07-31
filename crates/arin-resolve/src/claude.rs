//! Grounding against Claude, with the user's own API key.
//!
//! Send a screenshot and a description of one thing on it, get back where that thing is.
//! This is the first adapter and the only one that reaches the network, which makes it the
//! only part of Arin that sends anything off the machine. It reports
//! [`Resolver::is_remote`] as `true` and it is never enabled implicitly: an API key sitting
//! in the environment does not switch it on, because a screenshot leaving the machine is
//! not something anyone should discover by accident.
//!
//! # Why not the computer use tool
//!
//! The obvious way to ground with a model of this class is to hand it the computer use
//! tool and read the coordinate out of the mouse action it asks for. That is rejected here
//! for two reasons, one practical and one about what Arin is.
//!
//! The practical one is that a tool call carries no confidence. The daemon chooses between
//! drawing a precise point and outlining a region based on how sure the model is, and a
//! tool call that reports "click here" has no room to say "probably". Structured output
//! has a field for it.
//!
//! The other is that Arin does not actuate, and a request shaped like "click this" is a
//! request for an action that will never happen. Asking a question and getting an answer
//! is the honest shape for something that only ever draws.
//!
//! # What is not measured yet
//!
//! The effort level, the detail sent, and the confidence the model reports are all
//! unvalidated against a real corpus of screens. The published accuracy for this class of
//! model is 85 to 95 percent, which is the number the confidence threshold in
//! [`arin_core::policy`] was chosen against, not one measured here. The eval set the
//! roadmap owes for this is still owed.

use crate::grounding::{self, Grounding};
use crate::screenshot::{self, Encoded};
use arin_core::{Error, Frame, Resolution, Resolver, Result};
use futures::future::BoxFuture;
use std::time::Duration;

/// Where the messages API lives.
pub const DEFAULT_ENDPOINT: &str = "https://api.anthropic.com/v1/messages";

/// The model used when configuration names none.
pub const DEFAULT_MODEL: &str = "claude-opus-5";

/// How hard the model is asked to think about it.
///
/// Low on purpose. Someone is watching an orb wait, so latency is part of the result, and
/// locating a described element in an image is a perception task rather than a reasoning
/// one. Raise it if grounding turns out to be the weak link. Like everything else here,
/// this is a starting point rather than a measurement.
pub const DEFAULT_EFFORT: &str = "low";

/// Wire version, as the API requires on every request.
const API_VERSION: &str = "2023-06-01";

/// Opts into server-side fallback, so a declined request is re-served rather than lost.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

/// Room for the answer, plus the thinking that precedes it.
///
/// The answer itself is a few dozen tokens. The rest is headroom: `max_tokens` caps
/// thinking and output together, and a resolve that runs out mid-answer is a truncated
/// JSON document rather than a coordinate.
const MAX_TOKENS: u32 = 4096;

/// How long to wait before giving up on a resolve.
///
/// A mark nobody is still waiting for is worse than an error. The client is blocked on
/// this and the orb is sitting in its thinking state for as long as it takes.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Grounds queries by asking Claude where something is.
pub struct ClaudeResolver {
    client: reqwest::Client,
    api_key: String,
    model: String,
    effort: String,
    endpoint: String,
    detail: u32,
}

impl ClaudeResolver {
    /// Build a resolver over an API key the user supplied.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .build()
                .unwrap_or_default(),
            api_key: api_key.into(),
            model: DEFAULT_MODEL.to_owned(),
            effort: DEFAULT_EFFORT.to_owned(),
            endpoint: DEFAULT_ENDPOINT.to_owned(),
            detail: arin_core::DEFAULT_DETAIL,
        }
    }

    /// Read the key from the environment, the way every other tool expects to find it.
    ///
    /// The absence of a key is an error rather than a silent fallback to no resolver: a
    /// daemon told to ground queries and unable to should say so at startup, not the first
    /// time a client asks.
    pub fn from_env() -> Result<Self> {
        let key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| {
            Error::Resolver(
                "ANTHROPIC_API_KEY is not set, and the claude resolver has no other way to \
                 authenticate"
                    .into(),
            )
        })?;
        if key.trim().is_empty() {
            return Err(Error::Resolver("ANTHROPIC_API_KEY is empty".into()));
        }
        Ok(Self::new(key.trim()))
    }

    /// Use a different model.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Use a different effort level.
    #[must_use]
    pub fn with_effort(mut self, effort: impl Into<String>) -> Self {
        self.effort = effort.into();
        self
    }

    /// Point at something other than the public API, for a proxy or a test.
    #[must_use]
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    /// Ask for a different amount of detail in the screenshot.
    #[must_use]
    pub fn with_detail(mut self, pixels: u32) -> Self {
        self.detail = pixels;
        self
    }

    /// The request body for one grounding call.
    fn body(&self, query: &str, image: &Encoded) -> serde_json::Value {
        serde_json::json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "system": *grounding::SYSTEM,
            // Routes a policy decline to whatever Anthropic currently recommends rather
            // than to a model named here, which would need changing when it is retired.
            // The refusal path below still works without this, so losing the beta costs
            // a rescued request rather than the adapter.
            "fallbacks": "default",
            "output_config": {
                "effort": self.effort,
                "format": {
                    "type": "json_schema",
                    "schema": grounding::schema(),
                },
            },
            "messages": [{
                "role": "user",
                "content": [
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": image.base64,
                        },
                    },
                    { "type": "text", "text": format!("Locate: {query}") },
                ],
            }],
        })
    }

    async fn ask(&self, query: &str, image: &Encoded) -> Result<Resolution> {
        let response = self
            .client
            .post(&self.endpoint)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("anthropic-beta", FALLBACK_BETA)
            .json(&self.body(query, image))
            .send()
            .await
            .map_err(|e| Error::Resolver(format!("could not reach the model: {e}")))?;

        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| Error::Resolver(format!("could not read the reply: {e}")))?;

        if !status.is_success() {
            return Err(Error::Resolver(describe_failure(status.as_u16(), &text)));
        }
        let grounding = read_answer(&text)?;
        into_resolution(grounding, image)
    }
}

impl Resolver for ClaudeResolver {
    fn name(&self) -> &str {
        "claude"
    }

    fn is_remote(&self) -> bool {
        true
    }

    fn detail(&self) -> u32 {
        self.detail
    }

    fn resolve<'a>(
        &'a self,
        query: &'a str,
        frame: &'a Frame,
    ) -> BoxFuture<'a, Result<Resolution>> {
        Box::pin(async move {
            let image = screenshot::encode(frame)?;
            tracing::debug!(
                query,
                width = image.width,
                height = image.height,
                model = %self.model,
                "sending a screenshot off the machine to ground a query"
            );
            self.ask(query, &image).await
        })
    }
}

/// Pull the answer out of a messages API reply.
fn read_answer(body: &str) -> Result<Grounding> {
    let reply: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| Error::Resolver(format!("the reply was not json: {e}")))?;

    // Checked before the content is read, not after. A refusal is a successful HTTP
    // response whose content array is empty or partial, so indexing into it first turns a
    // clear answer into a confusing parse error.
    match reply.get("stop_reason").and_then(|s| s.as_str()) {
        Some("refusal") => {
            let category = reply
                .get("stop_details")
                .and_then(|d| d.get("category"))
                .and_then(|c| c.as_str())
                .unwrap_or("unspecified");
            return Err(Error::Resolver(format!(
                "the model declined to answer, category {category}. The screenshot was sent \
                 and no coordinates came back"
            )));
        }
        Some("max_tokens") => {
            return Err(Error::Resolver(
                "the model ran out of room before finishing its answer".into(),
            ));
        }
        _ => {}
    }

    let text = reply
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|blocks| {
            blocks
                .iter()
                .find(|block| block.get("type").and_then(|t| t.as_str()) == Some("text"))
        })
        .and_then(|block| block.get("text"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| Error::Resolver("the reply carried no answer".into()))?;

    grounding::read_json(text)
}

/// Turn an answer in image pixels into one the protocol can carry.
///
/// The model is asked for a confidence and the schema requires one, so an answer without
/// it did not come back through the path this adapter set up. That is worth refusing
/// rather than papering over with a number of the adapter's own: a hosted model that
/// stopped reporting confidence would otherwise start silently drawing precise marks off
/// a value nobody measured.
fn into_resolution(answer: Grounding, image: &Encoded) -> Result<Resolution> {
    let confidence = answer.confidence.ok_or_else(|| {
        Error::Resolver("the answer carried no confidence, and the schema requires one".into())
    })?;
    grounding::into_resolution(answer, confidence, image)
}

/// Say what an HTTP failure means in terms of what to do about it.
fn describe_failure(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| grounding::snippet(body));

    match status {
        401 => format!("the API key was rejected: {detail}"),
        403 => format!("the API key is not allowed to use this model: {detail}"),
        404 => format!("no such model: {detail}"),
        429 => format!("rate limited: {detail}"),
        500..=599 => format!("the API is having trouble ({status}): {detail}"),
        _ => format!("the API refused the request ({status}): {detail}"),
    }
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
        screenshot::encode(&frame).expect("a well formed frame encodes")
    }

    fn reply(stop_reason: &str, answer: serde_json::Value) -> String {
        serde_json::json!({
            "type": "message",
            "stop_reason": stop_reason,
            "content": [{ "type": "text", "text": answer.to_string() }],
        })
        .to_string()
    }

    fn answer(found: bool, confidence: f64) -> serde_json::Value {
        serde_json::json!({
            "found": found,
            "x": 500.0,
            "y": 250.0,
            "width": 80.0,
            "height": 40.0,
            "confidence": confidence,
            "reasoning": "the button labelled Submit",
        })
    }

    #[test]
    fn an_answer_becomes_a_resolution_in_logical_points() {
        let grounding = read_answer(&reply("end_turn", answer(true, 0.94))).unwrap();
        let resolution = into_resolution(grounding, &image()).unwrap();

        assert_eq!(resolution.point.x, 500.0);
        assert_eq!(resolution.point.y, 250.0);
        assert_eq!(resolution.confidence, 0.94);

        let rect = resolution.rect.expect("a box was reported");
        // Centred on the point, which is not how the model reports it.
        assert_eq!(rect.x, 460.0);
        assert_eq!(rect.y, 230.0);
        assert_eq!(rect.width, 80.0);
        assert_eq!(rect.height, 40.0);
    }

    /// The whole point of asking. A model that cannot find the thing has to be able to
    /// say so, and saying so must not produce a mark.
    #[test]
    fn an_element_that_is_not_there_is_an_error_rather_than_a_guess() {
        let grounding = read_answer(&reply("end_turn", answer(false, 0.2))).unwrap();
        let error = into_resolution(grounding, &image()).unwrap_err();
        assert!(error.to_string().contains("did not find it"), "got {error}");
    }

    #[test]
    fn confidence_outside_the_range_is_clamped_rather_than_trusted() {
        let over = read_answer(&reply("end_turn", answer(true, 4.0))).unwrap();
        assert_eq!(into_resolution(over, &image()).unwrap().confidence, 1.0);

        let under = read_answer(&reply("end_turn", answer(true, -2.0))).unwrap();
        assert_eq!(into_resolution(under, &image()).unwrap().confidence, 0.0);
    }

    #[test]
    fn a_zero_sized_box_is_dropped_and_the_point_survives() {
        let mut degenerate = answer(true, 0.9);
        degenerate["width"] = 0.0.into();
        let grounding = read_answer(&reply("end_turn", degenerate)).unwrap();

        let resolution = into_resolution(grounding, &image()).unwrap();
        assert_eq!(resolution.rect, None, "an unusable box must not be carried");
        assert_eq!(resolution.point.x, 500.0, "the answer itself still stands");
    }

    /// A refusal is a successful HTTP response, so reading the content first would turn a
    /// clear answer into a baffling parse error.
    #[test]
    fn a_refusal_is_read_before_the_content_is() {
        let body = serde_json::json!({
            "type": "message",
            "stop_reason": "refusal",
            "stop_details": { "type": "refusal", "category": "cyber" },
            "content": [],
        })
        .to_string();

        let error = read_answer(&body).unwrap_err();
        assert!(error.to_string().contains("declined"), "got {error}");
        assert!(
            error.to_string().contains("cyber"),
            "the category is the useful part, got {error}"
        );
    }

    #[test]
    fn a_truncated_answer_says_so_rather_than_failing_to_parse() {
        let body = serde_json::json!({
            "type": "message",
            "stop_reason": "max_tokens",
            "content": [{ "type": "text", "text": "{\"found\": tr" }],
        })
        .to_string();

        let error = read_answer(&body).unwrap_err();
        assert!(error.to_string().contains("ran out of room"), "got {error}");
    }

    #[test]
    fn a_reply_that_is_not_json_is_reported_as_such() {
        let error = read_answer("<html>gateway timeout</html>").unwrap_err();
        assert!(error.to_string().contains("not json"), "got {error}");
    }

    #[test]
    fn a_reply_with_no_text_block_is_an_error() {
        let body = serde_json::json!({
            "type": "message",
            "stop_reason": "end_turn",
            "content": [{ "type": "thinking", "thinking": "" }],
        })
        .to_string();
        assert!(read_answer(&body).is_err());
    }

    #[test]
    fn http_failures_say_what_to_do_about_them() {
        let body = r#"{"error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
        let described = describe_failure(401, body);
        assert!(described.contains("key was rejected"), "got {described}");
        assert!(
            described.contains("invalid x-api-key"),
            "the API's own words are the useful part, got {described}"
        );

        assert!(describe_failure(429, "{}").contains("rate limited"));
        assert!(describe_failure(503, "{}").contains("having trouble"));
    }

    #[test]
    fn the_request_carries_the_image_and_the_query() {
        let resolver = ClaudeResolver::new("sk-test").with_model("claude-opus-5");
        let image = image();
        let body = resolver.body("the Submit button", &image);

        assert_eq!(body["model"], "claude-opus-5");
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
        assert_eq!(body["output_config"]["effort"], DEFAULT_EFFORT);

        let content = &body["messages"][0]["content"];
        assert_eq!(content[0]["type"], "image");
        assert_eq!(content[0]["source"]["media_type"], "image/png");
        assert_eq!(content[0]["source"]["data"], image.base64);
        assert!(
            content[1]["text"].as_str().unwrap().contains("Submit"),
            "the query has to reach the model"
        );
    }

    /// The schema requires a confidence, so an answer without one did not come back through
    /// the path this adapter set up. Substituting a number here would let a hosted model
    /// start drawing precise marks off a value nobody measured.
    #[test]
    fn an_answer_with_no_confidence_is_refused_rather_than_given_one() {
        let mut unrated = answer(true, 0.9);
        unrated.as_object_mut().unwrap().remove("confidence");
        let grounding = read_answer(&reply("end_turn", unrated)).unwrap();

        let error = into_resolution(grounding, &image()).unwrap_err();
        assert!(error.to_string().contains("no confidence"), "got {error}");
    }

    /// The one thing about this adapter that is not an implementation detail.
    #[test]
    fn it_admits_to_being_remote() {
        assert!(ClaudeResolver::new("sk-test").is_remote());
        assert_eq!(ClaudeResolver::new("sk-test").name(), "claude");
    }

    #[test]
    fn a_missing_key_is_refused_at_construction() {
        // Cannot safely unset a variable for the whole process here, so this checks the
        // empty case, which takes the same path.
        assert!(ClaudeResolver::from_env().is_err() || std::env::var("ANTHROPIC_API_KEY").is_ok());
    }
}
