//! Captions: the short text a client attaches to a point or a highlight.
//!
//! A caption is not a [`textbox`](crate::host). A textbox is the content an agent asked
//! to place, sized and positioned by the client. A caption is a label on a mark, so its
//! size comes from the text and its position comes from whatever it is labelling. The
//! client says `label: "Save"` and nothing about where that goes.
//!
//! # Staying on screen
//!
//! A mark near an edge is exactly where a caption wants to overflow, and a caption half
//! off the display is worse than none. Placement therefore has a preferred side and a
//! fallback: beside the orb it flips left when the right runs out, above a highlight it
//! drops below when the top does. After flipping it still clamps, which is what covers a
//! mark in a corner where neither side fits outright.
//!
//! The placement functions are pure geometry in the panel's own coordinates, which is
//! what makes the edge cases testable without a screen.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSFont, NSFontAttributeName, NSStringDrawing};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSDictionary, NSString};
use objc2_quartz_core::{CALayer, CATextLayer};

use arin_core::Rgb;

use crate::host::annotation_color;
use crate::orb::HALO;

/// Type size, in logical points.
const FONT_SIZE: f64 = 13.0;

/// The font captions are measured and drawn in.
///
/// `CATextLayer` draws in Helvetica unless it is handed a font, and measuring in one font
/// while drawing in another gives a pill that does not fit its own text. Naming the
/// default explicitly is what keeps the two in step, and it matches the textbox, which
/// takes the same default.
const FONT_NAME: &str = "Helvetica";

/// Space between the text and the edge of the pill.
const PADDING_X: f64 = 8.0;
const PADDING_Y: f64 = 4.0;

/// Space between the pill and the mark it labels.
const GAP: f64 = 8.0;

/// How close a caption may sit to the edge of the display.
const MARGIN: f64 = 6.0;

/// Longest line a caption draws before it truncates.
///
/// A label is a few words. Anything longer is a textbox that came through the wrong
/// field, and letting it run the width of the display would cover what it describes.
const MAX_TEXT_WIDTH: f64 = 260.0;

/// A caption placed beside the orb.
///
/// `center` is the orb's centre in panel coordinates, which is where the point landed.
pub fn beside_orb(
    text: &str,
    center: CGPoint,
    color: Rgb,
    panel: CGSize,
    scale: f64,
) -> Retained<CALayer> {
    let size = size_for(text);
    pill(
        text,
        CGRect::new(place_beside(center, size, panel), size),
        color,
        scale,
    )
}

/// A caption placed against a highlighted region.
///
/// `rect` is the region in panel coordinates, already converted from the protocol's
/// top-left origin.
pub fn against_rect(
    text: &str,
    rect: CGRect,
    color: Rgb,
    panel: CGSize,
    scale: f64,
) -> Retained<CALayer> {
    let size = size_for(text);
    pill(
        text,
        CGRect::new(place_above(rect, size, panel), size),
        color,
        scale,
    )
}

/// Whether a label is worth drawing.
///
/// An empty or blank label is a client sending the field rather than omitting it, and a
/// pill containing nothing is just a smudge over the thing being pointed at.
pub fn is_drawable(label: Option<&String>) -> Option<&str> {
    label.map(String::as_str).filter(|t| !t.trim().is_empty())
}

