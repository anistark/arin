//! End-to-end state machine tests with no display and no socket.
//!
//! This file is the proof that the hard dependency rule pays for itself: the whole
//! daemon is exercised here through the platform traits, on any target, in milliseconds.

use arin_core::{
    Annotation, Capture, Config, Connection, Daemon, Error, Frame, OrbState, Renderer, Rendering,
    Resolution, Resolver, Result,
};
use arin_protocol::*;
use futures::future::BoxFuture;
use std::sync::{Arc, Mutex};

const DISPLAY: DisplayId = DisplayId(1);

// fakes
#[derive(Default)]
struct FakeRenderer {
    drawn: Mutex<Vec<AnnotationId>>,
    cleared: Mutex<Vec<AnnotationId>>,
    states: Mutex<Vec<OrbState>>,
}

impl Renderer for FakeRenderer {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        Ok(vec![DisplayInfo {
            id: DISPLAY,
            scale: 2.0,
            logical_size: [1728.0, 1117.0],
        }])
    }

    fn draw(&self, annotation: &Annotation) -> Result<()> {
        self.drawn.lock().unwrap().push(annotation.id.clone());
        Ok(())
    }

    fn clear(&self, id: &AnnotationId) -> Result<()> {
        self.cleared.lock().unwrap().push(id.clone());
        Ok(())
    }

    fn clear_all(&self) -> Result<()> {
        Ok(())
    }

    fn set_orb_state(&self, state: OrbState) -> Result<()> {
        self.states.lock().unwrap().push(state);
        Ok(())
    }
}

struct FakeCapture;

impl Capture for FakeCapture {
    fn capture(&self, display: DisplayId) -> Result<Frame> {
        Ok(Frame {
            display,
            scale: 2.0,
            logical_size: [1728.0, 1117.0],
            width: 3456,
            height: 2234,
            pixels: vec![0u8; 64].into(),
        })
    }
}

struct FakeResolver {
    confidence: f64,
}

impl Resolver for FakeResolver {
    fn name(&self) -> &str {
        "fake"
    }

    fn is_remote(&self) -> bool {
        false
    }

    fn resolve<'a>(
        &'a self,
        _query: &'a str,
        _frame: &'a Frame,
    ) -> BoxFuture<'a, Result<Resolution>> {
        Box::pin(async move {
            Ok(Resolution {
                point: LogicalPoint::new(412.0, 88.0),
                rect: None,
                confidence: self.confidence,
            })
        })
    }
}

// harness
fn daemon() -> (Arc<Daemon>, Arc<FakeRenderer>) {
    let renderer = Arc::new(FakeRenderer::default());
    let daemon = Daemon::new(
        Config::with_socket_path("/tmp/arin-test.sock"),
        renderer.clone(),
        Arc::new(FakeCapture),
    );
    (Arc::new(daemon), renderer)
}

fn wrap(message: ClientMessage) -> Envelope<ClientMessage> {
    Envelope::current(message)
}

/// Open a connection that already holds a session.
async fn started(daemon: Arc<Daemon>) -> Connection {
    let mut conn = Connection::new(daemon);
    conn.handle(wrap(ClientMessage::SessionStart(SessionStart {
        client_name: "claude-code".into(),
    })))
    .await
    .expect("session_start");
    conn
}

// tests
#[tokio::test]
async fn session_start_returns_a_session_id() {
    let (daemon, _) = daemon();
    let mut conn = Connection::new(daemon.clone());

    let reply = conn
        .handle(wrap(ClientMessage::SessionStart(SessionStart {
            client_name: "claude-code".into(),
        })))
        .await
        .unwrap();

    let DaemonMessage::Ack(ack) = reply else {
        panic!("expected an ack, got {reply:?}");
    };
    assert!(ack.session_id.is_some());
    assert!(ack.annotation_id.is_none());
    assert_eq!(daemon.session_count(), 1);
}

#[tokio::test]
async fn a_point_is_drawn_and_acked_with_display_metadata() {
    let (daemon, renderer) = daemon();
    let mut conn = started(daemon.clone()).await;

    let reply = conn
        .handle(wrap(ClientMessage::Point(
            Point::at(412.0, 88.0, DISPLAY).with_label("Save"),
        )))
        .await
        .unwrap();

    let DaemonMessage::Ack(ack) = reply else {
        panic!("expected an ack, got {reply:?}");
    };
    assert!(ack.annotation_id.is_some());
    // Clients need the scale to convert screenshot pixels back to logical points.
    assert_eq!(ack.display.unwrap().scale, 2.0);
    // No resolver ran, so there is nothing to report about confidence.
    assert_eq!(ack.confidence, None);

    assert_eq!(renderer.drawn.lock().unwrap().len(), 1);
    assert_eq!(daemon.annotation_count(), 1);
}

