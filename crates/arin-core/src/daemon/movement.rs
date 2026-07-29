//! Following a mark when the content under it moves.

use super::{Appearance, Daemon};
use crate::annotation::Annotation;
use crate::contrast::{self, Footprint, Rgb};
use crate::fingerprint::Fingerprint;
use crate::traits::Frame;
use arin_protocol::{
    AnnotationId, DisplayId, Invalidated, InvalidationReason, LogicalRect, SessionId,
};

/// How far around a mark to look when measuring what moved under it, in logical points.
///
/// Two things pull against each other. Too tight and there is not enough content to
/// correlate against. Too wide and the region takes in the toolbars and neighbouring
/// windows that made the display-wide measurement useless in the first place, which is
/// the failure being fixed and so the one to lean away from.
///
/// The wrong reading of "not enough content" is what makes this tempting to raise. A
/// region with no structure in it is not a failure: it reports no movement, the mark stays
/// where it is, and a plain background is exactly the kind of thing that does not visibly
/// move. Widening buys correlation the mark did not need, at the price of measuring a
/// window it is not in.
///
/// It also bounds how far a scroll can jump and still be followed, since the search only
/// reaches half the region. A flick that throws the page further than that reads as
/// unexplainable and the mark goes, which is the right answer to content that is no longer
/// anywhere near where it was.
const CONTEXT: f64 = 120.0;

/// The patch of screen a mark's movement is measured against.
///
/// The mark's own ink is inside it and is left there rather than masked out. The overlay
/// is in the frame, so between two ticks the mark has not moved and votes for an offset of
/// zero against content that may have scrolled. Masking it would leave holes in the profile
/// that vote for zero exactly as loudly, so it is cheaper to let the surrounding content
/// outvote it and let the residual carry the disagreement.
fn neighbourhood(anchor: LogicalRect, display: [f64; 2]) -> LogicalRect {
    // Fixed, not proportional to the mark. A margin that scaled with a six hundred point
    // highlight would cover a laptop display, which is the display-wide measurement this
    // exists to avoid.
    let left = (anchor.x - CONTEXT).max(0.0);
    let top = (anchor.y - CONTEXT).max(0.0);
    let right = (anchor.x + anchor.width + CONTEXT).min(display[0]);
    let bottom = (anchor.y + anchor.height + CONTEXT).min(display[1]);

    LogicalRect::new(left, top, (right - left).max(1.0), (bottom - top).max(1.0))
}

/// What became of a display's annotations when its content moved.
#[derive(Debug, Default)]
pub struct Followed {
    /// Marks that kept up with the content and were redrawn.
    pub moved: Vec<AnnotationId>,
    /// Marks that could not be followed, and the invalidations sent for them.
    pub invalidated: Vec<Invalidated>,
}

impl Daemon {
    /// What one look at the screen tells the daemon about a mark it is about to draw.
    ///
    /// Two answers from one capture: the colour to draw in, and a record of the content
    /// being marked so a later scroll can check the mark stayed on it. The capture used
    /// to buy only the colour, which made it hard to justify at around 100ms per
    /// positioned annotation. It now also buys the only per-annotation evidence the
    /// daemon has that following a scroll put the mark somewhere sensible.
    pub(super) fn appearance(
        &self,
        asked: Option<&str>,
        screen: DisplayId,
        footprint: &Footprint,
        region: LogicalRect,
    ) -> Appearance {
        // A named colour wins outright. An unparseable one falls through to the adaptive
        // pick rather than the default, which serves the intent better.
        let named = asked.and_then(|name| {
            let parsed = Rgb::parse(name);
            if parsed.is_none() {
                tracing::debug!(color = name, "unparseable colour, choosing one");
            }
            parsed
        });

        if !self.config.adaptive_color {
            return Appearance {
                color: named.unwrap_or(contrast::DEFAULT),
                fingerprint: None,
            };
        }

        // A failed capture is not an error. Screen Recording may not be granted, and a
        // mark in the default colour beats a refused request.
        match self.capture.capture(screen) {
            Ok(frame) => Appearance {
                color: named.unwrap_or_else(|| {
                    let picked = contrast::pick(&frame, footprint);
                    // Otherwise unobservable: colour management alters the pixels on the
                    // way out, so they cannot be compared against the palette.
                    tracing::debug!(
                        color = %picked,
                        adapted = picked != contrast::DEFAULT,
                        "chose an annotation colour"
                    );
                    picked
                }),
                fingerprint: Fingerprint::of(&frame, region),
            },
            Err(e) => {
                tracing::debug!(%screen, error = %e, "no frame to sample, using the default colour");
                Appearance {
                    color: named.unwrap_or(contrast::DEFAULT),
                    fingerprint: None,
                }
            }
        }
    }

