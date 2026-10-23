//! When a mark's ink goes down, and how fast the pen moves along it.
//!
//! A mark is not placed whole. Its strokes are drawn one after another, the way a hand
//! draws them, with the orb riding the tip. The timeline is computed here, once, so every
//! renderer plays the same one and the daemon knows when the ink will be finished without
//! asking. That finish time is what `ttl` counts from.
//!
//! The pacing is carried by where the samples sit, as it is for the orb's flight. Each
//! stroke is sampled at even slices of its duration, and each sample says how far along
//! the stroke the pen has got by then. Close together at the ends and far apart in the
//! middle is what makes a stroke start slowly, run, and land. A renderer gives every
//! adjacent pair of samples an equal slice of time and never applies an easing of its own.

use arin_protocol::LogicalPoint;
use std::time::{Duration, Instant};

/// How long a lone mark of no length takes. Also the floor for any stroke with ink in it.
const SHORTEST_STROKE: Duration = Duration::from_millis(40);

/// Samples per second of pen movement.
///
/// The display's own rate is the most a renderer could use. Below this the stroke's
/// acceleration starts to show as steps.
const SAMPLE_RATE: f64 = 60.0;

/// The pause between one stroke ending and the next starting, while the pen is in the air.
pub const PEN_LIFT: Duration = Duration::from_millis(18);

/// The longest a mark may wait for the orb before it is drawn without it.
///
/// There is one orb, so marks that arrive together queue for it. Past this the screen is
/// being held up by a burst of marks, and showing them at once is the better failure.
//
// NOTE: provisional, settled without seeing real agents produce bursts of marks. Revisit
// the 3 s limit, and drawing late arrivals at once, when there is evidence either way.
pub const QUEUE_LIMIT: Duration = Duration::from_secs(3);

/// How long a point's orb is given to land before it writes the caption beside itself.
///
/// The flight's own length is the renderer's business and depends on where the orb was,
/// which core does not know. This covers a flight across one display, and a longer one
/// is cut short by the pen taking the orb.
pub const LANDING: Duration = Duration::from_millis(550);

/// The whole timeline of one mark.
#[derive(Debug, Clone, PartialEq)]
pub struct Reveal {
    /// When the pen first touches down, after any wait for the orb.
    pub begins: Instant,
    /// Every stroke, in the order it is drawn, timed from [`Self::begins`].
    pub strokes: Vec<StrokeReveal>,
}

/// One stroke's share of a timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct StrokeReveal {
    /// When the pen touches down for this stroke, from the start of the reveal.
    pub start: Duration,
    /// How long the stroke takes.
    pub duration: Duration,
    /// Where the pen is at even slices of [`Self::duration`], first and last included.
    pub samples: Vec<RevealSample>,
}

/// Where the pen is at one moment of a stroke.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RevealSample {
    /// The tip, in logical points.
    pub at: LogicalPoint,
    /// How much of the stroke's length is drawn, from `0.0` to `1.0`.
    pub reach: f64,
}

impl Reveal {
    /// How long the pen is down and in the air, from the first touch to the last lift.
    pub fn duration(&self) -> Duration {
        self.strokes
            .iter()
            .map(|s| s.start + s.duration)
            .max()
            .unwrap_or_default()
    }

    /// When the ink is finished.
    pub fn settles_at(&self) -> Instant {
        self.begins + self.duration()
    }

    /// Move every sample by a logical offset, alongside the ink it describes.
    pub fn translate(&mut self, dx: f64, dy: f64) {
        for sample in self.strokes.iter_mut().flat_map(|s| s.samples.iter_mut()) {
            sample.at.x += dx;
            sample.at.y += dy;
        }
    }
}

/// How long a drawn mark takes, from the length of ink in it.
///
/// Long enough to follow and short enough not to hold anything up. A circled word takes
/// about half a second and an arrow across the screen a little longer.
pub fn mark_duration(length: f64) -> Duration {
    Duration::from_secs_f64((0.2 + length / 2000.0).min(0.8))
}

/// Time a run of strokes into `budget`, sharing it out by length.
///
/// The budget covers the pen lifts between strokes as well as the strokes themselves, so
/// a mark asked to take one second takes one second however many strokes it has.
pub fn plan(strokes: &[&[LogicalPoint]], budget: Duration) -> Vec<StrokeReveal> {
    let lengths: Vec<f64> = strokes.iter().map(|s| length(s)).collect();
    let whole: f64 = lengths.iter().sum();
    let lifts = PEN_LIFT * strokes.len().saturating_sub(1) as u32;
    let writing = budget.saturating_sub(lifts).max(SHORTEST_STROKE);

    let mut start = Duration::ZERO;
    let mut out = Vec::with_capacity(strokes.len());
    for (points, &stroke_length) in strokes.iter().zip(&lengths) {
        let share = if whole > 0.0 {
            writing.mul_f64(stroke_length / whole)
        } else {
            writing / strokes.len().max(1) as u32
        };
        let duration = share.max(SHORTEST_STROKE);
        out.push(StrokeReveal {
            start,
            duration,
            samples: samples(points, duration),
        });
        start += duration + PEN_LIFT;
    }
    out
}

/// One run of strokes followed by another, a pen lift apart.
pub fn then(mut first: Vec<StrokeReveal>, second: Vec<StrokeReveal>) -> Vec<StrokeReveal> {
    let end = first
        .iter()
        .map(|s| s.start + s.duration)
        .max()
        .map_or(Duration::ZERO, |end| end + PEN_LIFT);
    first.extend(delayed(second, end));
    first
}

/// A run of strokes started later.
pub fn delayed(strokes: Vec<StrokeReveal>, by: Duration) -> Vec<StrokeReveal> {
    strokes
        .into_iter()
        .map(|s| StrokeReveal {
            start: s.start + by,
            ..s
        })
        .collect()
}

