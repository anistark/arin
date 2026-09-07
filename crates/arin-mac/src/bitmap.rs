//! Small images drawn in code.
//!
//! The menu bar icon and the marker's pointer are each a few dozen pixels describing a
//! circle, and an image file for either would be one more asset to keep in step with the
//! palette. So both are rasterised here from a function of position, at a pixel size
//! twice the point size, which is what keeps them sharp on a Retina panel.

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::NSImage;
use objc2_core_foundation::{CFData, CGSize};
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo,
};

/// Rasterise a square image `pixels` on a side, shown at `points` on a side.
///
/// `shade` is asked once per pixel, with the pixel's offset from the centre of the image
/// in pixels, and answers a premultiplied RGBA colour.
pub(crate) fn raster(
    pixels: usize,
    points: f64,
    shade: impl Fn(f64, f64) -> [u8; 4],
) -> Option<Retained<NSImage>> {
    let centre = (pixels as f64 - 1.0) / 2.0;
    let mut data = Vec::with_capacity(pixels * pixels * 4);
    for y in 0..pixels {
        for x in 0..pixels {
            data.extend_from_slice(&shade(x as f64 - centre, y as f64 - centre));
        }
    }

    // SAFETY: the pointer and length describe `data`, which CFData copies out of.
    let cf = unsafe { CFData::new(None, data.as_ptr(), data.len() as isize) }?;
    let provider = CGDataProvider::with_cf_data(Some(&cf))?;
    let space = CGColorSpace::new_device_rgb()?;
    // SAFETY: the dimensions, stride, and bitmap info describe the buffer above.
    let cg = unsafe {
        CGImage::new(
            pixels,
            pixels,
            8,
            32,
            pixels * 4,
            Some(&space),
            CGBitmapInfo(CGImageAlphaInfo::PremultipliedLast.0),
            Some(&provider),
            std::ptr::null(),
            true,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }?;

    // Sized in points rather than pixels, so the image is drawn at 2x on a Retina panel
    // instead of being scaled up from a smaller one.
    Some(NSImage::initWithCGImage_size(
        NSImage::alloc(),
        &cg,
        CGSize::new(points, points),
    ))
}

/// How much of a pixel `distance` from the centre falls inside a disc of `radius`.
///
/// One pixel of falloff, so an edge is anti-aliased rather than stepped.
pub(crate) fn coverage(distance: f64, radius: f64) -> f64 {
    (radius + 0.5 - distance).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::coverage;

    #[test]
    fn a_disc_is_solid_inside_and_soft_at_the_edge() {
        assert_eq!(coverage(0.0, 4.0), 1.0);
        assert_eq!(coverage(3.5, 4.0), 1.0);
        assert_eq!(coverage(4.0, 4.0), 0.5);
        assert_eq!(coverage(4.5, 4.0), 0.0);
        assert_eq!(coverage(9.0, 4.0), 0.0);
    }
}
