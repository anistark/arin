//! Drawing marks on rather than placing them whole.
//!
//! The timeline is the daemon's, computed once in `arin_core::reveal`. This module only
//! plays it: each stroke's ink is revealed by animating how much of its path is stroked,
//! and the orb follows the tip. Both are paced by the samples core produced, at equal
//! slices of time, with no timing function of their own.
//!
//! A timeline is anchored to an `Instant`, not to the moment a layer is built. A mark that
//! is redrawn because the page under it scrolled therefore carries on from wherever its
//! pen had got to, and one whose ink has already settled is simply drawn whole.

use arin_core::{Reveal, StrokeReveal};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSWorkspace;
use objc2_core_foundation::CGPoint;
use objc2_foundation::{NSArray, NSNumber, NSString, NSValue};
use objc2_quartz_core::{
    CABasicAnimation, CACurrentMediaTime, CAKeyframeAnimation, CALayer, CAMediaTiming,
    CAShapeLayer, kCAFillModeBackwards,
};
use std::time::{Duration, Instant};

/// Animation keys, so a redraw replaces its own animation rather than stacking another.
mod key {
    pub const REVEAL: &str = "arin.reveal";
    pub const APPEAR: &str = "arin.appear";
}

/// How long something that arrives with the finished ink takes to fade in.
const APPEAR: Duration = Duration::from_millis(160);

/// Whether the person has asked macOS for less motion.
///
/// Read at every draw rather than once, so turning the setting on takes effect for the
/// next mark without a restart. A mark drawn under it appears whole, and the orb stays
/// where it is.
pub(crate) fn reduce_motion() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

/// Whether a timeline still has ink to put down.
pub(crate) fn is_drawing(reveal: &Reveal) -> bool {
    reveal.settles_at() > Instant::now()
}

/// Reveal one stroke's ink along its path, on the stroke's own slice of the timeline.
///
/// Before its start the stroke shows nothing, and after its end the layer's own value,
/// the whole path, takes over again. A stroke whose time has passed is left alone.
pub(crate) fn draw_on(layer: &CAShapeLayer, stroke: &StrokeReveal, begins: Instant) {
    let start = begins + stroke.start;
    if start + stroke.duration <= Instant::now() || stroke.samples.len() < 2 {
        return;
    }
    let reaches: Vec<Retained<NSNumber>> = stroke
        .samples
        .iter()
        .map(|s| NSNumber::new_f64(s.reach))
        .collect();
    let values: Vec<&AnyObject> = reaches.iter().map(|n| n.as_ref()).collect();

    let reveal = CAKeyframeAnimation::animationWithKeyPath(Some(&NSString::from_str("strokeEnd")));
    unsafe { reveal.setValues(Some(&NSArray::from_slice(&values))) };
    reveal.setDuration(stroke.duration.as_secs_f64());
    reveal.setBeginTime(media_time(start));
    reveal.setFillMode(unsafe { kCAFillModeBackwards });
    layer.addAnimation_forKey(&reveal, Some(&NSString::from_str(key::REVEAL)));
}

/// Hide a layer until a moment, then fade it in.
///
/// For what belongs with a mark but is not drawn by the pen, such as a typeset caption,
/// which would otherwise be sitting there before the loop it labels.
pub(crate) fn appear_at(layer: &CALayer, at: Instant) {
    if at <= Instant::now() {
        return;
    }
    let fade = CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str("opacity")));
    unsafe {
        fade.setFromValue(Some(&*NSNumber::new_f64(0.0)));
        fade.setToValue(Some(&*NSNumber::new_f64(1.0)));
    }
    fade.setDuration(APPEAR.as_secs_f64());
    fade.setBeginTime(media_time(at));
    fade.setFillMode(unsafe { kCAFillModeBackwards });
    layer.addAnimation_forKey(&fade, Some(&NSString::from_str(key::APPEAR)));
}

/// The path the orb follows as the pen, in a panel's layer coordinates.
pub(crate) struct Track {
    /// Positions of the tip.
    pub points: Vec<CGPoint>,
    /// When the tip is at each position, as a fraction of [`Self::duration`].
    pub times: Vec<f64>,
    /// From the first touch to the last lift.
    pub duration: Duration,
}

impl Track {
    /// The tip's whole journey over a timeline, pen lifts included.
    ///
    /// Each stroke contributes its samples at their own moments. Between one stroke's last
    /// sample and the next one's first, the tip crosses in a straight line over the lift,
    /// which is what a hand does with a pen in the air.
    pub(crate) fn of(
        reveal: &Reveal,
        to_layer: impl Fn(arin_protocol::LogicalPoint) -> CGPoint,
    ) -> Self {
        // From the first touch, which is not always the start of the reveal: a point's
        // caption is written after its flight lands.
        let lead = reveal.strokes.first().map_or(Duration::ZERO, |s| s.start);
        let duration = reveal.duration().saturating_sub(lead);
        let whole = duration.as_secs_f64().max(f64::EPSILON);
        let mut points = Vec::new();
        let mut times = Vec::new();
        for stroke in &reveal.strokes {
            let slices = stroke.samples.len().saturating_sub(1).max(1) as f64;
            for (i, sample) in stroke.samples.iter().enumerate() {
                let at = (stroke.start - lead).as_secs_f64()
                    + stroke.duration.as_secs_f64() * i as f64 / slices;
                let fraction = (at / whole).clamp(0.0, 1.0);
                // Key times must not go backwards, and two samples a rounding error apart
                // would otherwise do exactly that at a stroke boundary.
                if times.last().is_some_and(|&last: &f64| fraction < last) {
                    continue;
                }
                points.push(to_layer(sample.at));
                times.push(fraction);
            }
        }
        Self {
            points,
            times,
            duration,
        }
    }

    /// The tip's final position.
    pub(crate) fn end(&self) -> Option<CGPoint> {
        self.points.last().copied()
    }

    /// A keyframe animation of `position` along the track, already `elapsed` into it.
    pub(crate) fn animation(&self, elapsed: Duration) -> Retained<CAKeyframeAnimation> {
        let values: Vec<Retained<NSValue>> = self
            .points
            .iter()
            .map(|&p| unsafe { NSValue::valueWithPoint(p) })
            .collect();
        let refs: Vec<&AnyObject> = values
            .iter()
            .map(|v| <NSValue as AsRef<AnyObject>>::as_ref(v))
            .collect();
        let times: Vec<Retained<NSNumber>> =
            self.times.iter().map(|&t| NSNumber::new_f64(t)).collect();

        let track =
            CAKeyframeAnimation::animationWithKeyPath(Some(&NSString::from_str("position")));
        unsafe { track.setValues(Some(&NSArray::from_slice(&refs))) };
        track.setKeyTimes(Some(&NSArray::from_retained_slice(&times)));
        track.setDuration(self.duration.as_secs_f64());
        track.setBeginTime(CACurrentMediaTime() - elapsed.as_secs_f64());
        track
    }
}

/// The Core Animation clock reading at an `Instant`, which may be in the past.
///
/// Every layer in a panel shares the window's clock, with no offsets of its own, so the
/// media time at the root is the media time everywhere the marks are drawn.
fn media_time(at: Instant) -> f64 {
    let now = Instant::now();
    let offset = if at >= now {
        (at - now).as_secs_f64()
    } else {
        -(now - at).as_secs_f64()
    };
    CACurrentMediaTime() + offset
}
