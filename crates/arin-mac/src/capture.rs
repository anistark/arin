//! Screen capture via ScreenCaptureKit.
//!
//! Screen Recording is the only permission Arin asks for, and this is the only thing
//! that needs it. The first capture triggers the system prompt.
//!
//! # Excluding ourselves, which does not work
//!
//! The filter asks ScreenCaptureKit to leave Arin's own windows out of the frame, so a
//! capture would show the screen as it would look with Arin not running.
//!
//! Measured, it does not hold. Both forms were tried, `excludingWindows` with our panel
//! and `excludingApplications` with our process. Both identify us correctly and neither
//! keeps the overlay out of the capture.
//!
//! It is left in because it is the right request to make and costs nothing if it starts
//! working. Nothing depends on it: `crate::signature` tells an annotation apart from a
//! scroll by how much of the screen changed, which does not care whether the overlay is
//! in the frame.
//!
//! # Blocking
//!
//! [`arin_core::Capture`] is synchronous and ScreenCaptureKit is not, so this bridges by
//! blocking on a channel until the completion handler fires. Never call it from the main
//! thread: the handlers need a working run loop, and blocking the thread they may want
//! would deadlock. The daemon calls it from a worker, which is safe.

use arin_core::{Error, Frame, Result};
use arin_protocol::DisplayId;
use block2::RcBlock;
use objc2::AnyThread;
use objc2_core_graphics::{CGDataProvider, CGDisplayCopyDisplayMode, CGDisplayMode, CGImage};
use objc2_foundation::NSError;
use objc2_screen_capture_kit::{
    SCContentFilter, SCScreenshotManager, SCShareableContent, SCStreamConfiguration,
};
use std::sync::Arc;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::time::Duration;

/// How long to wait for ScreenCaptureKit before giving up.
///
/// Generous, because the very first call is the one that raises the permission dialog
/// and does not come back until the user has answered it.
const TIMEOUT: Duration = Duration::from_secs(30);

/// A captured frame's pixels, already detached from any Objective-C object so they can
/// cross the channel back to the caller.
struct Shot {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
}

/// The pixel and point dimensions of a display.
struct Geometry {
    pixel_width: usize,
    pixel_height: usize,
    scale: f64,
}

/// Ask Core Graphics for a display's real dimensions.
///
/// Works from any thread, which is the point: AppKit's version of this needs the main
/// one, and capture is explicitly not allowed there.
fn geometry(display: u32) -> Option<Geometry> {
    let mode = CGDisplayCopyDisplayMode(display)?;
    let points = CGDisplayMode::width(Some(&mode));
    let pixel_width = CGDisplayMode::pixel_width(Some(&mode));
    let pixel_height = CGDisplayMode::pixel_height(Some(&mode));
    if points == 0 {
        return None;
    }
    Some(Geometry {
        pixel_width,
        pixel_height,
        scale: pixel_width as f64 / points as f64,
    })
}

/// Captures the screen on macOS.
#[derive(Debug, Clone, Default)]
pub struct MacCapture {
    /// Longest edge to capture, in pixels. `None` captures at full resolution.
    ///
    /// Full resolution is what a grounding model wants. Change detection does not: it
    /// hashes a coarse grid, and a Retina frame is over twenty megabytes that would be
    /// allocated and copied twice a second for the life of a session.
    max_edge: Option<u32>,
}

impl MacCapture {
    /// A capture backend that downscales to `max_edge` on the longest side.
    pub fn downscaled(max_edge: u32) -> Self {
        Self {
            max_edge: Some(max_edge),
        }
    }
}

impl arin_core::Capture for MacCapture {
    fn capture(&self, display: DisplayId) -> Result<Frame> {
        let shot = capture_display(display, self.max_edge)?;

        // Derived from Core Graphics rather than AppKit. `NSScreen` needs the main
        // thread, capture never runs there, and a scale that silently differs by thread
        // is worse than one that is a little more work to obtain.
        let scale = geometry(display.0).map_or(1.0, |g| g.scale);
        Ok(Frame {
            display,
            scale,
            logical_size: [
                f64::from(shot.width) / scale,
                f64::from(shot.height) / scale,
            ],
            width: shot.width,
            height: shot.height,
            pixels: Arc::from(shot.pixels),
        })
    }
}

