//! Screen capture via ScreenCaptureKit.
//!
//! Screen Recording is the only permission Arin asks for, and this is the only thing
//! that needs it. The first capture triggers the system prompt.
//!
//! # Excluding ourselves
//!
//! The filter asks ScreenCaptureKit to leave Arin's own windows out of the frame, so a
//! capture shows the screen as it would look with Arin not running.
//!
//! This was documented here as not working, on a measurement taken before the frame
//! geometry below was correct. Re-measured on macOS 15: it does hold. A textbox was drawn
//! over a white background, covering a third of the display's width, and a colour picked
//! for that same region afterwards came back with the answer for white rather than for
//! the near black panel sitting on it. Had the overlay been in the frame the two would
//! have differed, since the palette moves a long way between those backgrounds.
//!
//! Nothing depends on it either way, which is why the wrong note survived so long.
//! `crate::signature` tells an annotation apart from a scroll by how much of the screen
//! changed, and the daemon re-baselines around its own drawing regardless. Both remain as
//! insurance rather than as load bearing machinery.
//!
//! # One capturing process at a time
//!
//! While the overlay daemon runs, a second process asking for a screenshot does not get
//! one. ScreenCaptureKit releases the completion block without ever calling it, so the
//! request fails with no error to report. Measured by running `arin capture` against a
//! live daemon, and against a headless one, which succeeds.
//!
//! Nothing in the daemon is affected, since it is the one capturing. It is the diagnostic
//! commands that cannot take their own frame, so they ask the daemon rather than guessing
//! from a failure that looks identical to a missing permission.
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
use objc2::{AnyThread, Message as _};
use objc2_core_graphics::{CGDataProvider, CGDisplayCopyDisplayMode, CGDisplayMode, CGImage};
use objc2_foundation::NSError;
use objc2_screen_capture_kit::{
    SCContentFilter, SCScreenshotManager, SCShareableContent, SCStreamConfiguration,
};
use std::sync::Arc;
use std::sync::mpsc::{RecvTimeoutError, SyncSender, sync_channel};
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

impl MacCapture {
    /// Take one frame, downscaling to `max_edge` when asked to.
    fn shoot(display: DisplayId, max_edge: Option<u32>) -> Result<Frame> {
        let shot = capture_display(display, max_edge)?;

        // Derived from Core Graphics rather than AppKit. `NSScreen` needs the main
        // thread, capture never runs there, and a scale that silently differs by thread
        // is worse than one that is a little more work to obtain.
        let (logical_size, scale) = frame_geometry(shot.width, shot.height, geometry(display.0));

        Ok(Frame {
            display,
            scale,
            logical_size,
            width: shot.width,
            height: shot.height,
            pixels: Arc::from(shot.pixels),
        })
    }
}

impl arin_core::Capture for MacCapture {
    fn capture(&self, display: DisplayId) -> Result<Frame> {
        Self::shoot(display, self.max_edge)
    }

    fn capture_detailed(&self, display: DisplayId, min_long_edge: u32) -> Result<Frame> {
        // The configured ceiling is what keeps routine captures cheap, and raising it for
        // one caller is the whole point of the request. ScreenCaptureKit will not invent
        // pixels the display does not have, so asking for more than it is wide simply
        // returns full resolution, and `None` was already that.
        Self::shoot(
            display,
            self.max_edge
                .map(|configured| configured.max(min_long_edge)),
        )
    }
}

/// What area a shot covers, and at what resolution it recorded it.
///
/// `logical_size` is the *display's*, not the shot's. A downscaled frame still shows the
/// whole display, so the area it covers in logical points is unchanged and only the
/// detail differs. Dividing the shot's own dimensions by the backing scale instead
/// describes a frame covering a fraction of the screen, and anything mapping a rect into
/// that frame lands somewhere else entirely. That is not hypothetical: it is what sent
/// the contrast picker to the bottom right corner of every downscaled capture.
///
/// `scale` is then the frame's own pixels per logical point, which is what the documented
/// `width == logical_size[0] * scale` actually promises. For a full resolution capture it
/// equals the backing scale. For a downscaled one it is smaller, and reporting the
/// backing scale there would misplace anything converted through it.
fn frame_geometry(shot_width: u32, shot_height: u32, display: Option<Geometry>) -> ([f64; 2], f64) {
    let Some(geometry) = display else {
        // Nothing to correct against, so the shot describes itself.
        return ([f64::from(shot_width), f64::from(shot_height)], 1.0);
    };
    if geometry.scale <= 0.0 {
        return ([f64::from(shot_width), f64::from(shot_height)], 1.0);
    }

    let logical_size = [
        geometry.pixel_width as f64 / geometry.scale,
        geometry.pixel_height as f64 / geometry.scale,
    ];
    let scale = if logical_size[0] > 0.0 {
        f64::from(shot_width) / logical_size[0]
    } else {
        geometry.scale
    };
    (logical_size, scale)
}

