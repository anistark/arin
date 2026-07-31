//! Both adapters against a real socket.
//!
//! The unit tests cover building a request and reading an answer, which is most of the
//! logic. What they cannot cover is the part where those meet an HTTP client: whether the
//! headers the API requires are actually sent, whether the body serialises the way the
//! model expects to receive it, and whether a reply comes back through `reqwest` and out
//! the other side as a `Resolution`.
//!
//! So this stands up a one-shot HTTP server on loopback, points a resolver at it, and
//! checks both directions. No network, no key, no cost.
//!
//! The local adapter gets the same treatment, and the loopback server is not a compromise
//! there but exactly what it talks to in production. What is being faked is the model, not
//! the transport.

use arin_core::{Capture, Frame, NoopCapture, Resolver};
use arin_protocol::DisplayId;
use arin_resolve::{ClaudeResolver, LocalResolver, local::CoordinateSpace};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A screenshot small enough to keep the request readable in a failure message.
fn frame() -> Frame {
    let (width, height) = (64u32, 40u32);
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    for (i, chunk) in pixels.chunks_exact_mut(4).enumerate() {
        chunk[0] = (i % 256) as u8;
        chunk[3] = 255;
    }
    Frame {
        display: DisplayId(1),
        scale: 2.0,
        logical_size: [1280.0, 800.0],
        width,
        height,
        pixels: Arc::from(pixels),
    }
}

/// Answer exactly one request with `reply`, and hand back what was asked.
async fn one_shot(
    reply: String,
    status: &'static str,
) -> (String, tokio::task::JoinHandle<String>) {
    served("/v1/messages", reply, status).await
}

/// The same, on a path the caller chooses. Two APIs, two paths.
async fn served(
    path: &str,
    reply: String,
    status: &'static str,
) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let endpoint = format!("http://{}{path}", listener.local_addr().expect("addr"));

    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept");

        // Read headers, then however much body the request says it has. Enough HTTP to
        // satisfy one well behaved client, and no more.
        let mut request = Vec::new();
        let mut buffer = [0u8; 8192];
        let headers_end = loop {
            let read = socket.read(&mut buffer).await.expect("read");
            assert!(read > 0, "the client hung up before sending a request");
            request.extend_from_slice(&buffer[..read]);
            if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                break at + 4;
            }
        };

        let head = String::from_utf8_lossy(&request[..headers_end]).to_string();
        let length: usize = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().ok())?
            })
            .unwrap_or(0);

        while request.len() < headers_end + length {
            let read = socket.read(&mut buffer).await.expect("read body");
            assert!(read > 0, "the body was shorter than content-length claimed");
            request.extend_from_slice(&buffer[..read]);
        }

        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
            reply.len()
        );
        socket.write_all(response.as_bytes()).await.expect("write");
        socket.flush().await.expect("flush");

        String::from_utf8_lossy(&request).to_string()
    });

    (endpoint, handle)
}

fn answer() -> String {
    // Inside the 64x40 image the frame above encodes to, since that is what the model
    // would actually be looking at.
    let inner = serde_json::json!({
        "found": true,
        "x": 32.0,
        "y": 20.0,
        "width": 16.0,
        "height": 8.0,
        "confidence": 0.91,
        "reasoning": "the blue button labelled Submit",
    });
    serde_json::json!({
        "type": "message",
        "stop_reason": "end_turn",
        "content": [{ "type": "text", "text": inner.to_string() }],
    })
    .to_string()
}

#[tokio::test]
async fn a_query_goes_out_and_coordinates_come_back() {
    let (endpoint, server) = one_shot(answer(), "200 OK").await;
    let resolver = ClaudeResolver::new("sk-ant-test").with_endpoint(endpoint);

    let resolution = resolver
        .resolve("the Submit button", &frame())
        .await
        .expect("a well formed answer resolves");

    // The image is 64 by 40 over a 1280 by 800 point display, so one image pixel is twenty
    // logical points. The model answered dead centre, so the mark belongs dead centre. A
    // conversion done twice or not at all shows up here as a factor of twenty.
    assert_eq!(resolution.point.x, 640.0);
    assert_eq!(resolution.point.y, 400.0);
    assert_eq!(resolution.confidence, 0.91);

    // Reported as a centre and a size, stored as a rect anchored at its corner.
    let rect = resolution.rect.expect("the model reported a box");
    assert_eq!((rect.x, rect.y), (480.0, 320.0));
    assert_eq!((rect.width, rect.height), (320.0, 160.0));

    let request = server.await.expect("the server ran");
    assert!(
        request.starts_with("POST /v1/messages"),
        "wrong method or path: {}",
        request.lines().next().unwrap_or_default()
    );
    // Every one of these is required, and omitting any is a 4xx that would otherwise only
    // show up against the real API.
    assert!(request.contains("x-api-key: sk-ant-test"), "no key sent");
    assert!(
        request
            .to_lowercase()
            .contains("anthropic-version: 2023-06-01"),
        "no api version sent"
    );
    assert!(
        request
            .to_lowercase()
            .contains("content-type: application/json"),
        "the body is json and has to say so"
    );
    assert!(
        request.contains("\"type\":\"json_schema\""),
        "the answer has to be constrained to the schema"
    );
    assert!(
        request.contains("the Submit button"),
        "the query never reached the model"
    );
    assert!(
        request.contains("\"media_type\":\"image/png\""),
        "the screenshot never reached the model"
    );
}

