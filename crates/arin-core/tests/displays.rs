//! The display matrix.
//!
//! Every coordinate in the protocol is a logical point paired with an explicit display,
//! and the reason that rule is written down in capitals is that mixed-DPI multi-monitor
//! setups are where it gets broken. A Retina panel reports twice the pixels for the same
//! points, an external display next to it does not, and any code that reaches for "the"
//! scale factor rather than *that display's* is wrong in a way that looks fine on the
//! machine it was written on.
//!
//! So this runs the same set of properties against a table of arrangements: one display,
//! two matched, two mismatched, three mismatched, and a portrait panel. Anything that
//! silently assumes the primary display fails here rather than on someone's desk.

use arin_core::{
    Annotation, Capture, Config, Connection, Daemon, Frame, OrbState, Renderer, Result, Rgb,
};
use arin_protocol::*;
use std::sync::{Arc, Mutex};

// the matrix

/// One arrangement of displays to run every property against.
struct Layout {
    name: &'static str,
    displays: Vec<DisplayInfo>,
}

fn display(id: u32, scale: f64, width: f64, height: f64) -> DisplayInfo {
    DisplayInfo {
        id: DisplayId(id),
        scale,
        logical_size: [width, height],
    }
}

/// The arrangements worth being sure about.
///
/// The mismatched ones are the point. A laptop at 2x with an external at 1x is the
/// commonest desk in the world and the one where a single scale factor goes wrong.
fn layouts() -> Vec<Layout> {
    vec![
        Layout {
            name: "one display at 1x",
            displays: vec![display(1, 1.0, 1440.0, 900.0)],
        },
        Layout {
            name: "one Retina display",
            displays: vec![display(1, 2.0, 1728.0, 1117.0)],
        },
        Layout {
            name: "two displays at the same scale",
            displays: vec![
                display(1, 2.0, 1728.0, 1117.0),
                display(2, 2.0, 1440.0, 900.0),
            ],
        },
        Layout {
            name: "Retina laptop beside a 1x external",
            displays: vec![
                display(1, 2.0, 1728.0, 1117.0),
                display(2, 1.0, 2560.0, 1440.0),
            ],
        },
        Layout {
            name: "three displays, all different",
            displays: vec![
                display(1, 2.0, 1728.0, 1117.0),
                display(2, 1.0, 2560.0, 1440.0),
                display(3, 1.5, 1920.0, 1080.0),
            ],
        },
        Layout {
            name: "a portrait display beside a landscape one",
            displays: vec![
                display(1, 2.0, 1728.0, 1117.0),
                display(2, 1.0, 1080.0, 1920.0),
            ],
        },
    ]
}

// fakes

/// A renderer over a fixed set of displays, recording what it was asked to draw.
struct MatrixRenderer {
    displays: Mutex<Vec<DisplayInfo>>,
    drawn: Mutex<Vec<Annotation>>,
    cleared: Mutex<Vec<AnnotationId>>,
}

impl MatrixRenderer {
    fn new(displays: Vec<DisplayInfo>) -> Self {
        Self {
            displays: Mutex::new(displays),
            drawn: Mutex::new(Vec::new()),
            cleared: Mutex::new(Vec::new()),
        }
    }

    /// The last annotation drawn, which is the one a test just asked for.
    fn last(&self) -> Annotation {
        self.drawn
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("something was drawn")
    }
}

impl Renderer for MatrixRenderer {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        Ok(self.displays.lock().unwrap().clone())
    }

    fn draw(&self, annotation: &Annotation) -> Result<()> {
        self.drawn.lock().unwrap().push(annotation.clone());
        Ok(())
    }

    fn clear(&self, id: &AnnotationId) -> Result<()> {
        self.cleared.lock().unwrap().push(id.clone());
        Ok(())
    }

    fn clear_all(&self) -> Result<()> {
        Ok(())
    }

    fn set_orb_state(&self, _state: OrbState) -> Result<()> {
        Ok(())
    }
}

/// A capture that answers for each display separately, in that display's own geometry.
///
/// The colour differs per display so a test can prove which frame was actually read. A
/// backend that returned the primary display's frame for every request would pass a great
/// many tests and put every mark in the wrong colour on a second monitor.
struct MatrixCapture {
    displays: Vec<DisplayInfo>,
    /// What each display is showing, by id.
    background: Vec<(DisplayId, Rgb)>,
}

impl MatrixCapture {
    fn new(displays: Vec<DisplayInfo>, background: Vec<(DisplayId, Rgb)>) -> Self {
        Self {
            displays,
            background,
        }
    }
}

