//! Turning a captured frame into something a hosted model will accept.
//!
//! Two jobs, and the second one is the one that causes bugs.
//!
//! The first is mechanical: a [`Frame`] is packed BGRA at whatever size the platform
//! produced, and the API wants base64 of an image format. That is a channel swap and a
//! PNG encode.
//!
//! The second is bookkeeping. A model answers in the pixel coordinates of *the image it
//! was sent*, which is not the frame, not the display, and not logical points. So the
//! encoded image carries its own dimensions back with it, and [`Encoded::to_logical`] is
//! the only place that conversion is spelled. Every coordinate bug in software of this
//! kind is a conversion done twice or not at all, and there is exactly one of them here.

use arin_core::{Error, Frame, Result};
use arin_protocol::{LogicalPoint, LogicalRect};
use base64::Engine as _;

/// Longest edge to send, in pixels.
///
/// The models this targets accept up to 2576 on the long edge. Sending less costs
/// accuracy and sending more is refused, so this is a ceiling on what the daemon captures
/// rather than a preference: a caller asking for a bigger frame gets it shrunk here.
pub const MAX_EDGE: u32 = 2576;

/// A frame ready to send, and what it takes to read the answer back.
pub struct Encoded {
    /// Base64 PNG, as the API's `data` field wants it.
    pub base64: String,
    /// Width of the image actually sent, in pixels.
    pub width: u32,
    /// Height of the image actually sent, in pixels.
    pub height: u32,
    /// The display's size in logical points, which is what the protocol speaks.
    logical_size: [f64; 2],
}

impl Encoded {
    /// Convert a position in the sent image into the logical points the protocol uses.
    ///
    /// Note what this does *not* consult: the frame's `scale`. A capture may be
    /// downscaled, this encoder may downscale it again, and the backing scale factor
    /// describes neither of those. The only honest ratio is the one between the image the
    /// model actually saw and the display it depicts.
    pub fn to_logical(&self, x: f64, y: f64) -> LogicalPoint {
        LogicalPoint::new(
            x / f64::from(self.width.max(1)) * self.logical_size[0],
            y / f64::from(self.height.max(1)) * self.logical_size[1],
        )
    }

    /// Convert a region of the sent image into logical points.
    pub fn rect_to_logical(&self, x: f64, y: f64, width: f64, height: f64) -> LogicalRect {
        let origin = self.to_logical(x, y);
        let far = self.to_logical(x + width, y + height);
        LogicalRect::new(origin.x, origin.y, far.x - origin.x, far.y - origin.y)
    }
}

/// Encode a frame as a base64 PNG, shrinking it if it is larger than the model accepts.
pub fn encode(frame: &Frame) -> Result<Encoded> {
    let (width, height) = (frame.width, frame.height);
    if width == 0 || height == 0 {
        return Err(Error::Capture("nothing was captured".into()));
    }
    let expected = width as usize * height as usize * 4;
    if frame.pixels.len() < expected {
        return Err(Error::Capture(format!(
            "frame is {} bytes, short of the {expected} its {width}x{height} claims",
            frame.pixels.len()
        )));
    }

    let (out_width, out_height) = fit(width, height, MAX_EDGE);
    let rgb = resample(frame, out_width, out_height);

    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, out_width, out_height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| Error::Capture(format!("png header: {e}")))?;
    writer
        .write_image_data(&rgb)
        .map_err(|e| Error::Capture(format!("png data: {e}")))?;
    writer
        .finish()
        .map_err(|e| Error::Capture(format!("png finish: {e}")))?;

    Ok(Encoded {
        base64: base64::engine::general_purpose::STANDARD.encode(&png),
        width: out_width,
        height: out_height,
        logical_size: frame.logical_size,
    })
}

/// The largest size within `max_edge` that keeps the aspect ratio.
fn fit(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest <= max_edge {
        return (width, height);
    }
    let scale = f64::from(max_edge) / f64::from(longest);
    (
        ((f64::from(width) * scale).round() as u32).max(1),
        ((f64::from(height) * scale).round() as u32).max(1),
    )
}

/// Convert BGRA to RGB, averaging over each source block when shrinking.
///
/// Averaging rather than picking the nearest pixel. Interface text is thin and high
/// contrast, which is exactly the content nearest-neighbour destroys: dropping rows takes
/// out whole strokes of a glyph and leaves the model reading something that is no longer
/// the word. Alpha is discarded because a screenshot is opaque.
fn resample(frame: &Frame, out_width: u32, out_height: u32) -> Vec<u8> {
    let (width, height) = (frame.width as usize, frame.height as usize);
    let (out_w, out_h) = (out_width as usize, out_height as usize);
    let mut rgb = vec![0u8; out_w * out_h * 3];

    for oy in 0..out_h {
        let y0 = oy * height / out_h;
        let y1 = (((oy + 1) * height).div_ceil(out_h))
            .min(height)
            .max(y0 + 1);
        for ox in 0..out_w {
            let x0 = ox * width / out_w;
            let x1 = (((ox + 1) * width).div_ceil(out_w)).min(width).max(x0 + 1);

            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for y in y0..y1 {
                for x in x0..x1 {
                    let idx = (y * width + x) * 4;
                    // Packed BGRA, as the capture backends document.
                    if let Some(px) = frame.pixels.get(idx..idx + 4) {
                        b += u32::from(px[0]);
                        g += u32::from(px[1]);
                        r += u32::from(px[2]);
                        n += 1;
                    }
                }
            }
            if n == 0 {
                continue;
            }
            let out = (oy * out_w + ox) * 3;
            rgb[out] = (r / n) as u8;
            rgb[out + 1] = (g / n) as u8;
            rgb[out + 2] = (b / n) as u8;
        }
    }
    rgb
}

