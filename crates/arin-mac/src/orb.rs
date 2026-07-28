//! The orb.
//!
//! Three concentric radial gradients plus a particle emitter. Glow needs radial falloff,
//! so the rings are real `CAGradientLayer`s rather than stacked flat circles.
//!
//! # States
//!
//! The client never asks for a state. They follow from what the daemon is doing, which
//! is what keeps the visual grammar consistent across every agent driving it.
//!
//! | State | Rendering |
//! |---|---|
//! | Idle | Slow pulse, sparse embers, parked |
//! | Thinking | Faster pulse, denser embers, stationary |
//! | Traveling | Stretch along velocity, trail spawns along the arc |
//! | Pointing | Settled, brief flare, embers calm |
//! | Ending | Dims to faint core, embers stop, fade out |
//!
//! # Why stretch rather than rotation
//!
//! A circle has no facing, so rotating it reads as nothing at all. Motion has to show up
//! as deformation: the orb squashes along the axis it is travelling, the way a thrown
//! thing does, and returns to round when it arrives.

use arin_core::OrbState;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSColor;
use objc2_core_foundation::{CFData, CFRetained, CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapInfo, CGColor, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage,
    CGImageAlphaInfo, CGMutablePath,
};
use objc2_foundation::{NSArray, NSNumber, NSString, NSValue};
use objc2_quartz_core::{
    CABasicAnimation, CAEmitterCell, CAEmitterLayer, CAGradientLayer, CAKeyframeAnimation, CALayer,
    CAMediaTiming, CAMediaTimingFunction, CATransform3D, CATransform3DIdentity,
    NSValueCATransform3DAdditions, kCAGradientLayerRadial, kCAMediaTimingFunctionEaseIn,
    kCAMediaTimingFunctionEaseInEaseOut,
};

/// Brand colours. Blue belongs to Arin, which is why the contrast picker excludes it
/// when choosing annotation colours.
mod color {
    /// Halo and outer ring.
    pub const DEEP: (f64, f64, f64) = (
        0x1E as f64 / 255.0,
        0x3A as f64 / 255.0,
        0x8A as f64 / 255.0,
    );
    /// Ring.
    pub const MID: (f64, f64, f64) = (
        0x3B as f64 / 255.0,
        0x82 as f64 / 255.0,
        0xF6 as f64 / 255.0,
    );
    /// Inner core.
    pub const CORE: (f64, f64, f64) = (
        0xA9 as f64 / 255.0,
        0xDC as f64 / 255.0,
        0xFF as f64 / 255.0,
    );
    /// Embers.
    pub const SPARK: (f64, f64, f64) = (
        0x7F as f64 / 255.0,
        0xE3 as f64 / 255.0,
        0xFF as f64 / 255.0,
    );
}

/// Diameter of the halo in logical points. Everything else is a fraction of this.
///
/// Visible to the crate because a caption has to clear the orb it labels, and the orb is
/// the only thing that knows how big it is.
pub(crate) const HALO: f64 = 72.0;
const RING: f64 = 34.0;
const CORE: f64 = 14.0;

/// Below this size the orb drops its embers and tightens its halo.
///
/// The menu bar icon is this same primitive with features disabled, not a separate asset.
pub const MINIMUM_FEATURED_SIZE: f64 = 20.0;

/// How far the flight arc bows away from the straight line, as a fraction of distance.
const ARC_BOW: f64 = 0.22;

/// How much the orb squashes while travelling.
///
/// Generous, because a soft glowing blob deforming subtly reads as nothing at all. The
/// deformation is the only cue that the orb is moving under its own power.
const STRETCH: f64 = 0.55;

/// Size of the spark image in points. The cell scale below is relative to it.
const SPARK_SIZE: usize = 16;

/// Embers per second at full density.
///
/// The emitter layer's own birth rate multiplies this, so states scale it between zero
/// and one rather than setting a rate directly. A cell rate of zero would leave the
/// multiplier nothing to scale and the orb would never spark at all.
const EMBER_RATE: f32 = 34.0;

/// Animation keys, so a state change can replace its own animation and leave the others.
mod key {
    pub const PULSE: &str = "arin.pulse";
    pub const FLIGHT: &str = "arin.flight";
    pub const FLARE: &str = "arin.flare";
    pub const STRETCH_KEY: &str = "arin.stretch";
}

/// A single orb instance in a panel's layer tree.
pub struct Orb {
    root: Retained<CALayer>,
    embers: Retained<CAEmitterLayer>,
    state: OrbState,
    /// Where the orb is, in the panel's flipped coordinates.
    center: CGPoint,
    /// Whether it has ever been placed. The first point should not fly in from nowhere.
    placed: bool,
}

