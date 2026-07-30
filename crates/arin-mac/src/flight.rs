//! Cutting one flight across the desktop into one segment per panel.
//!
//! There is a single orb for the whole system, so a flight can cross a screen boundary. A
//! panel cannot draw outside its own window, so the arc is computed once in AppKit's
//! global space and then cut where it leaves each display. Each piece is expressed in the
//! coordinates of the panel that draws it, and the pieces are played back to back.
//!
//! # Why the easing lives here
//!
//! The samples are taken at eased positions along the arc rather than at even ones: close
//! together where the orb should be slow, far apart where it should be fast. Core
//! Animation gives every adjacent pair of values an equal slice of time, so spacing the
//! samples is what paces the flight. A timing function on each segment would instead ease
//! within each one, and a flight drawn in three pieces would accelerate three times.

use crate::display::Screen;
use crate::orb;
use arin_protocol::DisplayId;
use objc2_foundation::NSPoint;
use std::time::Duration;

/// One leg of a flight, drawn by one panel.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    /// The display whose panel draws this leg.
    pub display: DisplayId,
    /// Positions in that panel's layer coordinates.
    pub points: Vec<NSPoint>,
    /// When this leg starts, measured from the start of the whole flight.
    pub start: Duration,
    /// How long this leg takes.
    pub duration: Duration,
}

/// A flight, already cut into the legs that draw it.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub segments: Vec<Segment>,
    /// How long the whole flight takes, including any time over a gap between displays.
    pub total: Duration,
}

/// Cut the arc from `from` to `to` into one segment per display it crosses.
///
/// Both points are in AppKit's global space. Samples that fall on no display at all, which
/// is possible when the arrangement has a gap in it, belong to no segment: nothing can
/// draw there, so the orb waits out that time where it last was rather than being drawn
/// somewhere it is not.
pub fn plan(from: NSPoint, to: NSPoint, screens: &[Screen]) -> Plan {
    let dx = to.x - from.x;
    let dy = to.y - from.y;

    // Every adjacent pair of samples gets an equal slice, so the flight is a whole number
    // of slices rather than the ideal duration. Taking the rounding off the total instead
    // of leaving it stranded at the end keeps the legs adding up exactly, at a cost of
    // under a microsecond on the whole journey.
    let steps = orb::FLIGHT_SAMPLES;
    let slice = orb::flight_duration((dx * dx + dy * dy).sqrt()) / steps as u32;
    let total = slice * steps as u32;

    // Where each sample falls, and on which display.
    let placed: Vec<(NSPoint, Option<usize>)> = (0..=steps)
        .map(|i| {
            let t = orb::ease(i as f64 / steps as f64);
            let point = orb::arc_point(from, to, t);
            let on = screens.iter().position(|s| s.contains(point));
            (point, on)
        })
        .collect();

    let mut segments: Vec<Segment> = Vec::new();
    let mut run: Option<(usize, usize)> = None;

    for index in 0..placed.len() {
        let on = placed[index].1;
        match (run, on) {
            // Extend the current run, or start one.
            (Some((screen, _)), Some(here)) if screen == here => {}
            (previous, Some(here)) => {
                if let Some((screen, start)) = previous {
                    segments.push(leg(screen, start, index - 1, &placed, screens, slice));
                }
                run = Some((here, index));
            }
            (Some((screen, start)), None) => {
                segments.push(leg(screen, start, index - 1, &placed, screens, slice));
                run = None;
            }
            (None, None) => {}
        }
    }
    if let Some((screen, start)) = run {
        segments.push(leg(
            screen,
            start,
            placed.len() - 1,
            &placed,
            screens,
            slice,
        ));
    }

    Plan { segments, total }
}

