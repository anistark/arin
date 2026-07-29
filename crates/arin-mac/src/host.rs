//! The main thread host, and the bridge onto it.
//!
//! AppKit and Core Animation may only be touched from the main thread, while the daemon
//! runs on tokio. So the main thread owns every panel and layer, and the [`MacRenderer`]
//! handle the daemon holds is a sender: each trait method packages a command and hands
//! it to the main queue.
//!
//! The one method that has to answer immediately is `displays`, which the daemon calls
//! on every positioned message. That reads a cache refreshed on the main thread rather
//! than blocking on a round trip, so a busy main thread slows drawing but never stalls
//! the socket.

use crate::caption;
use crate::display::{Screen, screens};
use crate::panel::Panel;
use arin_core::{Annotation, AnnotationKind, OrbState, Renderer, Result, Rgb};
use arin_protocol::{AnnotationId, DisplayId, DisplayInfo, LogicalPoint, LogicalRect, StrokeStyle};
use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchTime};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSApplicationDidChangeScreenParametersNotification, NSColor};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColor, CGMutablePath};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSString};
use objc2_quartz_core::{CALayer, CAShapeLayer, CATextLayer, CATransaction};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex, OnceLock};

thread_local! {
    /// Main thread only. Every access happens inside a block dispatched to the main
    /// queue, so this is never touched from a tokio worker.
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

/// Everything the renderer owns on the main thread.
struct Host {
    panels: Vec<Panel>,
    /// Layers added for annotations, and the display each sits on.
    ///
    /// `clear` finds them by id. The display is what lets a panel going away take its own
    /// layers with it, which the id alone cannot answer: a highlight or a text box has no
    /// other record of where it was drawn.
    layers: HashMap<AnnotationId, (DisplayId, Retained<CALayer>)>,
    /// Point annotations, and the display each is on.
    ///
    /// Points have no layer of their own: they move the orb, which belongs to the panel.
    /// Tracking them separately is what lets the orb stay up while any point still wants
    /// it, and go away once the last one is cleared.
    points: HashMap<AnnotationId, DisplayId>,
}

impl Host {
    /// Whether any point annotation still wants the orb on this display.
    fn any_point_on(&self, display: DisplayId) -> bool {
        self.points.values().any(|d| *d == display)
    }

    fn panel_for(&mut self, display: DisplayId) -> Option<&mut Panel> {
        self.panels
            .iter_mut()
            .find(|p| p.screen().info.id == display)
    }

    /// Bring the panels back in line with the displays that are actually attached.
    ///
    /// Returns the displays whose panel was replaced or removed, because every layer that
    /// was on one of those is gone and the daemon is still holding annotations that
    /// believe otherwise.
    ///
    /// A panel is kept only when its screen is unchanged in every respect. A display that
    /// moved, resized, or changed scale gets a new one: `Panel` has no way to be resized
    /// in place, and the layers already on it were positioned against the old height, so
    /// keeping them would leave every mark on that display at the wrong end of it.
    fn reconcile(&mut self, mtm: MainThreadMarker) -> Vec<DisplayId> {
        let attached = screens(mtm);

        let mut disturbed: Vec<DisplayId> = Vec::new();
        self.panels.retain(|panel| {
            let screen = panel.screen();
            let kept = attached.contains(&screen);
            if !kept {
                // Dropping the panel closes its window, which is what takes a stale
                // overlay off a display that resized.
                disturbed.push(screen.info.id);
            }
            kept
        });

        for screen in attached {
            if self.panel_for(screen.info.id).is_none() {
                tracing::info!(display = %screen.info.id, "display attached, adding an overlay");
                self.panels.push(Panel::new(screen, mtm));
            }
        }

        // Layers on a panel that went away went away with it, so the bookkeeping has to
        // agree rather than holding references to a closed window's tree.
        for display in &disturbed {
            self.layers.retain(|_, (on, _)| on != display);
            self.points.retain(|_, on| on != display);
        }
        disturbed
    }
}

/// Run a block on the main thread.
///
/// Async on purpose. A synchronous hop would put the socket at the mercy of whatever the
/// main thread is doing, and drawing is fire and forget anyway.
fn on_main<F>(work: F)
where
    F: FnOnce(&mut Host, MainThreadMarker) + Send + 'static,
{
    DispatchQueue::main().exec_async(move || {
        let mtm = MainThreadMarker::new()
            .expect("blocks dispatched to the main queue run on the main thread");

        // Core Animation animates layer changes by default. A mark should appear where
        // it was asked for, and every implicit frame is a screen change scroll detection
        // has to sit through.
        CATransaction::begin();
        CATransaction::setDisableActions(true);

        HOST.with_borrow_mut(|host| {
            if let Some(host) = host.as_mut() {
                work(host, mtm);
            } else {
                tracing::warn!("render command arrived before the host was set up");
            }
        });

        CATransaction::commit();
    });
}

/// How long the orb spends dimming before it disappears.
///
/// Long enough to read as fading rather than vanishing, short enough that a cleared mark
/// feels cleared.
const FADE: std::time::Duration = std::time::Duration::from_millis(320);

/// Run a block on the main thread after a delay.
///
/// Used to settle the orb when a flight lands. Core Animation will have finished the
/// movement by then; this is what puts the daemon's own idea of the orb's state back in
/// step with what the screen is showing.
fn on_main_after<F>(delay: std::time::Duration, work: F)
where
    F: FnOnce(&mut Host, MainThreadMarker) + Send + 'static,
{
    let Ok(when) = DispatchTime::try_from(delay) else {
        on_main(work);
        return;
    };
    let _ = DispatchQueue::main().after(when, move || {
        let mtm = MainThreadMarker::new()
            .expect("blocks dispatched to the main queue run on the main thread");
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        HOST.with_borrow_mut(|host| {
            if let Some(host) = host.as_mut() {
                work(host, mtm);
            }
        });
        CATransaction::commit();
    });
}

/// What to run after the displays change, once the panels have caught up.
///
/// Set by the binary, because the renderer has no handle on the daemon and the daemon is
/// what holds the annotations that need reconciling against the new arrangement.
static DISPLAYS_CHANGED: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// Register what happens when a display is attached, removed, or reconfigured.
///
/// Runs on the main thread with the panels already rebuilt, so it must be quick and must
/// not block. Reconciling the daemon's annotations is fine, capturing a screen is not.
pub fn on_displays_changed(handler: impl Fn() + Send + Sync + 'static) {
    if DISPLAYS_CHANGED.set(Box::new(handler)).is_err() {
        tracing::warn!("a display change handler was already registered");
    }
}

/// Draws the overlay on macOS.
///
/// Cheap to clone and safe to share. All it holds is the display cache and the knowledge
/// of how to reach the main thread.
#[derive(Clone)]
pub struct MacRenderer {
    displays: Arc<Mutex<Vec<DisplayInfo>>>,
}

impl MacRenderer {
    /// Build the host on the main thread and return a handle to it.
    ///
    /// Creates one overlay panel per connected display.
    pub fn install(mtm: MainThreadMarker) -> Self {
        let found = screens(mtm);
        let displays: Vec<DisplayInfo> = found.iter().map(|s| s.info).collect();
        tracing::info!(count = displays.len(), "creating overlay panels");

        let panels = found.into_iter().map(|s| Panel::new(s, mtm)).collect();
        HOST.with_borrow_mut(|slot| {
            *slot = Some(Host {
                panels,
                layers: HashMap::new(),
                points: HashMap::new(),
            });
        });

        let renderer = Self {
            displays: Arc::new(Mutex::new(displays)),
        };
        renderer.watch_for_display_changes();
        renderer
    }

    /// Rebuild the panels whenever the screen arrangement changes.
    ///
    /// AppKit posts this for a display being attached or removed, for a resolution or
    /// scale change, and for the screens being rearranged. Without it the panels and the
    /// display cache are whatever they were at startup: a monitor plugged in afterwards
    /// can never be drawn on, and marks on one that was unplugged sit in the daemon's
    /// state for the life of the session with no overlay left to show them.
    fn watch_for_display_changes(&self) {
        let cache = Arc::clone(&self.displays);
        let centre = NSNotificationCenter::defaultCenter();
        let handler = RcBlock::new(move |_: NonNull<NSNotification>| {
            let Some(mtm) = MainThreadMarker::new() else {
                // AppKit posts this on the main thread. If that ever stops being true,
                // touching panels from anywhere else is worse than doing nothing.
                tracing::error!("a screen change arrived off the main thread");
                return;
            };

            let attached = screens(mtm);
            let refreshed: Vec<DisplayInfo> = attached.iter().map(|s| s.info).collect();
            tracing::info!(count = refreshed.len(), "displays changed");

            CATransaction::begin();
            CATransaction::setDisableActions(true);
            HOST.with_borrow_mut(|host| {
                if let Some(host) = host.as_mut() {
                    host.reconcile(mtm);
                }
            });
            CATransaction::commit();

            // After the panels, so that anything the handler draws has somewhere to go.
            *cache.lock().expect("display cache") = refreshed;
            if let Some(handler) = DISPLAYS_CHANGED.get() {
                handler();
            }
        });

        unsafe {
            centre.addObserverForName_object_queue_usingBlock(
                Some(NSApplicationDidChangeScreenParametersNotification),
                None,
                None,
                &handler,
            )
        };
        // The observer holds the block, and the notification centre holds the observer for
        // the life of the process. Nothing here is ever unregistered because the overlay
        // outlives every display it draws on.
        std::mem::forget(handler);
    }
}

impl Renderer for MacRenderer {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        Ok(self.displays.lock().expect("display cache").clone())
    }

    fn draw(&self, annotation: &Annotation) -> Result<()> {
        let id = annotation.id.clone();
        let screen_id = annotation.display_id();
        let anchor = annotation.anchor.screen_rect;
        let kind = annotation.kind.clone();
        let color = annotation.color;

        on_main(move |host, _mtm| {
            // Drawing the same id twice is a redraw, which is how a mark follows content
            // that scrolled. Replacing the map entry alone would leave the old layer on
            // screen with nothing left holding a reference to remove it: a mark that
            // multiplies every time the page moves, and only the newest one clearable.
            if let Some((_, stale)) = host.layers.remove(&id) {
                stale.removeFromSuperlayer();
            }
            let Some(panel) = host.panel_for(screen_id) else {
                tracing::warn!(%screen_id, "no panel for display, dropping annotation");
                return;
            };
            let mut host_points: Vec<(AnnotationId, DisplayId)> = Vec::new();
            let info = panel.screen().info;
            let panel_size = CGSize::new(info.logical_size[0], info.logical_size[1]);
            let panel_height = panel_size.height;
            let scale = info.scale;

            let layer = match &kind {
                AnnotationKind::Point { at, label, .. } => {
                    let target = to_layer_point(*at, panel_height);
                    let orb = panel.orb_mut();
                    orb.set_visible(true);
                    // Flying rather than teleporting is what makes the orb read as one
                    // thing moving between targets instead of blinking out and back.
                    let flight = orb.travel_to(target);
                    host_points.push((id.clone(), screen_id));
                    if !flight.is_zero() {
                        on_main_after(flight, move |host, _mtm| {
                            if let Some(panel) = host.panel_for(screen_id) {
                                panel.orb_mut().settle();
                            }
                        });
                    }
                    // At the destination rather than riding the arc: text already in
                    // place reads better, and a clear during flight cannot race it.
                    caption::is_drawable(label.as_ref())
                        .map(|text| caption::beside_orb(text, target, color, panel_size, scale))
                }
                AnnotationKind::Highlight { label } => {
                    let outline = highlight_layer(anchor, color, panel_height);
                    match caption::is_drawable(label.as_ref()) {
                        None => Some(outline),
                        // `clear` removes one layer per annotation, so an outline and its
                        // caption are grouped rather than tracked as two entries that
                        // could come apart.
                        Some(text) => {
                            let group = CALayer::new();
                            group.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), panel_size));
                            group.addSublayer(&outline);
                            group.addSublayer(&caption::against_rect(
                                text,
                                to_layer_rect(anchor, panel_height),
                                color,
                                panel_size,
                                scale,
                            ));
                            Some(group)
                        }
                    }
                }
                AnnotationKind::Textbox { text } => {
                    Some(textbox_layer(anchor, text, color, scale, panel_height))
                }
                AnnotationKind::Path { points, style } => {
                    Some(path_layer(points, style.as_ref(), color, panel_height))
                }
            };

            if let Some(layer) = layer {
                panel.root().addSublayer(&layer);
                host.layers.insert(id, (screen_id, layer));
            }
            for (point, display) in host_points {
                host.points.insert(point, display);
            }
        });
        Ok(())
    }

    fn clear(&self, id: &AnnotationId) -> Result<()> {
        let id = id.clone();
        on_main(move |host, _mtm| {
            if let Some((_, layer)) = host.layers.remove(&id) {
                layer.removeFromSuperlayer();
            }
            // A point has no layer of its own, it moves the orb, so the orb only goes
            // away once nothing on that display still wants it.
            if let Some(display) = host.points.remove(&id)
                && !host.any_point_on(display)
                && let Some(panel) = host.panel_for(display)
            {
                panel.orb_mut().set_state(OrbState::Ending);
                on_main_after(FADE, move |host, _mtm| {
                    // Another point may have arrived while it was fading.
                    if !host.any_point_on(display)
                        && let Some(panel) = host.panel_for(display)
                    {
                        panel.orb_mut().set_visible(false);
                    }
                });
            }
        });
        Ok(())
    }

    fn clear_all(&self) -> Result<()> {
        on_main(|host, _mtm| {
            for (_, (_, layer)) in host.layers.drain() {
                layer.removeFromSuperlayer();
            }
            host.points.clear();
            for panel in &mut host.panels {
                panel.orb_mut().set_state(OrbState::Ending);
            }
            on_main_after(FADE, |host, _mtm| {
                let displays: Vec<DisplayId> = host
                    .panels
                    .iter()
                    .map(|p| p.screen().info.id)
                    .filter(|id| !host.any_point_on(*id))
                    .collect();
                for display in displays {
                    if let Some(panel) = host.panel_for(display) {
                        panel.orb_mut().set_visible(false);
                    }
                }
            });
        });
        Ok(())
    }

    fn set_orb_state(&self, state: OrbState) -> Result<()> {
        on_main(move |host, _mtm| {
            for panel in &mut host.panels {
                panel.orb_mut().set_state(state);
            }
        });
        Ok(())
    }
}

