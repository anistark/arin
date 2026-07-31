//! What a grounding model is asked for, and how its answer becomes a mark.
//!
//! Every adapter asks the same question and does the same thing with the reply. What
//! differs is the API it asks through and the envelope the answer arrives in. Keeping the
//! instructions, the schema, the range checks, and the one coordinate conversion here is
//! what stops two adapters drifting into disagreeing about which corner an answer is
//! measured from or what a confidence of 0.9 buys.

use crate::screenshot::Encoded;
use arin_core::{Error, Resolution, Result, policy::HIGH_CONFIDENCE};
use serde::Deserialize;
use std::sync::LazyLock;

/// What the model is asked to do, and how to answer.
///
/// The threshold is interpolated rather than written out, because a prompt that tells the
/// model the wrong number is worse than one that does not mention it: the model would be
/// calibrating against a boundary the daemon no longer uses.
pub static SYSTEM: LazyLock<String> = LazyLock::new(|| {
    format!(
        "\
You locate elements in screenshots. You are given one screenshot of a computer display \
and a description of a single element on it.

Report the position in PIXELS of the image you were given, with (0, 0) at its top left \
corner. Do not use any other coordinate system and do not rescale your answer.

- `x` and `y` are the centre of the element. A mark is placed there, so the centre of a \
button matters more than the corner of its bounding box.
- `width` and `height` are the size of the element.
- `confidence` runs from 0 to 1 and is how sure you are that this is the element that was \
described, not how clearly you can see it. Below {HIGH_CONFIDENCE} the daemon outlines a \
region instead of pointing precisely, so an honest low number produces a better result on \
screen than a hopeful high one.
- `found` is false when the described element is not on this screen. Say so rather than \
choosing the nearest thing to it. Nothing is drawn in that case, which is the right \
outcome: a mark on the wrong element is worse than no mark at all."
    )
});

/// What the model is required to answer with.
///
/// Numeric bounds are absent because the schema language does not carry them, so
/// [`into_resolution`] is what enforces the ranges rather than the model being asked
/// nicely.
pub fn schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "found": {
                "type": "boolean",
                "description": "Whether the described element is on this screen at all.",
            },
            "x": { "type": "number", "description": "Centre of the element, in image pixels from the left." },
            "y": { "type": "number", "description": "Centre of the element, in image pixels from the top." },
            "width": { "type": "number", "description": "Width of the element in image pixels." },
            "height": { "type": "number", "description": "Height of the element in image pixels." },
            "confidence": { "type": "number", "description": "0 to 1, how sure you are this is the element described." },
            "reasoning": { "type": "string", "description": "One short sentence on how you identified it." },
        },
        "required": ["found", "x", "y", "width", "height", "confidence", "reasoning"],
        "additionalProperties": false,
    })
}

/// What the model answered, before any of it is believed.
///
/// Every field but `found` and the position is optional, because a local server that
/// cannot constrain its output will drop one and the answer is still usable. A missing
/// size costs the box, a missing confidence is caught by the adapter that built this.
#[derive(Debug, Deserialize)]
pub struct Grounding {
    /// Whether the element is on this screen at all.
    #[serde(default = "yes")]
    pub found: bool,
    /// Centre of the element, in pixels of the image the model was sent.
    pub x: f64,
    /// Centre of the element, in pixels of the image the model was sent.
    pub y: f64,
    /// Width of the element in image pixels.
    #[serde(default)]
    pub width: f64,
    /// Height of the element in image pixels.
    #[serde(default)]
    pub height: f64,
    /// How sure the model was, in `0.0..=1.0`.
    ///
    /// `None` where the model reported nothing. An adapter substitutes its own number and
    /// says in its documentation where that number came from, since a resolution has to
    /// carry one and the daemon decides how to draw with it.
    pub confidence: Option<f64>,
    /// One short sentence on how the element was identified.
    #[serde(default)]
    pub reasoning: String,
}

fn yes() -> bool {
    true
}

/// Parse an answer that is a bare JSON object, tolerating what a model wraps it in.
///
/// A model asked for JSON and given no way to be constrained to it answers with a fenced
/// code block, or a sentence and then the object. Both are the right answer inside the
/// wrong envelope, and refusing them would make the adapter work only against servers
/// that support structured output.
pub fn read_json(text: &str) -> Result<Grounding> {
    let trimmed = text.trim();
    if let Ok(grounding) = serde_json::from_str::<Grounding>(trimmed) {
        return Ok(grounding);
    }

    let start = trimmed.find('{');
    let end = trimmed.rfind('}');
    if let (Some(start), Some(end)) = (start, end)
        && end > start
        && let Ok(grounding) = serde_json::from_str::<Grounding>(&trimmed[start..=end])
    {
        return Ok(grounding);
    }

    Err(Error::Resolver(format!(
        "the answer was not a position: {}",
        snippet(trimmed)
    )))
}