/// Ask ScreenCaptureKit for one frame of a display.
fn capture_display(display: DisplayId, max_edge: Option<u32>) -> Result<Shot> {
    let (tx, rx) = sync_channel::<std::result::Result<Shot, String>>(1);
    let target = display.0;

    // ScreenCaptureKit hands back the shareable content asynchronously, and the capture
    // itself is asynchronous again, so the second request is made from inside the first
    // one's handler.
    let outer = {
        let tx = tx.clone();
        RcBlock::new(
            move |content: *mut SCShareableContent, error: *mut NSError| {
                if let Some(message) = error_message(error) {
                    let _ = tx.send(Err(permission_hint(message)));
                    return;
                }
                match unsafe { content.as_ref() } {
                    Some(content) => match build_filter(content, target, max_edge) {
                        Ok((filter, config)) => request_image(&filter, &config, tx.clone()),
                        Err(e) => {
                            let _ = tx.send(Err(e));
                        }
                    },
                    None => {
                        let _ = tx.send(Err("no shareable content returned".into()));
                    }
                }
            },
        )
    };

    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&outer) };

    match rx.recv_timeout(TIMEOUT) {
        Ok(Ok(shot)) => Ok(shot),
        Ok(Err(message)) => Err(Error::Capture(message)),
        Err(_) => Err(Error::Capture(
            "ScreenCaptureKit did not respond in time".into(),
        )),
    }
}

/// Ask for the image itself, reporting whatever comes back down the channel.
fn request_image(
    filter: &SCContentFilter,
    config: &SCStreamConfiguration,
    tx: SyncSender<std::result::Result<Shot, String>>,
) {
    let handler = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        if let Some(message) = error_message(error) {
            let _ = tx.send(Err(message));
            return;
        }
        let result = match unsafe { image.as_ref() } {
            Some(image) => read_pixels(image),
            None => Err("no image returned".into()),
        };
        let _ = tx.send(result);
    });
    unsafe {
        SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
            filter,
            config,
            Some(&handler),
        );
    }
}

/// Build a filter for one display that leaves Arin's own windows out of the frame.
fn build_filter(
    content: &SCShareableContent,
    target: u32,
    max_edge: Option<u32>,
) -> std::result::Result<
    (
        objc2::rc::Retained<SCContentFilter>,
        objc2::rc::Retained<SCStreamConfiguration>,
    ),
    String,
> {
    let display = unsafe { content.displays() }
        .iter()
        .find(|d| unsafe { d.displayID() } == target)
        .ok_or_else(|| format!("no display with id {target} is shareable"))?;

    // Excluded window by window rather than application by application. The application
    // form of this filter does not keep our overlay out of the frame: it matches our
    // process, and the panels still come back in the capture. Windows are what the
    // compositor actually excludes.
    let ours = std::process::id() as i32;
    let mine: Vec<_> = unsafe { content.windows() }
        .iter()
        .filter(|window| {
            unsafe { window.owningApplication() }
                .is_some_and(|app| unsafe { app.processID() } == ours)
        })
        .collect();

    tracing::debug!(
        pid = ours,
        excluded = mine.len(),
        on_screen = unsafe { content.windows() }.len(),
        "excluding our own windows"
    );

    let excluded =
        objc2_foundation::NSArray::from_slice(&mine.iter().map(|w| w.as_ref()).collect::<Vec<_>>());

    let filter = unsafe {
        SCContentFilter::initWithDisplay_excludingWindows(
            SCContentFilter::alloc(),
            &display,
            &excluded,
        )
    };

    // `SCDisplay` reports points. Sizing the capture to those would throw away half the
    // detail on a Retina panel, so ask Core Graphics for the pixel dimensions.
    let (mut width, mut height) = match geometry(target) {
        Some(g) => (g.pixel_width, g.pixel_height),
        None => unsafe { (display.width() as usize, display.height() as usize) },
    };

    if let Some(max_edge) = max_edge {
        let max_edge = max_edge as usize;
        let longest = width.max(height);
        if longest > max_edge {
            width = width * max_edge / longest;
            height = height * max_edge / longest;
        }
    }

    let config = unsafe { SCStreamConfiguration::new() };
    unsafe {
        config.setWidth(width);
        config.setHeight(height);
        // The pointer is not part of the content, and including it would make every
        // mouse move look like the page changed.
        config.setShowsCursor(false);
    }

    Ok((filter, config))
}