#[cfg(test)]
mod tests {
    use super::*;
    use arin_protocol::DisplayId;
    use std::sync::Arc;

    fn frame(width: u32, height: u32, fill: impl Fn(u32, u32) -> [u8; 3]) -> Frame {
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        for y in 0..height {
            for x in 0..width {
                let [r, g, b] = fill(x, y);
                let idx = (y as usize * width as usize + x as usize) * 4;
                pixels[idx] = b;
                pixels[idx + 1] = g;
                pixels[idx + 2] = r;
                pixels[idx + 3] = 255;
            }
        }
        Frame {
            display: DisplayId(1),
            // Deliberately not 1.0, and deliberately not what the ratio below works out
            // to. Nothing in this module may consult it.
            scale: 2.0,
            logical_size: [1728.0, 1117.0],
            width,
            height,
            pixels: Arc::from(pixels),
        }
    }

    #[test]
    fn an_image_within_the_limit_is_sent_as_it_is() {
        assert_eq!(fit(1728, 1117, MAX_EDGE), (1728, 1117));
        assert_eq!(fit(MAX_EDGE, 100, MAX_EDGE), (MAX_EDGE, 100));
    }

    #[test]
    fn an_oversized_image_is_shrunk_and_keeps_its_shape() {
        let (w, h) = fit(3456, 2234, MAX_EDGE);
        assert_eq!(w, MAX_EDGE);
        let original = 3456.0 / 2234.0;
        let shrunk = f64::from(w) / f64::from(h);
        assert!(
            (original - shrunk).abs() < 0.01,
            "aspect ratio drifted from {original} to {shrunk}"
        );
    }

    #[test]
    fn a_tall_image_is_bounded_by_its_height() {
        let (w, h) = fit(1000, 4000, MAX_EDGE);
        assert_eq!(h, MAX_EDGE);
        assert!(w < h);
    }

    /// The conversion the whole module exists to get right. A model reports pixels of the
    /// image it was sent, and the protocol wants logical points of the display.
    #[test]
    fn image_pixels_convert_to_logical_points() {
        let encoded = encode(&frame(3456, 2234, |_, _| [10, 20, 30])).unwrap();
        // Shrunk on the way out, so the ratio is against the sent size and not the frame.
        assert_eq!(encoded.width, MAX_EDGE);

        let middle = encoded.to_logical(
            f64::from(encoded.width) / 2.0,
            f64::from(encoded.height) / 2.0,
        );
        assert!((middle.x - 864.0).abs() < 1.0, "got {}", middle.x);
        assert!((middle.y - 558.5).abs() < 1.0, "got {}", middle.y);

        let origin = encoded.to_logical(0.0, 0.0);
        assert_eq!((origin.x, origin.y), (0.0, 0.0));

        let corner = encoded.to_logical(f64::from(encoded.width), f64::from(encoded.height));
        assert!((corner.x - 1728.0).abs() < 1.0);
        assert!((corner.y - 1117.0).abs() < 1.0);
    }

    #[test]
    fn a_region_converts_by_its_corners() {
        let encoded = encode(&frame(1728, 1117, |_, _| [0, 0, 0])).unwrap();
        // A frame within the limit is sent one to one, so this is the identity case.
        let rect = encoded.rect_to_logical(100.0, 200.0, 340.0, 90.0);
        assert!((rect.x - 100.0).abs() < 0.01);
        assert!((rect.y - 200.0).abs() < 0.01);
        assert!((rect.width - 340.0).abs() < 0.01);
        assert!((rect.height - 90.0).abs() < 0.01);
    }

    #[test]
    fn the_encoded_bytes_are_a_png() {
        let encoded = encode(&frame(64, 32, |x, y| [x as u8, y as u8, 128])).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&encoded.base64)
            .expect("valid base64");
        assert_eq!(
            &bytes[..8],
            b"\x89PNG\r\n\x1a\n",
            "the API is told this is a png, so it had better be one"
        );
    }

    /// Channels are the classic silent bug: swap them and the image looks plausible, the
    /// model answers confidently, and nothing anywhere reports an error.
    #[test]
    fn the_channel_order_survives_the_round_trip() {
        // Solid red, which is unambiguous under a swap.
        let rgb = resample(&frame(8, 8, |_, _| [200, 40, 10]), 8, 8);
        assert_eq!(&rgb[..3], &[200, 40, 10]);
    }

    #[test]
    fn shrinking_averages_rather_than_dropping_pixels() {
        // Alternating black and white columns. Averaged, a two to one shrink is grey.
        // Nearest neighbour would give solid black or solid white.
        let striped = frame(64, 8, |x, _| {
            if x % 2 == 0 {
                [0, 0, 0]
            } else {
                [255, 255, 255]
            }
        });
        let rgb = resample(&striped, 32, 8);
        assert!(
            (100..=155).contains(&rgb[0]),
            "expected an average, got {}",
            rgb[0]
        );
    }

    #[test]
    fn an_empty_frame_is_an_error_rather_than_an_empty_png() {
        let empty = Frame {
            display: DisplayId(1),
            scale: 1.0,
            logical_size: [0.0, 0.0],
            width: 0,
            height: 0,
            pixels: Vec::new().into(),
        };
        assert!(encode(&empty).is_err());
    }

    #[test]
    fn a_frame_shorter_than_it_claims_is_an_error() {
        let truncated = Frame {
            display: DisplayId(1),
            scale: 1.0,
            logical_size: [100.0, 100.0],
            width: 100,
            height: 100,
            pixels: vec![0u8; 16].into(),
        };
        assert!(encode(&truncated).is_err());
    }
}
