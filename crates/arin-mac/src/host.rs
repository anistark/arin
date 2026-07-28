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

use crate::display::{Screen, screens};
use crate::panel::Panel;
use arin_core::{Annotation, AnnotationKind, OrbState, Renderer, Result};
use arin_protocol::{AnnotationId, DisplayId, DisplayInfo, LogicalRect};
use dispatch2::{DispatchQueue, DispatchTime};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::NSColor;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_quartz_core::{CALayer, CATransaction};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Default annotation colour, amber.
///
/// The contrast picker that samples the target region and chooses per annotation lands
/// in 0.2. Until then everything uses the default, which is why blue is reserved.
const ANNOTATION: (f64, f64, f64) = (
    0xFF as f64 / 255.0,
    0xB0 as f64 / 255.0,
    0x20 as f64 / 255.0,
);

thread_local! {
    /// Main thread only. Every access happens inside a block dispatched to the main
    /// queue, so this is never touched from a tokio worker.
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

/// Everything the renderer owns on the main thread.
struct Host {
    panels: Vec<Panel>,
    /// Layers added for annotations, so `clear` can find them again.
    layers: HashMap<AnnotationId, Retained<CALayer>>,
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

        // Core Animation animates most layer property changes by default, over about a
        // quarter of a second. That is the wrong default here twice over: a mark should
        // appear where it was asked for rather than drift into place, and every frame of
        // an implicit animation is a screen change that scroll detection has to sit
        // through. Motion in this overlay is deliberate or it does not happen.
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

        Self {
            displays: Arc::new(Mutex::new(displays)),
        }
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

        on_main(move |host, _mtm| {
            let Some(panel) = host.panel_for(screen_id) else {
                tracing::warn!(%screen_id, "no panel for display, dropping annotation");
                return;
            };
            let mut host_points: Vec<(AnnotationId, DisplayId)> = Vec::new();

            let layer = match &kind {
                AnnotationKind::Point { at, .. } => {
                    let orb = panel.orb_mut();
                    orb.set_visible(true);
                    // Flying rather than teleporting is what makes the orb read as one
                    // thing moving between targets instead of blinking out and back.
                    let flight = orb.travel_to(CGPoint::new(at.x, at.y));
                    host_points.push((id.clone(), screen_id));
                    if !flight.is_zero() {
                        // Land it: back to round, a flare, and the calm pointing pulse.
                        on_main_after(flight, move |host, _mtm| {
                            if let Some(panel) = host.panel_for(screen_id) {
                                panel.orb_mut().settle();
                            }
                        });
                    }
                    None
                }
                AnnotationKind::Highlight { .. } => Some(highlight_layer(anchor)),
                // Text boxes and freehand paths are protocol level already and land in
                // 0.2. The daemon accepts them today, so drop rather than fail.
                AnnotationKind::Textbox { .. } | AnnotationKind::Path { .. } => {
                    tracing::debug!(%id, "annotation kind not rendered yet");
                    None
                }
            };

            if let Some(layer) = layer {
                panel.root().addSublayer(&layer);
                host.layers.insert(id, layer);
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
            if let Some(layer) = host.layers.remove(&id) {
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
            for (_, layer) in host.layers.drain() {
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
fn highlight_layer(rect: LogicalRect) -> Retained<CALayer> {
    let layer = CALayer::new();
    layer.setFrame(CGRect::new(
        CGPoint::new(rect.x, rect.y),
        CGSize::new(rect.width, rect.height),
    ));
    let color =
        NSColor::colorWithSRGBRed_green_blue_alpha(ANNOTATION.0, ANNOTATION.1, ANNOTATION.2, 1.0)
            .CGColor();
    layer.setBorderColor(Some(&color));
    layer.setBorderWidth(3.0);
    layer.setCornerRadius(4.0);
    layer
}

/// Displays known to the host, for diagnostics.
pub fn known_screens(mtm: MainThreadMarker) -> Vec<Screen> {
    screens(mtm)
}