impl Orb {
    /// Build the layer tree. Not attached to anything until [`Orb::layer`] is added.
    pub fn new() -> Self {
        let root = CALayer::new();
        root.setBounds(rect(HALO));
        root.setFrame(rect(HALO));

        root.addSublayer(&radial(HALO, color::DEEP, 0.55));
        root.addSublayer(&radial(RING, color::MID, 0.85));
        root.addSublayer(&radial(CORE, color::CORE, 1.0));

        let embers = emitter();
        root.addSublayer(&embers);

        let orb = Self {
            root,
            embers,
            state: OrbState::Idle,
            center: CGPoint::new(0.0, 0.0),
            placed: false,
        };
        orb.apply_state();
        orb
    }

    /// The layer to add to a panel.
    pub fn layer(&self) -> &CALayer {
        &self.root
    }

    /// Show or hide without tearing the layer down.
    pub fn set_visible(&self, visible: bool) {
        self.root.setHidden(!visible);
    }

    /// Put the orb at a point with no travel. Used for the first placement.
    pub fn place(&mut self, point: CGPoint) {
        self.root
            .removeAnimationForKey(&NSString::from_str(key::FLIGHT));
        self.root.setPosition(point);
        self.root.setTransform(unsafe { CATransform3DIdentity });
        self.center = point;
        self.placed = true;
    }

    /// Fly to a point along a bowed arc, trailing embers.
    ///
    /// The arc is a quadratic curve bowed perpendicular to the direct line. A straight
    /// slide reads as a UI element being repositioned; a curve reads as something moving
    /// under its own power, which is the difference between a cursor and a companion.
    pub fn travel_to(&mut self, point: CGPoint) -> std::time::Duration {
        if !self.placed {
            self.place(point);
            self.settle();
            return std::time::Duration::ZERO;
        }

        let from = self.center;
        let dx = point.x - from.x;
        let dy = point.y - from.y;
        let distance = (dx * dx + dy * dy).sqrt();

        // Anything this close is a nudge, not a journey.
        if distance < 2.0 {
            self.place(point);
            self.settle();
            return std::time::Duration::ZERO;
        }

        let path = CGMutablePath::new();
        unsafe {
            CGMutablePath::move_to_point(Some(&path), std::ptr::null(), from.x, from.y);
            // Control point offset perpendicular to travel, which is what bows the arc.
            let control = CGPoint::new(
                (from.x + point.x) / 2.0 - dy * ARC_BOW,
                (from.y + point.y) / 2.0 + dx * ARC_BOW,
            );
            CGMutablePath::add_quad_curve_to_point(
                Some(&path),
                std::ptr::null(),
                control.x,
                control.y,
                point.x,
                point.y,
            );
        }

        // Longer trips take longer, but not linearly. Deliberate rather than quick: the
        // point of the flight is that a person can follow where the orb went, and at
        // half this duration the movement is over before the eye finds it.
        let duration = (0.45 + distance / 1400.0).min(1.4);

        let flight =
            CAKeyframeAnimation::animationWithKeyPath(Some(&NSString::from_str("position")));
        flight.setPath(Some(&path));
        flight.setDuration(duration);
        flight.setTimingFunction(Some(&CAMediaTimingFunction::functionWithName(unsafe {
            kCAMediaTimingFunctionEaseInEaseOut
        })));

        // Squash along the direction of travel, easing back to round over the flight.
        // Animating it rather than setting it means the resting transform stays identity
        // and arrival needs no callback to look right.
        let squash = CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str("transform")));
        unsafe {
            squash.setFromValue(Some(&*NSValue::valueWithCATransform3D(stretch_along(
                dx, dy,
            ))));
            squash.setToValue(Some(&*NSValue::valueWithCATransform3D(
                CATransform3DIdentity,
            )));
        }
        squash.setDuration(duration);
        // Ease in, so the orb holds its deformation through the flight and rounds out
        // near the end rather than relaxing the moment it sets off.
        squash.setTimingFunction(Some(&CAMediaTimingFunction::functionWithName(unsafe {
            kCAMediaTimingFunctionEaseIn
        })));

        self.root.setPosition(point);
        self.root
            .addAnimation_forKey(&flight, Some(&NSString::from_str(key::FLIGHT)));
        self.root
            .addAnimation_forKey(&squash, Some(&NSString::from_str(key::STRETCH_KEY)));

        self.center = point;
        self.set_state(OrbState::Traveling);
        std::time::Duration::from_secs_f64(duration)
    }

    /// Move to a state.
    pub fn set_state(&mut self, state: OrbState) {
        if self.state == state {
            return;
        }
        self.state = state;
        self.apply_state();
    }

    /// Settle after a flight: round again, with a brief flare.
    pub fn settle(&mut self) {
        self.root.setTransform(unsafe { CATransform3DIdentity });
        self.set_state(OrbState::Pointing);
        self.flare();
    }

    fn apply_state(&self) {
        // Density is a fraction of `EMBER_RATE`, not a rate, because the emitter layer
        // multiplies whatever the cell was built with.
        let (opacity, pulse_period, pulse_depth, ember_density) = match self.state {
            OrbState::Idle => (0.75, 2.4, 0.04, 0.18),
            OrbState::Thinking => (0.95, 0.7, 0.10, 0.65),
            OrbState::Traveling => (1.0, 0.0, 0.0, 1.0),
            OrbState::Pointing => (1.0, 1.6, 0.05, 0.12),
            OrbState::Ending => (0.2, 0.0, 0.0, 0.0),
        };

        self.root.setOpacity(opacity);
        self.embers.setBirthRate(ember_density);

        let pulse_key = NSString::from_str(key::PULSE);
        if pulse_period > 0.0 {
            let pulse = CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str(
                "transform.scale",
            )));
            unsafe {
                pulse.setFromValue(Some(&*number(1.0 - pulse_depth)));
                pulse.setToValue(Some(&*number(1.0 + pulse_depth)));
            }
            pulse.setDuration(pulse_period);
            pulse.setAutoreverses(true);
            pulse.setRepeatCount(f32::INFINITY);
            self.root.addAnimation_forKey(&pulse, Some(&pulse_key));
        } else {
            self.root.removeAnimationForKey(&pulse_key);
        }
    }

    /// A brief brightening on arrival, so the eye knows where to land.
    ///
    /// Brightness rather than size, both because that is what a flare is and because the
    /// transform is already carrying the stretch and the pulse.
    fn flare(&self) {
        let flare = CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str("opacity")));
        unsafe {
            flare.setFromValue(Some(&*number(0.45)));
            flare.setToValue(Some(&*number(1.0)));
        }
        flare.setDuration(0.28);
        flare.setTimingFunction(Some(&CAMediaTimingFunction::functionWithName(unsafe {
            kCAMediaTimingFunctionEaseInEaseOut
        })));
        self.root
            .addAnimation_forKey(&flare, Some(&NSString::from_str(key::FLARE)));
    }
}