/// Build one leg from a contiguous run of samples that share a display.
fn leg(
    screen: usize,
    start: usize,
    end: usize,
    placed: &[(NSPoint, Option<usize>)],
    screens: &[Screen],
    slice: Duration,
) -> Segment {
    let on = &screens[screen];
    Segment {
        display: on.info.id,
        points: placed[start..=end]
            .iter()
            .map(|(point, _)| on.global_to_layer(*point))
            .collect(),
        start: slice * start as u32,
        duration: slice * (end - start) as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arin_protocol::DisplayInfo;
    use objc2_foundation::{NSRect, NSSize};

    fn screen(id: u32, x: f64, y: f64, w: f64, h: f64) -> Screen {
        Screen {
            info: DisplayInfo {
                id: DisplayId(id),
                scale: 2.0,
                logical_size: [w, h],
            },
            frame: NSRect::new(NSPoint::new(x, y), NSSize::new(w, h)),
        }
    }

    /// A Retina laptop with a 1x external to its right, which is the arrangement the
    /// display matrix in core treats as the common mixed-DPI case.
    fn laptop_and_external() -> Vec<Screen> {
        vec![
            screen(1, 0.0, 0.0, 1512.0, 982.0),
            screen(2, 1512.0, 0.0, 1920.0, 1080.0),
        ]
    }

    #[test]
    fn a_flight_within_one_display_is_a_single_segment() {
        let screens = laptop_and_external();
        let plan = plan(
            NSPoint::new(100.0, 100.0),
            NSPoint::new(800.0, 700.0),
            &screens,
        );

        assert_eq!(plan.segments.len(), 1);
        let only = &plan.segments[0];
        assert_eq!(only.display, DisplayId(1));
        assert_eq!(only.start, Duration::ZERO);
        assert_eq!(only.duration, plan.total);
        assert_eq!(only.points.len(), orb::FLIGHT_SAMPLES + 1);
    }

    #[test]
    fn a_flight_across_the_boundary_is_cut_into_two() {
        let screens = laptop_and_external();
        let plan = plan(
            NSPoint::new(100.0, 500.0),
            NSPoint::new(2500.0, 500.0),
            &screens,
        );

        assert_eq!(plan.segments.len(), 2);
        assert_eq!(plan.segments[0].display, DisplayId(1));
        assert_eq!(plan.segments[1].display, DisplayId(2));
    }

    #[test]
    fn the_legs_are_continuous_in_time_and_cover_the_whole_flight() {
        let screens = laptop_and_external();
        let plan = plan(
            NSPoint::new(100.0, 500.0),
            NSPoint::new(2500.0, 500.0),
            &screens,
        );

        let first = &plan.segments[0];
        let second = &plan.segments[1];
        assert_eq!(first.start, Duration::ZERO);
        // No gap and no overlap: the second leg begins one slice after the first ended,
        // which is the step that carries the orb over the seam.
        let slice = plan.total / orb::FLIGHT_SAMPLES as u32;
        assert_eq!(second.start, first.start + first.duration + slice);
        assert_eq!(second.start + second.duration, plan.total);
    }

    #[test]
    fn the_seam_is_crossed_without_a_jump() {
        let screens = laptop_and_external();
        let plan = plan(
            NSPoint::new(100.0, 500.0),
            NSPoint::new(2500.0, 500.0),
            &screens,
        );

        // The last point of the first leg and the first of the second are one sample
        // apart on the same arc, so in global space they must be adjacent rather than
        // anywhere near a display's width apart.
        let left = &screens[0];
        let right = &screens[1];
        let leaving = left.layer_to_global(*plan.segments[0].points.last().unwrap());
        let arriving = right.layer_to_global(plan.segments[1].points[0]);

        let step = (arriving.x - leaving.x).abs();
        assert!(step < 60.0, "the orb jumped {step} points across the seam");
        assert!((arriving.y - leaving.y).abs() < 60.0);
    }

    #[test]
    fn the_flight_ends_exactly_on_the_target() {
        let screens = laptop_and_external();
        let target = NSPoint::new(2500.0, 400.0);
        let plan = plan(NSPoint::new(100.0, 500.0), target, &screens);

        let last = plan.segments.last().unwrap();
        let arrived = screens[1].layer_to_global(*last.points.last().unwrap());
        assert_eq!(arrived, target);
    }

    #[test]
    fn every_point_of_a_leg_lands_inside_the_display_that_draws_it() {
        let screens = laptop_and_external();
        let plan = plan(
            NSPoint::new(100.0, 900.0),
            NSPoint::new(3000.0, 200.0),
            &screens,
        );

        for segment in &plan.segments {
            let on = screens
                .iter()
                .find(|s| s.info.id == segment.display)
                .expect("a segment names a display that exists");
            for point in &segment.points {
                assert!(
                    on.contains(on.layer_to_global(*point)),
                    "{point:?} is outside {:?}",
                    on.info.id
                );
            }
        }
    }

    #[test]
    fn a_gap_in_the_arrangement_belongs_to_no_leg() {
        // Two displays with 400 points of nothing between them, which is what an
        // arrangement dragged apart in System Settings looks like.
        let screens = vec![
            screen(1, 0.0, 0.0, 1000.0, 800.0),
            screen(2, 1400.0, 0.0, 1000.0, 800.0),
        ];
        let plan = plan(
            NSPoint::new(100.0, 400.0),
            NSPoint::new(2000.0, 400.0),
            &screens,
        );

        assert_eq!(plan.segments.len(), 2);
        // The time over the gap is still spent, so the legs do not add up to the whole.
        let drawn: Duration = plan.segments.iter().map(|s| s.duration).sum();
        assert!(drawn < plan.total);
        // And nothing is drawn in the gap.
        for segment in &plan.segments {
            let on = screens
                .iter()
                .find(|s| s.info.id == segment.display)
                .unwrap();
            for point in &segment.points {
                assert!(on.contains(on.layer_to_global(*point)));
            }
        }
    }

    #[test]
    fn a_flight_that_bows_off_the_desktop_resumes_on_the_way_back() {
        // The arc bows perpendicular to travel, so a flight along the top edge leaves the
        // desktop in the middle and comes back. The orb should be drawn either side.
        let screens = vec![screen(1, 0.0, 0.0, 1512.0, 982.0)];
        let plan = plan(
            NSPoint::new(80.0, 950.0),
            NSPoint::new(1400.0, 950.0),
            &screens,
        );

        assert!(
            plan.segments.len() >= 2,
            "expected the arc to leave the top edge and return, got {} segment(s)",
            plan.segments.len()
        );
        assert!(plan.segments.iter().all(|s| s.display == DisplayId(1)));
    }

    #[test]
    fn a_flight_starting_off_every_display_still_arrives() {
        // Defensive: the orb should never be somewhere unattached, but if it is, the
        // flight must not silently become empty.
        let screens = laptop_and_external();
        let plan = plan(
            NSPoint::new(-5000.0, -5000.0),
            NSPoint::new(400.0, 400.0),
            &screens,
        );

        let last = plan.segments.last().expect("a leg that arrives");
        assert_eq!(last.display, DisplayId(1));
        assert_eq!(
            screens[0].layer_to_global(*last.points.last().unwrap()),
            NSPoint::new(400.0, 400.0)
        );
    }
}