impl Capture for MatrixCapture {
    fn capture(&self, display: DisplayId) -> Result<Frame> {
        self.capture_detailed(display, 512)
    }

    fn capture_detailed(&self, display: DisplayId, min_long_edge: u32) -> Result<Frame> {
        let info = self
            .displays
            .iter()
            .find(|d| d.id == display)
            .ok_or_else(|| arin_core::Error::Capture(format!("no display {display}")))?;

        // Physical pixels at this display's own scale, capped the way a real backend
        // caps them. The point is that the frame's dimensions differ per display and
        // never match the logical size.
        let longest = info.logical_size[0].max(info.logical_size[1]) * info.scale;
        let factor = (f64::from(min_long_edge) / longest).min(1.0);
        let width = (info.logical_size[0] * info.scale * factor)
            .round()
            .max(1.0) as u32;
        let height = (info.logical_size[1] * info.scale * factor)
            .round()
            .max(1.0) as u32;

        let color = self
            .background
            .iter()
            .find(|(id, _)| *id == display)
            .map(|(_, c)| *c)
            .unwrap_or(Rgb::new(0x1E, 0x1E, 0x1E));

        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        for chunk in pixels.chunks_exact_mut(4) {
            chunk[0] = color.b;
            chunk[1] = color.g;
            chunk[2] = color.r;
            chunk[3] = 255;
        }

        Ok(Frame {
            display,
            scale: info.scale * factor,
            logical_size: info.logical_size,
            width,
            height,
            pixels: pixels.into(),
        })
    }
}

// harness

fn daemon_for(
    layout: &Layout,
    background: Vec<(DisplayId, Rgb)>,
) -> (Arc<Daemon>, Arc<MatrixRenderer>) {
    let renderer = Arc::new(MatrixRenderer::new(layout.displays.clone()));
    let capture = Arc::new(MatrixCapture::new(layout.displays.clone(), background));
    let daemon = Daemon::new(
        Config::with_socket_path("/tmp/arin-matrix.sock"),
        renderer.clone(),
        capture,
    );
    (Arc::new(daemon), renderer)
}

async fn started(daemon: Arc<Daemon>) -> Connection {
    let mut conn = Connection::new(daemon);
    conn.handle(Envelope::current(ClientMessage::SessionStart(
        SessionStart {
            client_name: "matrix".into(),
        },
    )))
    .await
    .expect("session_start");
    conn
}

fn ack(reply: DaemonMessage) -> Ack {
    match reply {
        DaemonMessage::Ack(ack) => ack,
        other => panic!("expected an ack, got {other:?}"),
    }
}

// properties

/// Every display is addressable, and each ack reports that display's own geometry.
///
/// The failure this catches is reporting the primary display's scale for a mark on a
/// secondary one. A client divides screenshot pixels by what it is told, so the wrong
/// scale sends every subsequent coordinate to the wrong place, at a ratio it has no way
/// to discover.
#[tokio::test]
async fn every_display_reports_its_own_geometry() {
    for layout in layouts() {
        let (daemon, _) = daemon_for(&layout, Vec::new());
        let mut conn = started(daemon.clone()).await;

        for info in &layout.displays {
            let reply = conn
                .handle(Envelope::current(ClientMessage::Point(Point::at(
                    10.0, 10.0, info.id,
                ))))
                .await
                .unwrap_or_else(|e| panic!("{}: display {} refused: {e}", layout.name, info.id));

            let reported = ack(reply).display.unwrap_or_else(|| {
                panic!(
                    "{}: display {} acked without metadata",
                    layout.name, info.id
                )
            });
            assert_eq!(reported.id, info.id, "{}", layout.name);
            assert_eq!(
                reported.scale, info.scale,
                "{}: display {} was told the wrong scale",
                layout.name, info.id
            );
            assert_eq!(
                reported.logical_size, info.logical_size,
                "{}: display {} was told the wrong size",
                layout.name, info.id
            );
        }
    }
}