#[tokio::test]
async fn messages_before_session_start_are_refused() {
    let (daemon, _) = daemon();
    let mut conn = Connection::new(daemon);

    let err = conn
        .handle(wrap(ClientMessage::Point(Point::at(1.0, 2.0, DISPLAY))))
        .await
        .unwrap_err();

    assert!(matches!(err, Error::NoSession));
}

#[tokio::test]
async fn an_unknown_display_is_rejected() {
    let (daemon, _) = daemon();
    let mut conn = started(daemon).await;

    let err = conn
        .handle(wrap(ClientMessage::Point(Point::at(
            1.0,
            2.0,
            DisplayId(99),
        ))))
        .await
        .unwrap_err();

    assert_eq!(err.code(), ErrorCode::UnknownDisplay);
}

#[tokio::test]
async fn the_query_form_needs_a_resolver() {
    let (daemon, _) = daemon();
    let mut conn = started(daemon).await;

    let err = conn
        .handle(wrap(ClientMessage::Point(Point::query(
            "the Submit button",
            DISPLAY,
        ))))
        .await
        .unwrap_err();

    assert_eq!(err.code(), ErrorCode::NoResolver);
}

#[tokio::test]
async fn a_confident_resolution_points_precisely() {
    let renderer = Arc::new(FakeRenderer::default());
    let daemon = Arc::new(
        Daemon::new(
            Config::with_socket_path("/tmp/arin-test.sock"),
            renderer.clone(),
            Arc::new(FakeCapture),
        )
        .with_resolver(Arc::new(FakeResolver { confidence: 0.94 })),
    );
    let mut conn = started(daemon).await;

    let reply = conn
        .handle(wrap(ClientMessage::Point(Point::query(
            "the Submit button",
            DISPLAY,
        ))))
        .await
        .unwrap();

    let DaemonMessage::Ack(ack) = reply else {
        panic!("expected an ack, got {reply:?}");
    };
    assert_eq!(ack.confidence, Some(0.94));
    assert_eq!(ack.resolved_coords, Some(LogicalPoint::new(412.0, 88.0)));
    assert_eq!(Rendering::for_confidence(0.94), Rendering::Point);

    // The orb should have thought about it before it pointed.
    let states = renderer.states.lock().unwrap();
    assert!(states.contains(&OrbState::Thinking));
    assert!(states.contains(&OrbState::Pointing));
}

#[tokio::test]
async fn an_unsure_resolution_falls_back_to_a_region() {
    let renderer = Arc::new(FakeRenderer::default());
    let daemon = Arc::new(
        Daemon::new(
            Config::with_socket_path("/tmp/arin-test.sock"),
            renderer.clone(),
            Arc::new(FakeCapture),
        )
        .with_resolver(Arc::new(FakeResolver { confidence: 0.42 })),
    );
    let mut conn = started(daemon).await;

    conn.handle(wrap(ClientMessage::Point(Point::query(
        "something",
        DISPLAY,
    ))))
    .await
    .unwrap();

    assert_eq!(Rendering::for_confidence(0.42), Rendering::Region);
}

#[tokio::test]
async fn a_session_cannot_clear_another_sessions_annotation() {
    let (daemon, _) = daemon();

    let mut owner = started(daemon.clone()).await;
    let reply = owner
        .handle(wrap(ClientMessage::Point(Point::at(10.0, 10.0, DISPLAY))))
        .await
        .unwrap();
    let DaemonMessage::Ack(ack) = reply else {
        panic!("expected an ack");
    };
    let victim = ack.annotation_id.unwrap();

    let mut intruder = started(daemon.clone()).await;
    let err = intruder
        .handle(wrap(ClientMessage::Clear(Clear::one(victim.clone()))))
        .await
        .unwrap_err();

    assert_eq!(err.code(), ErrorCode::NotOwner);
    assert_eq!(daemon.annotation_count(), 1, "the annotation must survive");
}

#[tokio::test]
async fn a_missing_annotation_is_indistinguishable_from_someone_elses() {
    let (daemon, _) = daemon();
    let mut conn = started(daemon).await;

    let err = conn
        .handle(wrap(ClientMessage::Clear(Clear::one(AnnotationId::new(
            "a_does_not_exist",
        )))))
        .await
        .unwrap_err();

    assert_eq!(err.code(), ErrorCode::NotOwner);
}