/// Turn an answer in image pixels into one the protocol can carry.
///
/// The single place a coordinate crosses from the image the model saw into the logical
/// points the wire speaks. Every coordinate bug in software of this kind is a conversion
/// done twice or not at all, so there is exactly one of them.
pub fn into_resolution(
    grounding: Grounding,
    confidence: f64,
    image: &Encoded,
) -> Result<Resolution> {
    if !grounding.found {
        return Err(Error::Resolver(format!(
            "the model did not find it on screen: {}",
            grounding.reasoning
        )));
    }
    if !grounding.x.is_finite() || !grounding.y.is_finite() {
        return Err(Error::Resolver("the answer was not a position".into()));
    }

    let point = image.to_logical(grounding.x, grounding.y);
    // A box is a nicety and a point is the answer, so an unusable box is dropped rather
    // than failing the resolve. Low confidence then falls back to the daemon's own region
    // around the point, which is the same shape of answer.
    let rect = image
        .rect_to_logical(
            grounding.x - grounding.width / 2.0,
            grounding.y - grounding.height / 2.0,
            grounding.width,
            grounding.height,
        )
        .into_valid();

    Ok(Resolution {
        point,
        rect,
        // Clamped rather than trusted. The schema cannot express a range, and a confidence
        // above one would sail past the threshold that decides between a precise mark and
        // a cautious one.
        confidence: confidence.clamp(0.0, 1.0),
    })
}

/// The first of a reply worth putting in an error message.
pub fn snippet(body: &str) -> String {
    let short: String = body.chars().take(200).collect();
    if short.len() < body.len() {
        format!("{short}...")
    } else {
        short
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arin_core::Frame;
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
        crate::screenshot::encode(&frame).expect("a well formed frame encodes")
    }

    #[test]
    fn the_prompt_carries_the_threshold_the_daemon_actually_uses() {
        assert!(
            SYSTEM.contains(&HIGH_CONFIDENCE.to_string()),
            "a model calibrated against the wrong boundary is worse than one told nothing"
        );
    }

    #[test]
    fn a_bare_object_parses() {
        let grounding = read_json(r#"{"found":true,"x":10,"y":20,"confidence":0.9}"#).unwrap();
        assert_eq!((grounding.x, grounding.y), (10.0, 20.0));
        assert_eq!(grounding.confidence, Some(0.9));
    }

    /// A server that cannot constrain its output produces the right answer inside the
    /// wrong envelope. Refusing that would restrict the adapter to servers with structured
    /// output, which is most of the reason to run a model locally in the first place.
    #[test]
    fn an_object_inside_a_code_fence_parses() {
        let fenced =
            "Here it is:\n```json\n{\"found\": true, \"x\": 10, \"y\": 20}\n```\nHope that helps.";
        let grounding = read_json(fenced).unwrap();
        assert_eq!((grounding.x, grounding.y), (10.0, 20.0));
    }

    #[test]
    fn a_reply_with_no_object_in_it_is_an_error_that_quotes_the_reply() {
        let error = read_json("I am unable to help with that.").unwrap_err();
        assert!(error.to_string().contains("unable to help"), "got {error}");
    }

    #[test]
    fn a_missing_size_costs_the_box_and_not_the_answer() {
        let grounding = read_json(r#"{"found":true,"x":500,"y":250}"#).unwrap();
        let resolution = into_resolution(grounding, 0.9, &image()).unwrap();
        assert_eq!(resolution.rect, None);
        assert_eq!(resolution.point.x, 500.0);
    }

    #[test]
    fn confidence_outside_the_range_is_clamped_rather_than_trusted() {
        let over = read_json(r#"{"found":true,"x":1,"y":1}"#).unwrap();
        assert_eq!(
            into_resolution(over, 4.0, &image()).unwrap().confidence,
            1.0
        );

        let under = read_json(r#"{"found":true,"x":1,"y":1}"#).unwrap();
        assert_eq!(
            into_resolution(under, -2.0, &image()).unwrap().confidence,
            0.0
        );
    }

    #[test]
    fn an_element_that_is_not_there_is_an_error_rather_than_a_guess() {
        let grounding =
            read_json(r#"{"found":false,"x":0,"y":0,"reasoning":"no Submit button here"}"#)
                .unwrap();
        let error = into_resolution(grounding, 0.2, &image()).unwrap_err();
        assert!(
            error.to_string().contains("no Submit button"),
            "got {error}"
        );
    }
}