/// A named position resolves against the display it was sent to.
///
/// `--at bottom-right` on a 1080x1920 portrait panel is nowhere near where it is on a
/// 2560x1440 landscape one, and resolving it against the primary display's size puts the
/// mark off the screen it was meant for.
#[tokio::test]
async fn named_positions_resolve_against_their_own_display() {
    for layout in layouts() {
        let (daemon, renderer) = daemon_for(&layout, Vec::new());
        let mut conn = started(daemon.clone()).await;

        for info in &layout.displays {
            conn.handle(Envelope::current(ClientMessage::Point(Point::named(
                "bottom-right",
                info.id,
            ))))
            .await
            .unwrap_or_else(|e| panic!("{}: {e}", layout.name));

            let drawn = renderer.last();
            let arin_core::AnnotationKind::Point { at, .. } = drawn.kind else {
                panic!("expected a point");
            };

            assert!(
                at.x > info.logical_size[0] / 2.0 && at.x < info.logical_size[0],
                "{}: bottom-right on display {} landed at x {} on a {} wide display",
                layout.name,
                info.id,
                at.x,
                info.logical_size[0]
            );
            assert!(
                at.y > info.logical_size[1] / 2.0 && at.y < info.logical_size[1],
                "{}: bottom-right on display {} landed at y {} on a {} tall display",
                layout.name,
                info.id,
                at.y,
                info.logical_size[1]
            );
        }
    }
}

/// The centre of a display is its own centre, whatever the display next to it is doing.
#[tokio::test]
async fn the_centre_of_each_display_is_its_own() {
    for layout in layouts() {
        let (daemon, renderer) = daemon_for(&layout, Vec::new());
        let mut conn = started(daemon.clone()).await;

        for info in &layout.displays {
            conn.handle(Envelope::current(ClientMessage::Point(Point::named(
                "center", info.id,
            ))))
            .await
            .unwrap();

            let arin_core::AnnotationKind::Point { at, .. } = renderer.last().kind else {
                panic!("expected a point");
            };
            assert!(
                (at.x - info.logical_size[0] / 2.0).abs() < 0.001
                    && (at.y - info.logical_size[1] / 2.0).abs() < 0.001,
                "{}: centre of display {} resolved to {at:?}, expected {:?}",
                layout.name,
                info.id,
                [info.logical_size[0] / 2.0, info.logical_size[1] / 2.0]
            );
        }
    }
}

/// A display nobody has is refused rather than quietly drawn on the primary.
#[tokio::test]
async fn an_absent_display_is_refused_in_every_layout() {
    for layout in layouts() {
        let (daemon, _) = daemon_for(&layout, Vec::new());
        let mut conn = started(daemon.clone()).await;

        let error = conn
            .handle(Envelope::current(ClientMessage::Point(Point::at(
                10.0,
                10.0,
                DisplayId(99),
            ))))
            .await
            .expect_err("display 99 is in none of these layouts");
        assert_eq!(error.code(), ErrorCode::UnknownDisplay, "{}", layout.name);
        assert_eq!(daemon.annotation_count(), 0, "{}", layout.name);
    }
}

/// The colour picker reads the display the mark is going on.
///
/// One screen showing amber and another showing near black, marked in the same call
/// sequence. A backend that answered with the primary display's frame every time would
/// give both marks the same colour, and the one on the amber screen would be invisible.
#[tokio::test]
async fn the_colour_comes_from_the_display_being_marked() {
    for layout in layouts().into_iter().filter(|l| l.displays.len() > 1) {
        let amber = layout.displays[0].id;
        let dark = layout.displays[1].id;
        let background = vec![
            // The default annotation colour, so the picker has to move away from it.
            (amber, arin_core::contrast::DEFAULT),
            (dark, Rgb::new(0x1E, 0x1E, 0x1E)),
        ];

        let (daemon, renderer) = daemon_for(&layout, background);
        let mut conn = started(daemon.clone()).await;

        conn.handle(Envelope::current(ClientMessage::Point(Point::at(
            10.0, 10.0, amber,
        ))))
        .await
        .unwrap();
        let on_amber = renderer.last().color;

        conn.handle(Envelope::current(ClientMessage::Point(Point::at(
            10.0, 10.0, dark,
        ))))
        .await
        .unwrap();
        let on_dark = renderer.last().color;

        assert_ne!(
            on_amber,
            arin_core::contrast::DEFAULT,
            "{}: a mark on an amber screen kept the amber default",
            layout.name
        );
        assert_eq!(
            on_dark,
            arin_core::contrast::DEFAULT,
            "{}: a mark on a dark screen should keep the default",
            layout.name
        );
        assert_ne!(
            on_amber, on_dark,
            "{}: both displays produced the same colour, so only one frame was read",
            layout.name
        );
    }
}