/// The total length of a polyline.
pub fn length(points: &[LogicalPoint]) -> f64 {
    points
        .windows(2)
        .map(|pair| distance(pair[0], pair[1]))
        .sum()
}

/// Sample a stroke at even slices of its duration.
fn samples(points: &[LogicalPoint], duration: Duration) -> Vec<RevealSample> {
    let Some(&first) = points.first() else {
        return Vec::new();
    };
    let slices = ((duration.as_secs_f64() * SAMPLE_RATE).ceil() as usize).max(2);
    let total = length(points);
    (0..=slices)
        .map(|i| {
            let reach = ease(i as f64 / slices as f64);
            let at = if total > 0.0 {
                along(points, reach * total)
            } else {
                first
            };
            RevealSample { at, reach }
        })
        .collect()
}

/// How far along a stroke the pen is, `u` of the way through its time.
///
/// A half cosine: slow away from the start, fastest in the middle, slow into the end,
/// which is the bell shaped speed a hand has over a single stroke.
pub fn ease(u: f64) -> f64 {
    let u = u.clamp(0.0, 1.0);
    (1.0 - (std::f64::consts::PI * u).cos()) / 2.0
}

/// The point `distance` along a polyline, clamped to its ends.
fn along(points: &[LogicalPoint], distance_along: f64) -> LogicalPoint {
    let mut left = distance_along.max(0.0);
    for pair in points.windows(2) {
        let step = distance(pair[0], pair[1]);
        if step > 0.0 && left <= step {
            let t = left / step;
            return LogicalPoint::new(
                pair[0].x + (pair[1].x - pair[0].x) * t,
                pair[0].y + (pair[1].y - pair[0].y) * t,
            );
        }
        left -= step;
    }
    *points.last().expect("checked non-empty by the caller")
}

fn distance(a: LogicalPoint, b: LogicalPoint) -> f64 {
    ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(from: (f64, f64), to: (f64, f64)) -> Vec<LogicalPoint> {
        vec![
            LogicalPoint::new(from.0, from.1),
            LogicalPoint::new(to.0, to.1),
        ]
    }

    #[test]
    fn a_stroke_starts_and_ends_on_its_own_ends() {
        let stroke = line((10.0, 20.0), (110.0, 20.0));
        let timed = plan(&[&stroke], Duration::from_millis(400));
        let samples = &timed[0].samples;
        assert_eq!(samples.first().unwrap().at, stroke[0]);
        assert_eq!(samples.last().unwrap().at, stroke[1]);
        assert_eq!(samples.first().unwrap().reach, 0.0);
        assert!((samples.last().unwrap().reach - 1.0).abs() < 1e-9);
    }

    #[test]
    fn the_pen_is_fastest_in_the_middle_of_a_stroke() {
        let stroke = line((0.0, 0.0), (300.0, 0.0));
        let samples = &plan(&[&stroke], Duration::from_millis(500))[0].samples;
        let steps: Vec<f64> = samples
            .windows(2)
            .map(|p| p[1].reach - p[0].reach)
            .collect();
        let middle = steps[steps.len() / 2];
        assert!(steps[0] < middle && *steps.last().unwrap() < middle);
    }

    #[test]
    fn the_budget_covers_strokes_and_lifts_together() {
        let a = line((0.0, 0.0), (100.0, 0.0));
        let b = line((0.0, 10.0), (300.0, 10.0));
        let timed = plan(&[&a, &b], Duration::from_millis(600));
        let end = timed[1].start + timed[1].duration;
        assert!(
            end.abs_diff(Duration::from_millis(600)) < Duration::from_millis(2),
            "{end:?}"
        );
        assert_eq!(timed[1].start, timed[0].duration + PEN_LIFT);
    }

    #[test]
    fn a_longer_stroke_gets_more_of_the_budget() {
        let short = line((0.0, 0.0), (50.0, 0.0));
        let long = line((0.0, 0.0), (250.0, 0.0));
        let timed = plan(&[&short, &long], Duration::from_millis(800));
        assert!(timed[1].duration > timed[0].duration * 4);
    }

    #[test]
    fn a_dot_still_takes_a_moment() {
        let dot = vec![LogicalPoint::new(5.0, 5.0); 2];
        let timed = plan(&[&dot], Duration::ZERO);
        assert_eq!(timed[0].duration, SHORTEST_STROKE);
        assert!(timed[0].samples.iter().all(|s| s.at == dot[0]));
    }

    #[test]
    fn a_second_run_starts_a_lift_after_the_first_ends() {
        let a = line((0.0, 0.0), (100.0, 0.0));
        let b = line((0.0, 10.0), (100.0, 10.0));
        let first = plan(&[&a], Duration::from_millis(200));
        let first_end = first[0].start + first[0].duration;
        let both = then(first, plan(&[&b], Duration::from_millis(100)));
        assert_eq!(both[1].start, first_end + PEN_LIFT);
    }

    #[test]
    fn marks_take_longer_with_length_but_not_forever() {
        assert!(mark_duration(600.0) > mark_duration(100.0));
        assert_eq!(mark_duration(1_000_000.0), Duration::from_millis(800));
    }

    #[test]
    fn moving_a_reveal_moves_every_sample() {
        let stroke = line((0.0, 0.0), (100.0, 0.0));
        let mut reveal = Reveal {
            begins: Instant::now(),
            strokes: plan(&[&stroke], Duration::from_millis(200)),
        };
        reveal.translate(5.0, -3.0);
        let first = reveal.strokes[0].samples[0].at;
        assert_eq!(first, LogicalPoint::new(5.0, -3.0));
    }
}
