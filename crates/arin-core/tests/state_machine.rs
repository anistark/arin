//! End-to-end state machine tests with no display and no socket.
//!
//! This file is the proof that the hard dependency rule pays for itself: the whole
//! daemon is exercised here through the platform traits, on any target, in milliseconds.

use arin_core::{
    Annotation, Capture, Config, Connection, Daemon, Error, Frame, OrbState, Renderer, Rendering,
    Resolution, Resolver, Result, Rgb,
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
    /// The colour each annotation arrived with, in draw order.
    colors: Mutex<Vec<Rgb>>,
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
        self.colors.lock().unwrap().push(annotation.color);
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

    // Acked, not answered with an `invalidated`. Every request gets an ack or an error,
    // which is what leaves `invalidated` free to mean "unsolicited" and nothing else.
    let reply = conn.handle(wrap(ClientMessage::SessionEnd)).await.unwrap();
    assert!(matches!(reply, DaemonMessage::Ack(_)), "got {reply:?}");
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

// time to live

/// A daemon whose config differs from the harness default.
fn daemon_with(config: Config) -> (Arc<Daemon>, Arc<FakeRenderer>) {
    let renderer = Arc::new(FakeRenderer::default());
    let daemon = Daemon::new(config, renderer.clone(), Arc::new(FakeCapture));
    (Arc::new(daemon), renderer)
}

fn expiring_config(default_ttl: Option<std::time::Duration>) -> Config {
    Config {
        default_ttl,
        ..Config::with_socket_path("/tmp/arin-test.sock")
    }
}

/// Long enough for a one millisecond ttl to be comfortably past.
async fn let_it_expire() {
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
}

#[tokio::test]
async fn a_ttl_expires_the_annotation() {
    let (daemon, renderer) = daemon();
    let mut conn = started(daemon.clone()).await;

    conn.handle(wrap(ClientMessage::Point(
        Point::at(1.0, 1.0, DISPLAY).with_ttl_ms(Some(1)),
    )))
    .await
    .unwrap();
    assert_eq!(daemon.annotation_count(), 1);

    let_it_expire().await;
    let expired = daemon.expire_annotations();

    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].reason, InvalidationReason::Ttl);
    assert_eq!(daemon.annotation_count(), 0);
    assert_eq!(renderer.cleared.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn an_annotation_with_no_ttl_is_never_swept() {
    let (daemon, _) = daemon();
    let mut conn = started(daemon.clone()).await;

    conn.handle(wrap(ClientMessage::Point(Point::at(1.0, 1.0, DISPLAY))))
        .await
        .unwrap();

    let_it_expire().await;
    assert!(daemon.expire_annotations().is_empty());
    assert_eq!(daemon.annotation_count(), 1);
}

#[tokio::test]
async fn a_clients_ttl_wins_over_the_daemon_default() {
    // A default long enough that the sweep below can only be the client's own ttl.
    let (daemon, _) = daemon_with(expiring_config(Some(std::time::Duration::from_secs(3600))));
    let mut conn = started(daemon.clone()).await;

    conn.handle(wrap(ClientMessage::Point(
        Point::at(1.0, 1.0, DISPLAY).with_ttl_ms(Some(1)),
    )))
    .await
    .unwrap();

    let_it_expire().await;
    assert_eq!(daemon.expire_annotations().len(), 1);
}

#[tokio::test]
async fn the_default_ttl_applies_when_a_client_asks_for_nothing() {
    let (daemon, _) = daemon_with(expiring_config(Some(std::time::Duration::from_millis(1))));
    let mut conn = started(daemon.clone()).await;

    // Every drawing message, since each one has to reach for the default separately.
    conn.handle(wrap(ClientMessage::Point(Point::at(1.0, 1.0, DISPLAY))))
        .await
        .unwrap();
    conn.handle(wrap(ClientMessage::Highlight(Highlight::over(
        LogicalRect::new(0.0, 0.0, 10.0, 10.0),
        DISPLAY,
    ))))
    .await
    .unwrap();
    conn.handle(wrap(ClientMessage::Textbox(Textbox::new(
        Anchor::new(LogicalRect::new(0.0, 0.0, 10.0, 10.0), DISPLAY),
        "text",
    ))))
    .await
    .unwrap();
    conn.handle(wrap(ClientMessage::Draw(Draw::new(
        DISPLAY,
        vec![[0.0, 0.0], [10.0, 10.0]],
    ))))
    .await
    .unwrap();

    let_it_expire().await;
    assert_eq!(daemon.expire_annotations().len(), 4);
    assert_eq!(daemon.annotation_count(), 0);
}

/// The bug this guards: an expiry changes the screen, and scroll detection compares
/// captures. Without a bumped generation the vanishing mark reads as the page moving and
/// takes every other annotation on that display with it.
#[tokio::test]
async fn expiring_counts_as_the_daemon_changing_the_screen() {
    let (daemon, _) = daemon();
    let mut conn = started(daemon.clone()).await;

    conn.handle(wrap(ClientMessage::Point(
        Point::at(1.0, 1.0, DISPLAY).with_ttl_ms(Some(1)),
    )))
    .await
    .unwrap();

    let before = daemon.render_generation();
    let_it_expire().await;
    daemon.expire_annotations();

    assert!(
        daemon.render_generation() > before,
        "an expiry has to bump the render generation"
    );
}

/// A sweep that expires nothing must not claim the screen changed, or scroll detection
/// re-baselines on every tick and stops noticing real scrolling.
#[tokio::test]
async fn a_sweep_that_expires_nothing_changes_nothing() {
    let (daemon, _) = daemon();
    let mut conn = started(daemon.clone()).await;

    conn.handle(wrap(ClientMessage::Point(Point::at(1.0, 1.0, DISPLAY))))
        .await
        .unwrap();

    let before = daemon.render_generation();
    assert!(daemon.expire_annotations().is_empty());
    assert_eq!(daemon.render_generation(), before);
}

#[tokio::test]
async fn a_zero_ttl_is_refused_rather_than_drawn_and_removed() {
    let (daemon, _) = daemon();
    let mut conn = started(daemon.clone()).await;

    let error = conn
        .handle(wrap(ClientMessage::Point(
            Point::at(1.0, 1.0, DISPLAY).with_ttl_ms(Some(0)),
        )))
        .await
        .expect_err("a zero ttl should be refused");

    assert_eq!(error.to_wire().code, ErrorCode::BadSchema);
    assert_eq!(daemon.annotation_count(), 0);
}

// annotation colour

/// A capture that returns a real frame of one flat colour, so the contrast picker has
/// something to look at. The harness `FakeCapture` deliberately returns a truncated
/// buffer, which is its own test.
struct FlatCapture(Rgb);

impl Capture for FlatCapture {
    fn capture(&self, display: DisplayId) -> Result<Frame> {
        let (w, h) = (64usize, 64usize);
        let mut pixels = vec![0u8; w * h * 4];
        for px in pixels.chunks_exact_mut(4) {
            px[0] = self.0.b;
            px[1] = self.0.g;
            px[2] = self.0.r;
            px[3] = 255;
        }
        Ok(Frame {
            display,
            scale: 1.0,
            logical_size: [1728.0, 1117.0],
            width: w as u32,
            height: h as u32,
            pixels: pixels.into(),
        })
    }
}

fn daemon_over(background: Rgb, config: Config) -> (Arc<Daemon>, Arc<FakeRenderer>) {
    let renderer = Arc::new(FakeRenderer::default());
    let daemon = Daemon::new(config, renderer.clone(), Arc::new(FlatCapture(background)));
    (Arc::new(daemon), renderer)
}

fn config() -> Config {
    Config::with_socket_path("/tmp/arin-test.sock")
}

#[tokio::test]
async fn a_mark_on_a_dark_screen_keeps_the_default_colour() {
    let (daemon, renderer) = daemon_over(Rgb::new(0x1E, 0x1E, 0x1E), config());
    let mut conn = started(daemon).await;

    conn.handle(wrap(ClientMessage::Point(Point::at(1.0, 1.0, DISPLAY))))
        .await
        .unwrap();

    assert_eq!(
        renderer.colors.lock().unwrap()[0],
        arin_core::contrast::DEFAULT
    );
}

#[tokio::test]
async fn a_mark_over_its_own_colour_moves_away_from_it() {
    // The case a fixed colour cannot survive: the screen is already the mark's colour.
    let (daemon, renderer) = daemon_over(arin_core::contrast::DEFAULT, config());
    let mut conn = started(daemon).await;

    conn.handle(wrap(ClientMessage::Highlight(Highlight::over(
        LogicalRect::new(0.0, 0.0, 100.0, 100.0),
        DISPLAY,
    ))))
    .await
    .unwrap();

    let chosen = renderer.colors.lock().unwrap()[0];
    assert_ne!(chosen, arin_core::contrast::DEFAULT);
    assert!(!chosen.is_blue_family(), "{chosen:?} belongs to the orb");
}

#[tokio::test]
async fn a_colour_the_client_named_is_not_second_guessed() {
    // Over a background the picker would certainly move away from.
    let (daemon, renderer) = daemon_over(arin_core::contrast::DEFAULT, config());
    let mut conn = started(daemon).await;

    let mut draw = Draw::new(DISPLAY, vec![[0.0, 0.0], [10.0, 10.0]]);
    draw.style = Some(StrokeStyle {
        width: None,
        color: Some("#123456".into()),
    });
    conn.handle(wrap(ClientMessage::Draw(draw))).await.unwrap();

    assert_eq!(
        renderer.colors.lock().unwrap()[0],
        Rgb::new(0x12, 0x34, 0x56)
    );
}

#[tokio::test]
async fn turning_the_picker_off_always_draws_the_default() {
    let config = Config {
        adaptive_color: false,
        ..config()
    };
    // A background the picker would otherwise flee.
    let (daemon, renderer) = daemon_over(arin_core::contrast::DEFAULT, config);
    let mut conn = started(daemon).await;

    conn.handle(wrap(ClientMessage::Point(Point::at(1.0, 1.0, DISPLAY))))
        .await
        .unwrap();

    assert_eq!(
        renderer.colors.lock().unwrap()[0],
        arin_core::contrast::DEFAULT
    );
}

#[tokio::test]
async fn an_unreadable_colour_falls_back_to_a_chosen_one_not_a_failure() {
    let (daemon, renderer) = daemon_over(Rgb::new(0x1E, 0x1E, 0x1E), config());
    let mut conn = started(daemon).await;

    let mut draw = Draw::new(DISPLAY, vec![[0.0, 0.0], [10.0, 10.0]]);
    draw.style = Some(StrokeStyle {
        width: None,
        color: Some("not a colour".into()),
    });
    let reply = conn.handle(wrap(ClientMessage::Draw(draw))).await.unwrap();

    assert!(matches!(reply, DaemonMessage::Ack(_)));
    assert_eq!(
        renderer.colors.lock().unwrap()[0],
        arin_core::contrast::DEFAULT
    );
}

// named positions

/// The display the fake renderer reports, so a named position has something to land on.
const FAKE_DISPLAY: [f64; 2] = [1728.0, 1117.0];

#[tokio::test]
async fn a_named_position_lands_where_it_says() {
    let (daemon, renderer) = daemon();
    let mut conn = started(daemon.clone()).await;

    conn.handle(wrap(ClientMessage::Point(Point::named("center", DISPLAY))))
        .await
        .unwrap();

    assert_eq!(renderer.drawn.lock().unwrap().len(), 1);
    assert_eq!(daemon.annotation_count(), 1);
}

/// Every name has to resolve to somewhere on the display it was sent to. A name that
/// resolved off screen would ack happily and draw nothing at all.
#[tokio::test]
async fn every_name_resolves_onto_the_display() {
    for name in Position::NAMES {
        let position = Position::parse(name).expect("a documented name must parse");
        let at = position.resolve(FAKE_DISPLAY);
        assert!(
            (0.0..=FAKE_DISPLAY[0]).contains(&at.x) && (0.0..=FAKE_DISPLAY[1]).contains(&at.y),
            "{name} resolved to {at:?}, off the display"
        );
    }
}

#[tokio::test]
async fn a_position_nobody_knows_is_refused_rather_than_guessed() {
    let (daemon, _) = daemon();
    let mut conn = started(daemon.clone()).await;

    let err = conn
        .handle(wrap(ClientMessage::Point(Point::named(
            "somewhere over there",
            DISPLAY,
        ))))
        .await
        .unwrap_err();

    assert_eq!(err.code(), ErrorCode::BadSchema);
    assert_eq!(daemon.annotation_count(), 0);
}

/// The daemon is the one that knows the geometry, so the same name on two displays of
/// different sizes has to land in different places.
#[tokio::test]
async fn a_name_resolves_against_the_display_it_was_sent_to() {
    let small = Position::parse("bottom-right")
        .unwrap()
        .resolve([800.0, 600.0]);
    let large = Position::parse("bottom-right")
        .unwrap()
        .resolve([3840.0, 2160.0]);
    assert!(large.x > small.x && large.y > small.y);
}

// pushed invalidations

/// Everything that goes away without the client asking has to reach the client. Before
/// this the daemon computed each one and dropped it on the floor, so an agent's mark
/// could expire or scroll away and it would carry on describing something that was no
/// longer on screen.
#[tokio::test]
async fn a_ttl_expiry_is_announced_to_its_owner() {
    let (daemon, _) = daemon();
    let mut listener = daemon.subscribe();
    let mut conn = started(daemon.clone()).await;

    conn.handle(wrap(ClientMessage::Point(
        Point::at(1.0, 1.0, DISPLAY).with_ttl_ms(Some(1)),
    )))
    .await
    .unwrap();
    let_it_expire().await;
    daemon.expire_annotations();

    let announced = listener.try_recv().expect("an expiry should be announced");
    assert_eq!(announced.event.reason, InvalidationReason::Ttl);
    assert_eq!(Some(&announced.session), conn.session());
}

#[tokio::test]
async fn a_scroll_is_announced_to_its_owner() {
    let (daemon, _) = daemon();
    let mut listener = daemon.subscribe();
    let mut conn = started(daemon.clone()).await;

    conn.handle(wrap(ClientMessage::Point(Point::at(1.0, 1.0, DISPLAY))))
        .await
        .unwrap();
    daemon.invalidate_display(DISPLAY, InvalidationReason::Scroll);

    let announced = listener.try_recv().expect("a scroll should be announced");
    assert_eq!(announced.event.reason, InvalidationReason::Scroll);
    assert_eq!(Some(&announced.session), conn.session());
}

#[tokio::test]
async fn a_user_clear_is_announced_to_its_owner() {
    let (daemon, _) = daemon();
    let mut listener = daemon.subscribe();
    let mut conn = started(daemon.clone()).await;

    conn.handle(wrap(ClientMessage::Point(Point::at(1.0, 1.0, DISPLAY))))
        .await
        .unwrap();
    daemon.clear_everything();

    let announced = listener.try_recv().expect("a clear should be announced");
    assert_eq!(announced.event.reason, InvalidationReason::Cleared);
    assert_eq!(Some(&announced.session), conn.session());
}

/// The privacy rule, carried into the push path. A session must not learn that another
/// session's annotation went away, for the same reason `clear` answers `not_owner` to
/// both a missing annotation and someone else's. The announcement carries the owner so
/// connections can filter, and the wire message never does.
#[tokio::test]
async fn an_announcement_names_the_owner_so_it_can_be_filtered() {
    let (daemon, _) = daemon();
    let mut listener = daemon.subscribe();

    let mut mine = started(daemon.clone()).await;
    let mut theirs = started(daemon.clone()).await;
    assert_ne!(mine.session(), theirs.session());

    mine.handle(wrap(ClientMessage::Point(Point::at(1.0, 1.0, DISPLAY))))
        .await
        .unwrap();
    theirs
        .handle(wrap(ClientMessage::Point(Point::at(2.0, 2.0, DISPLAY))))
        .await
        .unwrap();

    daemon.clear_everything();

    let first = listener.try_recv().expect("two marks, two announcements");
    let second = listener.try_recv().expect("two marks, two announcements");
    let owners = [&first.session, &second.session];

    assert!(owners.contains(&mine.session().unwrap()));
    assert!(owners.contains(&theirs.session().unwrap()));
    // And the wire message itself says nothing about who owns it.
    assert!(first.event.annotation_id.is_some());
}

/// Nothing to announce when the client asked for it. A `clear` the session sent is a
/// reply, not news, and telling it twice would have an agent believe the daemon undid
/// something behind its back.
#[tokio::test]
async fn clearing_your_own_mark_announces_nothing() {
    let (daemon, _) = daemon();
    let mut listener = daemon.subscribe();
    let mut conn = started(daemon.clone()).await;

    conn.handle(wrap(ClientMessage::Point(Point::at(1.0, 1.0, DISPLAY))))
        .await
        .unwrap();
    conn.handle(wrap(ClientMessage::Clear(Clear::all())))
        .await
        .unwrap();

    assert!(listener.try_recv().is_err(), "a self clear is not news");
}