impl Default for Orb {
    fn default() -> Self {
        Self::new()
    }
}

/// Squash along the travel direction and swell across it, preserving area by eye.
fn stretch_along(dx: f64, dy: f64) -> CATransform3D {
    let angle = dy.atan2(dx);
    let identity = unsafe { CATransform3DIdentity };
    // Rotate the stretch axis onto x, scale, then rotate back. Composing this way keeps
    // the deformation aligned with travel without ever rotating the orb itself, which
    // would be invisible on something round.
    identity
        .rotate(angle, 0.0, 0.0, 1.0)
        .scale(1.0 + STRETCH, 1.0 - STRETCH * 0.5, 1.0)
        .rotate(-angle, 0.0, 0.0, 1.0)
}

/// The ember emitter, and the cell it spawns.
fn emitter() -> Retained<CAEmitterLayer> {
    let cell = CAEmitterCell::new();
    cell.setBirthRate(EMBER_RATE);
    cell.setLifetime(0.9);
    cell.setVelocity(26.0);
    cell.setEmissionRange(std::f64::consts::TAU);
    // Relative to the spark image, which is SPARK_SIZE points across. Anything much
    // smaller than this rounds away to nothing on screen.
    cell.setScale(0.7);
    cell.setScaleSpeed(-0.45);
    cell.setAlphaSpeed(-1.0);
    cell.setColor(Some(&cg_color(color::SPARK, 1.0)));
    // SAFETY: the image is retained by the cell for as long as it draws with it.
    if let Some(spark) = spark_image() {
        let object: &AnyObject = <CFType as AsRef<AnyObject>>::as_ref(spark.as_ref());
        unsafe { cell.setContents(Some(object)) };
    } else {
        tracing::warn!("could not build the ember image, the orb will not spark");
    }

    let layer = CAEmitterLayer::new();
    layer.setEmitterPosition(CGPoint::new(HALO / 2.0, HALO / 2.0));
    layer.setEmitterSize(CGSize::new(RING / 2.0, RING / 2.0));
    layer.setEmitterShape(unsafe { objc2_quartz_core::kCAEmitterLayerCircle });
    layer.setRenderMode(unsafe { objc2_quartz_core::kCAEmitterLayerAdditive });
    layer.setEmitterCells(Some(&NSArray::from_slice(&[cell.as_ref()])));
    layer.setBirthRate(0.0);
    layer.setFrame(rect(HALO));

    layer
}