/// The pill: a dark rounded plate with the text inside it.
///
/// Dark and mostly opaque for the same reason the textbox is. A caption lands on top of
/// whatever the user has on screen, and legibility cannot depend on what that is. The
/// border is the annotation colour at low alpha, enough to place the caption in the same
/// family as the mark without competing with it.
fn pill(text: &str, frame: CGRect, color: Rgb, scale: f64) -> Retained<CALayer> {
    let plate = CALayer::new();
    plate.setFrame(frame);
    plate.setBackgroundColor(Some(&crate::host::srgb(0.06, 0.07, 0.10, 0.92)));
    plate.setCornerRadius(5.0);
    plate.setBorderWidth(1.0);
    plate.setBorderColor(Some(&annotation_color(color, 0.55)));

    let label = CATextLayer::new();
    label.setFrame(CGRect::new(
        CGPoint::new(PADDING_X, PADDING_Y),
        CGSize::new(
            (frame.size.width - PADDING_X * 2.0).max(1.0),
            (frame.size.height - PADDING_Y * 2.0).max(1.0),
        ),
    ));
    unsafe { label.setString(Some(&NSString::from_str(text))) };
    label.setFontSize(FONT_SIZE);
    label.setForegroundColor(Some(&crate::host::srgb(0.93, 0.95, 0.98, 1.0)));
    // One line. A label that outruns `MAX_TEXT_WIDTH` loses its tail rather than its
    // position, because where the caption sits is what carries the meaning.
    label.setWrapped(false);
    unsafe { label.setTruncationMode(objc2_quartz_core::kCATruncationEnd) };
    // Without this the text renders at 1x and is scaled up, which on a Retina panel looks
    // soft in exactly the way real text does not.
    label.setContentsScale(scale);

    plate.addSublayer(&label);
    plate
}

/// The pill's size for a given string, text plus padding.
fn size_for(text: &str) -> CGSize {
    let text_size = measure(text);
    CGSize::new(
        text_size.width.min(MAX_TEXT_WIDTH) + PADDING_X * 2.0,
        text_size.height + PADDING_Y * 2.0,
    )
}

/// Measure one line in the caption font.
///
/// Falls back to an estimate if the font is missing, which it should not be, since a
/// caption that is slightly the wrong size still points at the right thing while a panic
/// on a render thread takes the overlay with it.
fn measure(text: &str) -> CGSize {
    let Some(font) = NSFont::fontWithName_size(&NSString::from_str(FONT_NAME), FONT_SIZE) else {
        tracing::warn!(
            font = FONT_NAME,
            "caption font missing, estimating the width"
        );
        return CGSize::new(text.chars().count() as f64 * FONT_SIZE * 0.55, FONT_SIZE);
    };

    let value: &AnyObject = &font;
    let attributes = NSDictionary::from_slices(&[unsafe { NSFontAttributeName }], &[value]);
    // SAFETY: the dictionary holds the key this attribute is documented to take, an
    // NSFont under NSFontAttributeName.
    unsafe { NSString::from_str(text).sizeWithAttributes(Some(&attributes)) }
}

/// Place a caption beside the orb, preferring its right.
///
/// Vertically centred on the orb rather than aligned to an edge, so the caption reads as
/// belonging to it regardless of how tall the text turned out.
fn place_beside(center: CGPoint, size: CGSize, panel: CGSize) -> CGPoint {
    let clearance = HALO / 2.0 + GAP;
    let right = center.x + clearance;
    // Flip to the left only when the right genuinely has no room, so captions stay on a
    // consistent side for every mark that is not near the edge.
    let x = if right + size.width + MARGIN > panel.width {
        center.x - clearance - size.width
    } else {
        right
    };

    CGPoint::new(
        clamp(x, MARGIN, panel.width - size.width - MARGIN),
        clamp(
            center.y - size.height / 2.0,
            MARGIN,
            panel.height - size.height - MARGIN,
        ),
    )
}

/// Place a caption against a region, preferring above it.
///
/// Left aligned with the region, because a caption that lines up with the edge of what it
/// describes reads as attached to it, while a centred one reads as floating.
///
/// Panel coordinates grow upward, so above the region is a larger y.
fn place_above(rect: CGRect, size: CGSize, panel: CGSize) -> CGPoint {
    let above = rect.origin.y + rect.size.height + GAP;
    let y = if above + size.height + MARGIN > panel.height {
        // No room above, so drop underneath rather than overlap the region itself.
        rect.origin.y - GAP - size.height
    } else {
        above
    };

    CGPoint::new(
        clamp(rect.origin.x, MARGIN, panel.width - size.width - MARGIN),
        clamp(y, MARGIN, panel.height - size.height - MARGIN),
    )
}