/// Copy a captured image's pixels out into a plain buffer.
///
/// Rows are copied one at a time because Core Graphics pads each row up to an alignment
/// boundary, so the source stride is usually wider than `width * 4`.
fn read_pixels(image: &CGImage) -> std::result::Result<Shot, String> {
    let width = CGImage::width(Some(image));
    let height = CGImage::height(Some(image));
    let stride = CGImage::bytes_per_row(Some(image));
    let bits = CGImage::bits_per_pixel(Some(image));

    if bits != 32 {
        return Err(format!("expected 32 bits per pixel, got {bits}"));
    }

    // The layout is what Core Graphics chose, not what we asked for, so record it rather
    // than assume. `Frame` documents BGRA, which is little endian order with alpha first.
    tracing::debug!(
        width,
        height,
        stride,
        alpha = ?CGImage::alpha_info(Some(image)),
        byte_order = ?CGImage::byte_order_info(Some(image)),
        "captured image format"
    );

    let provider =
        CGImage::data_provider(Some(image)).ok_or_else(|| "image has no data".to_owned())?;
    let data = CGDataProvider::data(Some(&provider))
        .ok_or_else(|| "could not copy image data".to_owned())?;

    let len = data.len();
    // SAFETY: the pointer and length come from the same CFData, which outlives the copy
    // below because `data` is still held.
    let bytes = unsafe { std::slice::from_raw_parts(data.byte_ptr(), len) };

    Ok(Shot {
        pixels: unpad(bytes, width, height, stride)?,
        width: width as u32,
        height: height as u32,
    })
}

/// Copy `height` rows of `width` pixels out of a buffer whose rows are `stride` apart.
///
/// Split out from the capture path because it is pure, and because row padding is the
/// detail most likely to be wrong: Core Graphics aligns each row, so the source stride is
/// usually wider than the pixels in it and a naive copy shears the image.
fn unpad(
    bytes: &[u8],
    width: usize,
    height: usize,
    stride: usize,
) -> std::result::Result<Vec<u8>, String> {
    let row = width * 4;
    if stride < row {
        return Err(format!("stride {stride} is narrower than a {row} byte row"));
    }

    let mut pixels = Vec::with_capacity(row * height);
    for y in 0..height {
        let start = y * stride;
        let end = start + row;
        if end > bytes.len() {
            return Err("image data is shorter than its dimensions".into());
        }
        pixels.extend_from_slice(&bytes[start..end]);
    }
    Ok(pixels)
}

/// Turn an Objective-C error out-parameter into a message.
fn error_message(error: *mut NSError) -> Option<String> {
    unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string())
}

/// Point at the likely cause, since the first failure is almost always the permission.
fn permission_hint(message: String) -> String {
    format!(
        "{message}. Screen Recording is the one permission Arin needs. Grant it in System \
         Settings under Privacy and Security, then restart the daemon."
    )
}

#[cfg(test)]
mod tests {
    use super::unpad;

    #[test]
    fn an_unpadded_buffer_is_copied_whole() {
        // 2x2 pixels, no padding.
        let bytes: Vec<u8> = (0..16).collect();
        assert_eq!(unpad(&bytes, 2, 2, 8).unwrap(), bytes);
    }

    #[test]
    fn padding_is_dropped_from_the_end_of_each_row() {
        // 1 pixel wide, 2 rows, each row padded out to 8 bytes.
        let bytes = vec![
            1, 2, 3, 4, 9, 9, 9, 9, // row 0, then padding
            5, 6, 7, 8, 9, 9, 9, 9, // row 1, then padding
        ];
        assert_eq!(
            unpad(&bytes, 1, 2, 8).unwrap(),
            vec![1, 2, 3, 4, 5, 6, 7, 8]
        );
    }

    #[test]
    fn a_stride_narrower_than_a_row_is_refused() {
        let bytes = vec![0u8; 32];
        assert!(unpad(&bytes, 4, 2, 8).is_err());
    }

    #[test]
    fn a_short_buffer_is_refused_rather_than_read_past() {
        let bytes = vec![0u8; 8];
        assert!(unpad(&bytes, 1, 4, 8).is_err());
    }
}