/// Content moving on one display leaves the other displays alone.
#[tokio::test]
async fn a_scroll_on_one_display_does_not_touch_another() {
    for layout in layouts().into_iter().filter(|l| l.displays.len() > 1) {
        let (daemon, _) = daemon_for(&layout, Vec::new());
        let mut conn = started(daemon.clone()).await;

        for info in &layout.displays {
            conn.handle(Envelope::current(ClientMessage::Point(Point::at(
                10.0, 10.0, info.id,
            ))))
            .await
            .unwrap();
        }
        assert_eq!(daemon.annotation_count(), layout.displays.len());

        let moved = layout.displays[0].id;
        let invalidated = daemon.invalidate_display(moved, InvalidationReason::Scroll);

        assert_eq!(invalidated.len(), 1, "{}", layout.name);
        assert_eq!(
            daemon.annotation_count(),
            layout.displays.len() - 1,
            "{}: a scroll on display {moved} took marks off other displays",
            layout.name
        );
    }
}

/// Only displays with something on them are watched, whatever else is connected.
#[tokio::test]
async fn only_displays_in_use_are_watched() {
    for layout in layouts().into_iter().filter(|l| l.displays.len() > 1) {
        let (daemon, _) = daemon_for(&layout, Vec::new());
        let mut conn = started(daemon.clone()).await;

        let marked = layout.displays[1].id;
        conn.handle(Envelope::current(ClientMessage::Point(Point::at(
            10.0, 10.0, marked,
        ))))
        .await
        .unwrap();

        assert_eq!(
            daemon.displays_in_use(),
            vec![marked],
            "{}: watching a display with nothing on it",
            layout.name
        );
    }
}

/// A region given in logical points keeps those points, whatever the display's scale.
///
/// The anchor is what every later decision is made against, so a scale factor applied on
/// the way in would move the mark and nothing downstream would know.
#[tokio::test]
async fn a_rect_survives_the_trip_at_any_scale() {
    let rect = LogicalRect::new(100.0, 200.0, 340.0, 90.0);
    for layout in layouts() {
        let (daemon, renderer) = daemon_for(&layout, Vec::new());
        let mut conn = started(daemon.clone()).await;

        for info in &layout.displays {
            conn.handle(Envelope::current(ClientMessage::Highlight(
                Highlight::over(rect, info.id),
            )))
            .await
            .unwrap();

            let drawn = renderer.last();
            assert_eq!(
                drawn.anchor.screen_rect, rect,
                "{}: display {} at {}x moved the rect",
                layout.name, info.id, info.scale
            );
            assert_eq!(drawn.anchor.display_id, info.id, "{}", layout.name);
        }
    }
}

// displays that change while marks are on them

impl MatrixRenderer {
    /// Pull a display out, as unplugging a monitor does.
    fn unplug(&self, gone: DisplayId) {
        self.displays.lock().unwrap().retain(|d| d.id != gone);
    }

    /// Change a display's size, as switching resolution does.
    fn resize(&self, id: DisplayId, width: f64, height: f64) {
        for info in self.displays.lock().unwrap().iter_mut() {
            if info.id == id {
                info.logical_size = [width, height];
            }
        }
    }
}

/// A mark on a display that is no longer there cannot be seen, cannot be cleared by the
/// user, and will never be invalidated on its own. It sits in the daemon's state forever,
/// keeping the scroll watcher captureing a display that does not exist.
#[tokio::test]
async fn unplugging_a_display_takes_its_marks_with_it() {
    let layout = Layout {
        name: "two displays, one unplugged",
        displays: vec![
            display(1, 2.0, 1728.0, 1117.0),
            display(2, 1.0, 2560.0, 1440.0),
        ],
    };
    let (daemon, renderer) = daemon_for(&layout, Vec::new());
    let mut conn = started(daemon.clone()).await;

    for info in &layout.displays {
        conn.handle(Envelope::current(ClientMessage::Point(Point::at(
            10.0, 10.0, info.id,
        ))))
        .await
        .unwrap();
    }
    assert_eq!(daemon.annotation_count(), 2);

    renderer.unplug(DisplayId(2));
    let invalidated = daemon.reconcile_displays();

    assert_eq!(
        invalidated.len(),
        1,
        "the mark on the unplugged display should have gone"
    );
    assert_eq!(
        invalidated[0].reason,
        InvalidationReason::DisplayChange,
        "the wire has a reason for exactly this and it should be used"
    );
    assert_eq!(
        daemon.annotation_count(),
        1,
        "the other display is untouched"
    );
    assert_eq!(daemon.displays_in_use(), vec![DisplayId(1)]);
}