/// Clamp, tolerating a range that has collapsed.
///
/// A caption wider than the display leaves no valid position at all, and `f64::clamp`
/// panics when its bounds cross. Pinning to the near edge keeps the text visible from one
/// side instead of taking the overlay down.
fn clamp(value: f64, min: f64, max: f64) -> f64 {
    if max < min {
        min
    } else {
        value.clamp(min, max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 14 inch panel, matching the numbers in `host`.
    const PANEL: CGSize = CGSize::new(1512.0, 982.0);
    /// A caption about the size of "Save".
    const SMALL: CGSize = CGSize::new(52.0, 21.0);

    #[test]
    fn a_caption_sits_to_the_right_of_the_orb() {
        let at = place_beside(CGPoint::new(400.0, 500.0), SMALL, PANEL);
        assert!(
            at.x > 400.0 + HALO / 2.0,
            "should clear the orb, got {}",
            at.x
        );
    }

    #[test]
    fn a_caption_is_centred_on_the_orb() {
        let at = place_beside(CGPoint::new(400.0, 500.0), SMALL, PANEL);
        assert_eq!(at.y + SMALL.height / 2.0, 500.0);
    }

    #[test]
    fn a_caption_flips_left_at_the_right_edge() {
        // An orb near the right edge has no room for a caption beside it.
        let at = place_beside(CGPoint::new(PANEL.width - 20.0, 500.0), SMALL, PANEL);
        assert!(
            at.x + SMALL.width < PANEL.width - 20.0,
            "should have flipped to the left of the orb, got {}",
            at.x
        );
    }

    #[test]
    fn a_caption_never_leaves_the_panel() {
        // Every corner, plus dead centre, and nothing may hang off an edge.
        for (x, y) in [
            (0.0, 0.0),
            (PANEL.width, 0.0),
            (0.0, PANEL.height),
            (PANEL.width, PANEL.height),
            (PANEL.width / 2.0, PANEL.height / 2.0),
        ] {
            let at = place_beside(CGPoint::new(x, y), SMALL, PANEL);
            assert!(at.x >= 0.0 && at.x + SMALL.width <= PANEL.width, "x {at:?}");
            assert!(
                at.y >= 0.0 && at.y + SMALL.height <= PANEL.height,
                "y {at:?}"
            );
        }
    }

    #[test]
    fn a_region_caption_sits_above_it() {
        let rect = CGRect::new(CGPoint::new(100.0, 400.0), CGSize::new(340.0, 90.0));
        let at = place_above(rect, SMALL, PANEL);
        // Panel coordinates grow upward, so above the region is past its top edge.
        assert!(
            at.y >= rect.origin.y + rect.size.height,
            "should clear the top of the region, got {}",
            at.y
        );
        assert_eq!(at.x, rect.origin.x, "should line up with the region's left");
    }

    #[test]
    fn a_region_caption_drops_below_when_the_top_runs_out() {
        // A region against the top of the screen: protocol y of zero, so its top edge is
        // the panel ceiling and there is nothing above it.
        let rect = CGRect::new(
            CGPoint::new(100.0, PANEL.height - 90.0),
            CGSize::new(340.0, 90.0),
        );
        let at = place_above(rect, SMALL, PANEL);
        assert!(
            at.y + SMALL.height <= rect.origin.y,
            "should have dropped below the region, got {}",
            at.y
        );
    }

    #[test]
    fn a_caption_wider_than_the_display_does_not_panic() {
        let huge = CGSize::new(PANEL.width + 400.0, 21.0);
        let at = place_beside(CGPoint::new(700.0, 500.0), huge, PANEL);
        assert_eq!(at.x, MARGIN);
    }

    #[test]
    fn a_blank_label_is_not_drawable() {
        assert_eq!(is_drawable(None), None);
        assert_eq!(is_drawable(Some(&String::new())), None);
        assert_eq!(is_drawable(Some(&"   ".to_string())), None);
        assert_eq!(is_drawable(Some(&"Save".to_string())), Some("Save"));
    }
}