/// A stroked rectangle over the target region.
fn highlight_layer(rect: LogicalRect, color: Rgb, panel_height: f64) -> Retained<CALayer> {
    let layer = CALayer::new();
    layer.setFrame(to_layer_rect(rect, panel_height));
    layer.setBorderColor(Some(&annotation_color(color, 1.0)));
    // The same width the daemon sampled under when it chose this colour.
    layer.setBorderWidth(arin_core::contrast::STROKE_WIDTH);
    layer.setCornerRadius(4.0);
    layer
}

/// Displays known to the host, for diagnostics.
pub fn known_screens(mtm: MainThreadMarker) -> Vec<Screen> {
    screens(mtm)
}

/// Convert a protocol point into the panel's layer coordinates.
///
/// The protocol measures from the top left of the display and grows downward, the way a
/// screenshot reads. A panel's layer tree measures from the bottom left and grows upward.
/// Every position handed to Core Animation goes through here or [`to_layer_rect`], so the
/// rule lives in one place rather than in each call site's head.
fn to_layer_point(point: LogicalPoint, panel_height: f64) -> CGPoint {
    CGPoint::new(point.x, panel_height - point.y)
}

/// Convert a protocol rect into the panel's layer coordinates.
///
/// A rect is anchored by its top left, so the height comes off as well as the origin.
/// Forgetting that puts a box one box-height out, which is subtle enough to ship.
fn to_layer_rect(rect: LogicalRect, panel_height: f64) -> CGRect {
    CGRect::new(
        CGPoint::new(rect.x, panel_height - rect.y - rect.height),
        CGSize::new(rect.width, rect.height),
    )
}