    /// Follow content that moved, and drop whatever could not be followed.
    ///
    /// Two things have to hold for a mark to survive. It has to still be on the display,
    /// and where it landed has to still look like what it was pointing at. The second is
    /// the one that matters: a display-wide shift is right for most of the screen and
    /// wrong for any part that did not move with the rest, and the anchor's fingerprint is
    /// the only evidence the daemon has about which of those a given mark is.
    ///
    /// `frame` is the capture the movement was detected from. Without it, and without a
    /// fingerprint recorded when the mark was made, a mark is followed on the display-wide
    /// estimate alone.
    /// The patches of screen that would be correlated for the marks on a display.
    ///
    /// For recording alongside a capture pair, so a scorer tried against that pair offline
    /// is looking at the same regions the daemon was.
    pub fn measured_regions(&self, display: DisplayId) -> Vec<LogicalRect> {
        let Ok(info) = self.display(display) else {
            return Vec::new();
        };
        let state = self.state.lock().expect("state lock");
        state
            .annotations
            .values()
            .filter(|a| a.display_id() == display)
            .map(|a| neighbourhood(a.anchor.screen_rect, info.logical_size))
            .collect()
    }

    /// Follow whatever moved under each mark on a display, one mark at a time.
    ///
    /// The display-wide version of this was wrong, and wrong in a way only a real screen
    /// showed. A scroll happens inside a window. Correlated across the whole display the
    /// answer comes back as *nothing moved*, because the menu bar, the dock, the desktop
    /// and every other window did not, and they outvote the one region that did. Measured
    /// while scrolling a text window: best offset zero, residual 4.6, with a fifth of the
    /// screen's samples changed. Both answers were true at once, and the display-wide one
    /// was no use to a mark sitting inside the window.
    ///
    /// So each mark is measured against its own surroundings. A mark in the scrolling pane
    /// sees the scroll. A mark on a toolbar beside it sees nothing and stays put, which is
    /// correct rather than a special case, and is what the display-wide code needed the
    /// fingerprint to patch up after the fact.
    pub fn follow_movement(&self, display: DisplayId, before: &Frame, after: &Frame) -> Followed {
        let Ok(info) = self.display(display) else {
            return Followed::default();
        };

        let watched: Vec<Annotation> = {
            let state = self.state.lock().expect("state lock");
            state
                .annotations
                .values()
                .filter(|a| a.display_id() == display)
                .cloned()
                .collect()
        };

        let mut moving: Vec<Annotation> = Vec::new();
        let mut doomed: Vec<(AnnotationId, SessionId)> = Vec::new();
        for annotation in watched {
            let around = neighbourhood(annotation.anchor.screen_rect, info.logical_size);
            tracing::debug!(id = %annotation.id, ?around, "measuring what moved under a mark");
            let Some(shift) = crate::signature::shift_within(before, after, around) else {
                // Something changed here that no movement explains, so there is nowhere
                // honest to put this mark. The rest of the display is unaffected.
                doomed.push((annotation.id.clone(), annotation.session.clone()));
                continue;
            };
            let mut candidate = annotation.clone();
            candidate.translate(shift);

            let on_screen = candidate.is_on_screen(info.logical_size);
            let recognised = match candidate.fingerprint() {
                Some(recorded) => Fingerprint::of(after, candidate.anchor.screen_rect)
                    .is_none_or(|now| recorded.matches(&now)),
                None => true,
            };

            // Checked even when nothing was measured to move. A region split between a
            // still part and a scrolling one settles on zero, so the mark most in need of
            // this check is the one that reported no movement.
            if !recognised {
                tracing::debug!(
                    id = %annotation.id,
                    dx = shift.dx,
                    dy = shift.dy,
                    "the content under a mark is not the content it was put on"
                );
                doomed.push((annotation.id.clone(), annotation.session.clone()));
                continue;
            }
            if shift.is_negligible() {
                continue;
            }

            if on_screen {
                tracing::debug!(
                    id = %annotation.id,
                    dx = shift.dx,
                    dy = shift.dy,
                    "following a mark"
                );
                moving.push(candidate);
            } else {
                tracing::debug!(
                    id = %annotation.id,
                    dx = shift.dx,
                    dy = shift.dy,
                    on_screen,
                    recognised,
                    "measured a movement but could not follow it"
                );
                doomed.push((annotation.id.clone(), annotation.session.clone()));
            }
        }

        if moving.is_empty() && doomed.is_empty() {
            return Followed::default();
        }

        {
            let mut state = self.state.lock().expect("state lock");
            for annotation in &moving {
                // Only if it is still there. A clear or an expiry may have got here first.
                if let Some(slot) = state.annotations.get_mut(&annotation.id) {
                    *slot = annotation.clone();
                }
            }
            for (id, _) in &doomed {
                state.annotations.remove(id);
            }
        }
        self.mark_drawn();

        // Redrawn in place. Clearing first would fade the orb out and fly it back in,
        // reading as a new mark rather than one keeping up with the page.
        for annotation in &moving {
            if let Err(e) = self.renderer.draw(annotation) {
                tracing::warn!(id = %annotation.id, error = %e, "renderer refused a redraw");
            }
        }
        for (id, _) in &doomed {
            if let Err(e) = self.renderer.clear(id) {
                tracing::warn!(%id, error = %e, "renderer refused a clear");
            }
        }
        self.announce(doomed.clone(), &InvalidationReason::Scroll);

        Followed {
            moved: moving.into_iter().map(|a| a.id).collect(),
            invalidated: doomed
                .into_iter()
                .map(|(id, _)| Invalidated::one(id, InvalidationReason::Scroll))
                .collect(),
        }
    }
}