/// The failure a user is most likely to hit first, and the one where a bad message costs
/// the most time.
#[tokio::test]
async fn a_rejected_key_says_so_in_words() {
    let body = serde_json::json!({
        "type": "error",
        "error": { "type": "authentication_error", "message": "invalid x-api-key" },
    })
    .to_string();
    let (endpoint, server) = one_shot(body, "401 Unauthorized").await;

    let error = ClaudeResolver::new("sk-ant-wrong")
        .with_endpoint(endpoint)
        .resolve("anything", &frame())
        .await
        .expect_err("a 401 is a failure");

    let error = error.to_string();
    assert!(error.contains("key was rejected"), "got {error}");
    assert!(error.contains("invalid x-api-key"), "got {error}");
    let _ = server.await;
}

/// Nothing is drawn when the model cannot find the thing, and the client is told why
/// rather than being handed a mark on whatever was nearest.
#[tokio::test]
async fn an_element_that_is_not_on_screen_produces_no_coordinates() {
    let inner = serde_json::json!({
        "found": false,
        "x": 0.0,
        "y": 0.0,
        "width": 0.0,
        "height": 0.0,
        "confidence": 0.0,
        "reasoning": "there is no Submit button on this screen",
    });
    let body = serde_json::json!({
        "type": "message",
        "stop_reason": "end_turn",
        "content": [{ "type": "text", "text": inner.to_string() }],
    })
    .to_string();
    let (endpoint, server) = one_shot(body, "200 OK").await;

    let error = ClaudeResolver::new("sk-ant-test")
        .with_endpoint(endpoint)
        .resolve("the Submit button", &frame())
        .await
        .expect_err("a missing element is not a resolution");

    assert!(
        error.to_string().contains("no Submit button"),
        "the model's own reason is the useful part, got {error}"
    );
    let _ = server.await;
}

/// A local server answering the way a schema-capable runtime does. Same conversion as the
/// hosted adapter, reached through an entirely different API shape.
#[tokio::test]
async fn a_local_model_grounds_a_query_without_a_key() {
    let inner = serde_json::json!({
        "found": true,
        "x": 32.0,
        "y": 20.0,
        "width": 16.0,
        "height": 8.0,
        "confidence": 0.88,
        "reasoning": "the button labelled Submit",
    });
    let body = serde_json::json!({
        "choices": [{
            "finish_reason": "stop",
            "message": { "role": "assistant", "content": inner.to_string() },
        }],
    })
    .to_string();
    let (endpoint, server) = served("/v1/chat/completions", body, "200 OK").await;

    // Deliberately not probing first. `check_reachable` opens and drops a connection, which
    // a server built to answer exactly one request would spend its only accept on.
    let resolver = LocalResolver::new(&endpoint).expect("loopback is accepted");

    let resolution = resolver
        .resolve("the Submit button", &frame())
        .await
        .expect("a well formed answer resolves");

    // The same 64 by 40 image over the same 1280 by 800 point display as above, so one
    // image pixel is twenty logical points and a conversion done twice shows up as a
    // factor of twenty. The two adapters share that conversion and this is what pins it.
    assert_eq!(resolution.point.x, 640.0);
    assert_eq!(resolution.point.y, 400.0);
    assert_eq!(resolution.confidence, 0.88);

    let request = server.await.expect("the server ran");
    assert!(
        request.starts_with("POST /v1/chat/completions"),
        "wrong method or path: {}",
        request.lines().next().unwrap_or_default()
    );
    assert!(
        !request.to_lowercase().contains("x-api-key"),
        "a local resolver has no key and must not invent a header for one"
    );
    assert!(
        request.contains("data:image/png;base64,"),
        "runtimes here want the screenshot as a data url"
    );
    assert!(
        request.contains("the Submit button"),
        "the query never arrived"
    );
    assert!(
        request.contains("\"json_schema\""),
        "the answer is constrained to the schema by default"
    );
}

/// The format the models this adapter targets actually emit. No confidence anywhere in it,
/// so the daemon has to be handed one that draws a region.
#[tokio::test]
async fn a_ui_tars_action_resolves_and_lands_below_the_threshold() {
    let body = serde_json::json!({
        "choices": [{
            "finish_reason": "stop",
            "message": {
                "role": "assistant",
                "content": "Thought: The Submit button is in the middle.\nAction: click(point='<point>500 500</point>')",
            },
        }],
    })
    .to_string();
    let (endpoint, server) = served("/v1/chat/completions", body, "200 OK").await;

    let resolution = LocalResolver::new(&endpoint)
        .expect("loopback is accepted")
        .with_coordinate_space(CoordinateSpace::Normalized)
        .resolve("the Submit button", &frame())
        .await
        .expect("an action resolves");

    // Thousandths of a 64 by 40 image, then image pixels into logical points. Both
    // conversions or neither.
    assert_eq!(resolution.point.x, 640.0);
    assert_eq!(resolution.point.y, 400.0);
    assert_eq!(
        arin_core::Rendering::for_confidence(resolution.confidence),
        arin_core::Rendering::Region,
        "an answer nobody rated must not be drawn as a precise mark"
    );
    let _ = server.await;
}

/// A resolver never captures for itself, so one handed an empty frame fails before it
/// opens a socket. This is what keeps a headless daemon from posting a blank screenshot.
#[tokio::test]
async fn an_empty_frame_never_leaves_the_machine() {
    let blank = NoopCapture.capture(DisplayId(1)).expect("noop captures");
    let resolver = ClaudeResolver::new("sk-ant-test").with_endpoint("http://127.0.0.1:1/unused");

    let error = resolver
        .resolve("anything", &blank)
        .await
        .expect_err("there is nothing to ground against");
    assert!(
        error.to_string().contains("nothing was captured"),
        "got {error}"
    );
}