#[tokio::test]
async fn clear_all_only_touches_your_own() {
    let (daemon, _) = daemon();

    let mut a = started(daemon.clone()).await;
    a.handle(wrap(ClientMessage::Point(Point::at(1.0, 1.0, DISPLAY))))
        .await
        .unwrap();

    let mut b = started(daemon.clone()).await;
    b.handle(wrap(ClientMessage::Point(Point::at(2.0, 2.0, DISPLAY))))
        .await
        .unwrap();
    assert_eq!(daemon.annotation_count(), 2);

    b.handle(wrap(ClientMessage::Clear(Clear::all())))
        .await
        .unwrap();

    assert_eq!(daemon.annotation_count(), 1, "only b's annotation goes");
}

#[tokio::test]
async fn session_end_clears_that_sessions_annotations() {
    let (daemon, renderer) = daemon();
    let mut conn = started(daemon.clone()).await;

    conn.handle(wrap(ClientMessage::Point(Point::at(1.0, 1.0, DISPLAY))))
        .await
        .unwrap();
    conn.handle(wrap(ClientMessage::Highlight(Highlight::over(
        LogicalRect::new(0.0, 0.0, 100.0, 50.0),
        DISPLAY,
    ))))
    .await
    .unwrap();
    assert_eq!(daemon.annotation_count(), 2);

    let reply = conn.handle(wrap(ClientMessage::SessionEnd)).await.unwrap();
    assert!(matches!(
        reply,
        DaemonMessage::Invalidated(Invalidated {
            reason: InvalidationReason::SessionEnd,
            ..
        })
    ));
    assert_eq!(daemon.annotation_count(), 0);
    assert_eq!(daemon.session_count(), 0);
    assert_eq!(renderer.cleared.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn dropping_a_connection_ends_the_session() {
    let (daemon, _) = daemon();
    {
        let mut conn = started(daemon.clone()).await;
        conn.handle(wrap(ClientMessage::Point(Point::at(1.0, 1.0, DISPLAY))))
            .await
            .unwrap();
        assert_eq!(daemon.annotation_count(), 1);
    }
    // A client that crashes mid-explanation must not leave marks on the screen.
    assert_eq!(daemon.annotation_count(), 0);
    assert_eq!(daemon.session_count(), 0);
}

#[tokio::test]
async fn a_scroll_invalidates_everything_on_that_display() {
    let (daemon, _) = daemon();
    let mut conn = started(daemon.clone()).await;

    conn.handle(wrap(ClientMessage::Point(Point::at(1.0, 1.0, DISPLAY))))
        .await
        .unwrap();
    conn.handle(wrap(ClientMessage::Point(Point::at(2.0, 2.0, DISPLAY))))
        .await
        .unwrap();

    let invalidated = daemon.invalidate_display(DISPLAY, InvalidationReason::Scroll);

    assert_eq!(invalidated.len(), 2);
    assert!(
        invalidated
            .iter()
            .all(|i| i.reason == InvalidationReason::Scroll)
    );
    assert_eq!(daemon.annotation_count(), 0);
}

#[tokio::test]
async fn an_incompatible_major_version_is_refused() {
    let (daemon, _) = daemon();
    let mut conn = Connection::new(daemon);

    let err = conn
        .handle(Envelope::new(Version::new(1, 0), ClientMessage::SessionEnd))
        .await
        .unwrap_err();

    assert_eq!(err.code(), ErrorCode::VersionUnsupported);
}

#[tokio::test]
async fn a_future_minor_version_is_accepted() {
    let (daemon, _) = daemon();
    let mut conn = Connection::new(daemon);

    let reply = conn
        .handle(Envelope::new(
            Version::new(0, 9),
            ClientMessage::SessionStart(SessionStart {
                client_name: "from-the-future".into(),
            }),
        ))
        .await;

    assert!(reply.is_ok());
}

#[tokio::test]
async fn a_textbox_is_pinned_to_its_anchor() {
    let (daemon, _) = daemon();
    let mut conn = started(daemon.clone()).await;

    let reply = conn
        .handle(wrap(ClientMessage::Textbox(Textbox::new(
            Anchor::new(LogicalRect::new(100.0, 200.0, 340.0, 90.0), DISPLAY),
            "This paragraph is the counterargument.",
        ))))
        .await
        .unwrap();

    assert!(matches!(reply, DaemonMessage::Ack(_)));
    assert_eq!(daemon.annotation_count(), 1);
}

#[tokio::test]
async fn a_straight_path_still_produces_a_drawable_anchor() {
    let (daemon, _) = daemon();
    let mut conn = started(daemon.clone()).await;

    // Every point on one horizontal line: the naive bounding box has zero height.
    conn.handle(wrap(ClientMessage::Draw(Draw::new(
        DISPLAY,
        vec![[100.0, 200.0], [140.0, 200.0], [180.0, 200.0]],
    ))))
    .await
    .unwrap();

    assert_eq!(daemon.annotation_count(), 1);
}