/// A block of explanatory text pinned to a region.
///
/// Display only, and deliberately not a control. The overlay is click through, so there
/// is nothing here to focus, select, or type into, and there will not be within 0.x.
fn textbox_layer(
    rect: LogicalRect,
    text: &str,
    color: Rgb,
    scale: f64,
    panel_height: f64,
) -> Retained<CALayer> {
    const PADDING: f64 = 10.0;
    const FONT_SIZE: f64 = 13.0;

    let panel = CALayer::new();
    panel.setFrame(to_layer_rect(rect, panel_height));
    // Dark and mostly opaque, because the text has to be readable over whatever the user
    // happens to have on screen, which is not something an annotation gets to choose.
    panel.setBackgroundColor(Some(&srgb(0.06, 0.07, 0.10, 0.92)));
    panel.setCornerRadius(6.0);
    panel.setBorderWidth(1.5);
    panel.setBorderColor(Some(&annotation_color(color, 0.9)));

    let label = CATextLayer::new();
    label.setFrame(CGRect::new(
        CGPoint::new(PADDING, PADDING),
        CGSize::new(
            (rect.width - PADDING * 2.0).max(1.0),
            (rect.height - PADDING * 2.0).max(1.0),
        ),
    ));
    unsafe { label.setString(Some(&NSString::from_str(text))) };
    label.setFontSize(FONT_SIZE);
    label.setForegroundColor(Some(&srgb(0.93, 0.95, 0.98, 1.0)));
    label.setWrapped(true);
    // Without this the text renders at 1x and is then scaled up, which on a Retina panel
    // looks soft in exactly the way real text does not.
    label.setContentsScale(scale);

    panel.addSublayer(&label);
    panel
}

