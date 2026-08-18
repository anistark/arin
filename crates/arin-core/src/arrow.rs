//! The shape of an arrow, computed once so every renderer draws the same one.
//!
//! An arrow on the wire is two ends and a bow. What reaches a renderer is an ordinary
//! path: the shaft is a quadratic curve sampled into segments, and the head is two barbs
//! stroked back from the tip, retracing over it so one polyline draws the whole thing.
//! Building the geometry here rather than per platform keeps three renderers from having
//! three opinions about what an arrow looks like, the same reason colour is resolved in
//! the daemon and arrives at a renderer already decided.

use arin_protocol::LogicalPoint;

/// The bow used when a client does not ask for one.
///
/// A quarter of the length puts the middle of the shaft an eighth of the length off the
/// straight line, which reads as a drawn arrow rather than a ruled one and still leaves
/// no doubt about where it starts and ends.
pub const NATURAL_BOW: f64 = 0.25;

/// Straight segments the shaft is sampled into.
///
/// Enough that the curve has no visible corners at screen scale. These points never
/// travel on the wire, so the count costs nothing but renderer vertices.
const SEGMENTS: usize = 24;

/// How far the barbs reach back from the tip, as a fraction of the arrow's length.
const HEAD_FRACTION: f64 = 0.15;
/// The shortest barb worth drawing. Below this a head stops reading as one.
const HEAD_MIN: f64 = 10.0;
/// The longest barb worth drawing. Past this a head dominates its own shaft.
const HEAD_MAX: f64 = 26.0;
/// The angle between a barb and the shaft's direction of arrival, in radians.
const HEAD_SPREAD: f64 = 0.46;

/// The arrow as a stroked polyline: the sampled shaft, then the two barbs.
///
/// `bow` is a signed fraction of the straight-line length. Zero is straight, positive
/// bows to the right of travel, negative to the left, in screen coordinates where y
/// grows downward. The head sits at `to`.
///
/// Coincident ends are the daemon's to refuse. Handed them anyway, this returns the two
/// points bare rather than dividing by zero.
pub fn path(from: LogicalPoint, to: LogicalPoint, bow: f64) -> Vec<LogicalPoint> {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let length = (dx * dx + dy * dy).sqrt();
    if length == 0.0 {
        return vec![from, to];
    }

    // The control point sits off the chord's midpoint, perpendicular to it. With y
    // growing downward, (-dy, dx) is the right-hand side of travel.
    let control_x = (from.x + to.x) / 2.0 + (-dy / length) * bow * length;
    let control_y = (from.y + to.y) / 2.0 + (dx / length) * bow * length;

    let mut points = Vec::with_capacity(SEGMENTS + 4);
    for i in 0..=SEGMENTS {
        let t = i as f64 / SEGMENTS as f64;
        let u = 1.0 - t;
        points.push(LogicalPoint::new(
            u * u * from.x + 2.0 * u * t * control_x + t * t * to.x,
            u * u * from.y + 2.0 * u * t * control_y + t * t * to.y,
        ));
    }

    // The barbs lean back from the tip against the direction of arrival, which for a
    // quadratic is the line from the control point to the end. Retracing tip, barb,
    // tip, barb overdraws one edge of the head exactly, so it costs nothing visible.
    let arrival = (to.y - control_y).atan2(to.x - control_x);
    let reach = (length * HEAD_FRACTION)
        .clamp(HEAD_MIN, HEAD_MAX)
        .min(length / 2.0);
    let barb = |spread: f64| {
        let angle = arrival + std::f64::consts::PI + spread;
        LogicalPoint::new(to.x + reach * angle.cos(), to.y + reach * angle.sin())
    };
    points.push(barb(HEAD_SPREAD));
    points.push(to);
    points.push(barb(-HEAD_SPREAD));
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    const FROM: LogicalPoint = LogicalPoint::new(100.0, 500.0);
    const TO: LogicalPoint = LogicalPoint::new(500.0, 100.0);

    /// The shaft's samples, without the head.
    fn shaft(points: &[LogicalPoint]) -> &[LogicalPoint] {
        &points[..points.len() - 3]
    }

    fn distance(a: LogicalPoint, b: LogicalPoint) -> f64 {
        ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
    }

    /// Signed distance from the chord, positive on the right of travel.
    fn off_chord(p: LogicalPoint) -> f64 {
        let (dx, dy) = (TO.x - FROM.x, TO.y - FROM.y);
        let length = (dx * dx + dy * dy).sqrt();
        ((p.y - FROM.y) * dx - (p.x - FROM.x) * dy) / length
    }

    #[test]
    fn the_shaft_runs_from_one_end_to_the_other() {
        let points = path(FROM, TO, 0.3);
        assert_eq!(points[0], FROM);
        assert_eq!(*shaft(&points).last().unwrap(), TO);
        assert!(points.iter().all(|p| p.is_finite()));
    }

    #[test]
    fn a_zero_bow_is_a_straight_line() {
        let points = path(FROM, TO, 0.0);
        for p in shaft(&points) {
            assert!(off_chord(*p).abs() < 1e-9, "{p:?} left the chord");
        }
    }

    #[test]
    fn the_bow_bulges_by_half_its_fraction_at_the_middle() {
        let bow = 0.25;
        let points = path(FROM, TO, bow);
        let middle = shaft(&points)[SEGMENTS / 2];
        let length = distance(FROM, TO);
        let expected = bow * length / 2.0;
        assert!(
            (off_chord(middle).abs() - expected).abs() < 1e-6,
            "middle sits {} off the chord, expected {expected}",
            off_chord(middle)
        );
    }

    #[test]
    fn the_bow_takes_its_sign_seriously() {
        let bowed = path(FROM, TO, 0.3);
        let mirrored = path(FROM, TO, -0.3);
        let side = off_chord(shaft(&bowed)[SEGMENTS / 2]);
        let other = off_chord(shaft(&mirrored)[SEGMENTS / 2]);
        assert!(side > 0.0, "positive bow should sit right of travel");
        assert!(other < 0.0, "negative bow should sit left of travel");
    }

    #[test]
    fn the_head_is_two_barbs_retracing_the_tip() {
        let points = path(FROM, TO, 0.25);
        let tail = &points[points.len() - 3..];
        assert_eq!(tail[1], TO, "the tip is retraced between the barbs");
        let reach = distance(TO, tail[0]);
        assert!((reach - distance(TO, tail[2])).abs() < 1e-9, "barbs match");
        assert!((HEAD_MIN..=HEAD_MAX).contains(&reach));
    }

    #[test]
    fn a_short_arrow_keeps_its_head_smaller_than_itself() {
        let near = LogicalPoint::new(106.0, 500.0);
        let points = path(FROM, near, 0.0);
        let tip = points[points.len() - 2];
        let reach = distance(tip, points[points.len() - 1]);
        assert!(reach <= distance(FROM, near) / 2.0 + 1e-9);
    }

    /// The barbs open against the direction of arrival, so on a bowed arrow the head
    /// follows the curve in rather than sitting on the straight line.
    #[test]
    fn the_head_leans_with_the_curve() {
        let straight = path(FROM, TO, 0.0);
        let bowed = path(FROM, TO, 0.5);
        let straight_barb = straight[straight.len() - 3];
        let bowed_barb = bowed[bowed.len() - 3];
        assert!(
            distance(straight_barb, bowed_barb) > 1.0,
            "a bow should move the head's barbs"
        );
    }

    #[test]
    fn coincident_ends_do_not_divide_by_zero() {
        let points = path(FROM, FROM, 0.25);
        assert!(points.iter().all(|p| p.is_finite()));
    }
}