/// A display that shrinks can leave a mark outside it. The mark is describing something
/// that is no longer where it was, which is the same failure a scroll causes.
#[tokio::test]
async fn a_display_that_shrinks_drops_marks_left_outside_it() {
    let layout = Layout {
        name: "one display, resized",
        displays: vec![display(1, 1.0, 2560.0, 1440.0)],
    };
    let (daemon, renderer) = daemon_for(&layout, Vec::new());
    let mut conn = started(daemon.clone()).await;

    // One near the far corner, one near the origin.
    conn.handle(Envelope::current(ClientMessage::Highlight(
        Highlight::over(LogicalRect::new(2200.0, 1300.0, 200.0, 100.0), DisplayId(1)),
    )))
    .await
    .unwrap();
    conn.handle(Envelope::current(ClientMessage::Highlight(
        Highlight::over(LogicalRect::new(40.0, 40.0, 200.0, 100.0), DisplayId(1)),
    )))
    .await
    .unwrap();

    renderer.resize(DisplayId(1), 1280.0, 800.0);
    let invalidated = daemon.reconcile_displays();

    assert_eq!(
        invalidated.len(),
        1,
        "only the mark now outside the display should go"
    );
    assert_eq!(invalidated[0].reason, InvalidationReason::DisplayChange);
    assert_eq!(daemon.annotation_count(), 1);
}

/// Reconciling an unchanged arrangement is silent. This runs on a timer, so it has to be
/// free and it has to not announce anything.
#[tokio::test]
async fn an_unchanged_arrangement_reconciles_to_nothing() {
    for layout in layouts() {
        let (daemon, _) = daemon_for(&layout, Vec::new());
        let mut conn = started(daemon.clone()).await;

        for info in &layout.displays {
            conn.handle(Envelope::current(ClientMessage::Point(Point::at(
                10.0, 10.0, info.id,
            ))))
            .await
            .unwrap();
        }

        assert!(
            daemon.reconcile_displays().is_empty(),
            "{}: nothing changed, so nothing should be invalidated",
            layout.name
        );
        assert_eq!(daemon.annotation_count(), layout.displays.len());
    }
}

/// The owner is told, the same as for a scroll or a time to live.
#[tokio::test]
async fn a_display_going_away_is_announced_to_its_owner() {
    let layout = Layout {
        name: "two displays",
        displays: vec![
            display(1, 2.0, 1728.0, 1117.0),
            display(2, 1.0, 2560.0, 1440.0),
        ],
    };
    let (daemon, renderer) = daemon_for(&layout, Vec::new());
    let mut conn = started(daemon.clone()).await;
    let mut listener = daemon.subscribe();

    conn.handle(Envelope::current(ClientMessage::Point(Point::at(
        10.0,
        10.0,
        DisplayId(2),
    ))))
    .await
    .unwrap();

    renderer.unplug(DisplayId(2));
    daemon.reconcile_displays();

    let announced = listener.try_recv().expect("the owner is told");
    assert_eq!(announced.event.reason, InvalidationReason::DisplayChange);
}

/// Rebuilding an overlay throws away every layer on it, and the renderer has no record of
/// what those were. The daemon does, so it is what puts them back.
#[tokio::test]
async fn surviving_marks_are_drawn_again_after_an_arrangement_changes() {
    let layout = Layout {
        name: "two displays, one unplugged",
        displays: vec![
            display(1, 2.0, 1728.0, 1117.0),
            display(2, 1.0, 2560.0, 1440.0),
        ],
    };
    let (daemon, renderer) = daemon_for(&layout, Vec::new());
    let mut conn = started(daemon.clone()).await;

    for info in &layout.displays {
        conn.handle(Envelope::current(ClientMessage::Highlight(
            Highlight::over(LogicalRect::new(10.0, 10.0, 100.0, 50.0), info.id),
        )))
        .await
        .unwrap();
    }
    let drawn_before = renderer.drawn.lock().unwrap().len();
    assert_eq!(drawn_before, 2);

    renderer.unplug(DisplayId(2));
    daemon.reconcile_displays();
    let redrawn = daemon.redraw_all();

    assert_eq!(redrawn, 1, "only the surviving mark should be drawn again");
    let all = renderer.drawn.lock().unwrap();
    assert_eq!(all.len(), drawn_before + 1);
    assert_eq!(
        all.last().unwrap().anchor.display_id,
        DisplayId(1),
        "the mark on the unplugged display must not be drawn onto anything"
    );
}

/// Redrawing an empty overlay is free and reports as much.
#[tokio::test]
async fn redrawing_nothing_draws_nothing() {
    let layout = Layout {
        name: "one display",
        displays: vec![display(1, 2.0, 1728.0, 1117.0)],
    };
    let (daemon, renderer) = daemon_for(&layout, Vec::new());
    let _conn = started(daemon.clone()).await;

    assert_eq!(daemon.redraw_all(), 0);
    assert!(renderer.drawn.lock().unwrap().is_empty());
}