/// A freehand path.
fn path_layer(
    points: &[LogicalPoint],
    style: Option<&StrokeStyle>,
    color: Rgb,
    panel_height: f64,
) -> Retained<CALayer> {
    let path = CGMutablePath::new();
    let mut rest = points.iter();
    if let Some(first) = rest.next() {
        let start = to_layer_point(*first, panel_height);
        unsafe {
            CGMutablePath::move_to_point(Some(&path), std::ptr::null(), start.x, start.y);
            for point in rest {
                let point = to_layer_point(*point, panel_height);
                CGMutablePath::add_line_to_point(Some(&path), std::ptr::null(), point.x, point.y);
            }
        }
    }

    let layer = CAShapeLayer::new();
    layer.setPath(Some(&path));
    layer.setStrokeColor(Some(&annotation_color(color, 1.0)));
    // A stroked path, not a filled shape. Without this the path closes itself and fills,
    // which turns a gesture into a blob.
    layer.setFillColor(None);
    layer.setLineWidth(
        style
            .and_then(|s| s.width)
            .unwrap_or(arin_core::contrast::STROKE_WIDTH),
    );
    layer.setLineCap(unsafe { objc2_quartz_core::kCALineCapRound });
    layer.setLineJoin(unsafe { objc2_quartz_core::kCALineJoinRound });
    Retained::into_super(layer)
}