/// A soft round dot for the embers to draw.
///
/// Built rather than shipped as an asset: it is a radial falloff like everything else
/// here, and a file would be one more thing to keep in step with the palette.
///
/// This has to be a `CGImage`. An emitter cell draws its `contents` the way a layer
/// does, and handing it anything else, a `CALayer` included, silently produces no
/// particles at all.
fn spark_image() -> Option<CFRetained<CGImage>> {
    const SIZE: usize = SPARK_SIZE;
    let centre = (SIZE as f64 - 1.0) / 2.0;
    let radius = SIZE as f64 / 2.0;

    let (r, g, b) = color::SPARK;
    let mut pixels = vec![0u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f64 - centre;
            let dy = y as f64 - centre;
            let distance = (dx * dx + dy * dy).sqrt() / radius;
            // Squared falloff, so the spark has a bright middle and a soft edge rather
            // than looking like a hard little disc.
            let alpha = (1.0 - distance).clamp(0.0, 1.0).powi(2);
            let idx = (y * SIZE + x) * 4;
            // Premultiplied, which is what the bitmap info below promises.
            pixels[idx] = (r * alpha * 255.0) as u8;
            pixels[idx + 1] = (g * alpha * 255.0) as u8;
            pixels[idx + 2] = (b * alpha * 255.0) as u8;
            pixels[idx + 3] = (alpha * 255.0) as u8;
        }
    }

    // SAFETY: the pointer and length describe `pixels`, and CFData copies out of it
    // before this returns.
    let data = unsafe { CFData::new(None, pixels.as_ptr(), pixels.len() as isize) }?;
    let provider = CGDataProvider::with_cf_data(Some(&data))?;
    let space = CGColorSpace::new_device_rgb()?;

    // SAFETY: the dimensions, stride, and bitmap info all describe the buffer above.
    unsafe {
        CGImage::new(
            SIZE,
            SIZE,
            8,
            32,
            SIZE * 4,
            Some(&space),
            CGBitmapInfo(CGImageAlphaInfo::PremultipliedLast.0),
            Some(&provider),
            std::ptr::null(),
            true,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
}

/// A radial gradient from an opaque centre out to fully transparent.
fn radial(size: f64, rgb: (f64, f64, f64), center_alpha: f64) -> Retained<CAGradientLayer> {
    let layer = CAGradientLayer::new();
    layer.setFrame(centered(size));
    // `colors` is an NSArray of CGColorRef, so the Core Foundation values have to be
    // handed over as objects.
    let inner = cg_color(rgb, center_alpha);
    let outer = cg_color(rgb, 0.0);
    let colors: [&AnyObject; 2] = [as_object(&inner), as_object(&outer)];
    unsafe {
        layer.setType(kCAGradientLayerRadial);
        layer.setColors(Some(&NSArray::from_slice(&colors)));
    }
    layer.setLocations(Some(&NSArray::from_retained_slice(&[
        NSNumber::new_f64(0.0),
        NSNumber::new_f64(1.0),
    ])));
    // For a radial gradient these describe the centre and the extent of the falloff.
    layer.setStartPoint(CGPoint::new(0.5, 0.5));
    layer.setEndPoint(CGPoint::new(1.0, 1.0));
    layer
}

fn cg_color(rgb: (f64, f64, f64), alpha: f64) -> Retained<CGColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(rgb.0, rgb.1, rgb.2, alpha).CGColor()
}

fn number(value: f64) -> Retained<NSNumber> {
    NSNumber::new_f64(value)
}

/// Borrow a Core Foundation value as an object, for the Objective-C collections that
/// expect one.
fn as_object(color: &CGColor) -> &AnyObject {
    let cf: &CFType = color.as_ref();
    cf.as_ref()
}

fn rect(size: f64) -> CGRect {
    CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(size, size))
}

/// A sublayer of `size`, centred inside the halo-sized root.
fn centered(size: f64) -> CGRect {
    let offset = (HALO - size) / 2.0;
    CGRect::new(CGPoint::new(offset, offset), CGSize::new(size, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stretch_is_along_the_direction_of_travel() {
        // Travelling straight right: x grows, y shrinks.
        let right = stretch_along(100.0, 0.0);
        assert!(
            right.m11 > 1.0,
            "should lengthen along x, got {}",
            right.m11
        );
        assert!(right.m22 < 1.0, "should narrow across y, got {}", right.m22);

        // Travelling straight down swaps which axis lengthens.
        let down = stretch_along(0.0, 100.0);
        assert!(down.m22 > 1.0, "should lengthen along y, got {}", down.m22);
        assert!(down.m11 < 1.0, "should narrow across x, got {}", down.m11);
    }

    #[test]
    fn stretch_does_not_depend_on_distance() {
        // Only the direction matters. A long flight is not a flatter orb.
        let near = stretch_along(3.0, 4.0);
        let far = stretch_along(300.0, 400.0);
        assert!((near.m11 - far.m11).abs() < 1e-9);
        assert!((near.m22 - far.m22).abs() < 1e-9);
    }
}