/// Ask ScreenCaptureKit for one frame of a display.
///
/// Two round trips, deliberately not nested. The obvious shape is to request the image
/// from inside the shareable content handler, since that is where the content arrives.
/// Doing so deadlocks: the handler runs on ScreenCaptureKit's own callback queue, and
/// asking the framework for more work from inside it never returns whenever another
/// process is also an active client. It looks fine in isolation, which is what makes it
/// worth the extra channel: the failure only shows up once the daemon and a second
/// command are both using capture, and then it presents as a timeout with no error.
fn capture_display(display: DisplayId, max_edge: Option<u32>) -> Result<Shot> {
    let content = shareable_content()?;

    // Back on our own thread now, so this is an ordinary call rather than a reentrant one.
    let (filter, config) = build_filter(&content.0, display.0, max_edge).map_err(Error::Capture)?;

    let (tx, rx) = sync_channel::<std::result::Result<Shot, String>>(1);
    request_image(&filter, &config, tx);

    match rx.recv_timeout(TIMEOUT) {
        Ok(Ok(shot)) => Ok(shot),
        Ok(Err(message)) => Err(Error::Capture(message)),
        Err(RecvTimeoutError::Timeout) => Err(Error::Capture(
            "ScreenCaptureKit did not return an image in time".into(),
        )),
        Err(RecvTimeoutError::Disconnected) => Err(Error::Capture(
            "ScreenCaptureKit dropped the image request without answering".into(),
        )),
    }
}

/// What is on screen, as ScreenCaptureKit sees it.
///
/// Wrapped so it can leave the callback queue it arrives on. See [`capture_display`] for
/// why that matters.
struct ShareableContent(objc2::rc::Retained<SCShareableContent>);

// SAFETY: an immutable snapshot of the window list with no thread affinity. Nothing here
// touches AppKit or any main thread only API, and the value is read on exactly one thread
// after the handler that produced it has returned.
unsafe impl Send for ShareableContent {}

/// Ask what is shareable, and wait for the answer on the calling thread.
fn shareable_content() -> Result<ShareableContent> {
    let (tx, rx) = sync_channel::<std::result::Result<ShareableContent, String>>(1);

    let handler = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            let result = match error_message(error) {
                Some(message) => Err(permission_hint(message)),
                None => match unsafe { content.as_ref() } {
                    Some(content) => Ok(ShareableContent(content.retain())),
                    None => Err("no shareable content returned".into()),
                },
            };
            let _ = tx.send(result);
        },
    );

    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&handler) };

    match rx.recv_timeout(TIMEOUT) {
        Ok(Ok(content)) => Ok(content),
        Ok(Err(message)) => Err(Error::Capture(message)),
        Err(_) => Err(Error::Capture(
            "ScreenCaptureKit did not list the shareable content in time".into(),
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
    /// A 14 inch Retina panel: 1512 by 982 points at 2x.
    const RETINA: super::Geometry = super::Geometry {
        pixel_width: 3024,
        pixel_height: 1964,
        scale: 2.0,
    };

    /// A frame covers the whole display however few pixels it was recorded with.
    ///
    /// The regression: a downscaled shot used to report the area it covered as its own
    /// pixel count over the backing scale, claiming a 512 wide capture of a 1512 point
    /// display covered 256 points. Anything mapping a rect through that landed in the
    /// corner.
    #[test]
    fn a_downscaled_frame_still_covers_the_whole_display() {
        let (logical, scale) = super::frame_geometry(512, 332, Some(RETINA));
        assert_eq!(logical, [1512.0, 982.0]);
        // Its own pixels per point, not the panel's 2x.
        assert!((scale - 512.0 / 1512.0).abs() < 1e-9, "got {scale}");
    }

    #[test]
    fn a_full_resolution_frame_reports_the_backing_scale() {
        let (logical, scale) = super::frame_geometry(3024, 1964, Some(RETINA));
        assert_eq!(logical, [1512.0, 982.0]);
        assert!((scale - 2.0).abs() < 1e-9, "got {scale}");
    }

    /// The invariant `Frame` documents, at both resolutions.
    #[test]
    fn width_is_always_the_logical_width_times_the_scale() {
        for shot in [3024u32, 1024, 512, 256] {
            let (logical, scale) = super::frame_geometry(shot, 332, Some(RETINA));
            assert!(
                (logical[0] * scale - f64::from(shot)).abs() < 1e-6,
                "{shot} wide broke the invariant"
            );
        }
    }

    #[test]
    fn a_display_that_cannot_be_measured_lets_the_shot_describe_itself() {
        let (logical, scale) = super::frame_geometry(800, 600, None);
        assert_eq!(logical, [800.0, 600.0]);
        assert_eq!(scale, 1.0);
    }

    #[test]
    fn a_nonsense_scale_does_not_divide_by_zero() {
        let broken = super::Geometry {
            pixel_width: 100,
            pixel_height: 100,
            scale: 0.0,
        };
        let (logical, scale) = super::frame_geometry(50, 50, Some(broken));
        assert!(logical[0].is_finite() && scale.is_finite());
    }

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