/// A colour the daemon resolved, at some alpha.
///
/// Every annotation arrives with a concrete colour already chosen. This crate no longer
/// has an opinion about palettes, contrast, or what a client asked for: that decision is
/// the daemon's, so it stays the same on every platform.
pub(crate) fn annotation_color(color: Rgb, alpha: f64) -> Retained<CGColor> {
    let (r, g, b) = color.as_unit();
    srgb(r, g, b, alpha)
}

pub(crate) fn srgb(r: f64, g: f64, b: f64, a: f64) -> Retained<CGColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a).CGColor()
}

#[cfg(test)]
mod tests {
    use super::{to_layer_point, to_layer_rect};
    use arin_protocol::{LogicalPoint, LogicalRect};

    /// A 14 inch panel, which is what the numbers below are sized against.
    const HEIGHT: f64 = 982.0;

    #[test]
    fn the_protocol_origin_is_the_top_of_the_panel() {
        // Protocol y grows downward, so y of zero is the top, which in layer space is
        // the full height up.
        assert_eq!(
            to_layer_point(LogicalPoint::new(10.0, 0.0), HEIGHT).y,
            HEIGHT
        );
        assert_eq!(
            to_layer_point(LogicalPoint::new(10.0, HEIGHT), HEIGHT).y,
            0.0
        );
    }

    #[test]
    fn x_is_never_touched() {
        assert_eq!(
            to_layer_point(LogicalPoint::new(412.0, 88.0), HEIGHT).x,
            412.0
        );
        assert_eq!(
            to_layer_rect(LogicalRect::new(412.0, 88.0, 30.0, 20.0), HEIGHT)
                .origin
                .x,
            412.0
        );
    }

    #[test]
    fn a_rect_loses_its_height_as_well_as_its_origin() {
        // A box asked for at the very top should sit with its top edge at the top, which
        // means its layer origin is a box height below the ceiling.
        let top = to_layer_rect(LogicalRect::new(0.0, 0.0, 100.0, 90.0), HEIGHT);
        assert_eq!(top.origin.y, HEIGHT - 90.0);

        // And one asked for at the bottom sits on the floor.
        let bottom = to_layer_rect(LogicalRect::new(0.0, HEIGHT - 90.0, 100.0, 90.0), HEIGHT);
        assert_eq!(bottom.origin.y, 0.0);
    }

    #[test]
    fn a_box_near_the_top_lands_above_one_near_the_bottom() {
        // The property that actually failed on screen, stated directly.
        let near_top = to_layer_rect(LogicalRect::new(0.0, 40.0, 500.0, 90.0), HEIGHT);
        let near_bottom = to_layer_rect(LogicalRect::new(0.0, 850.0, 500.0, 90.0), HEIGHT);
        assert!(
            near_top.origin.y > near_bottom.origin.y,
            "a smaller protocol y must render higher up the screen"
        );
    }
}
