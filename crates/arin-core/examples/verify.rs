//! Grades the whole measure, move and verify pipeline against a recorded corpus.
//!
//! `calibrate` asks which offset scorer finds the right answer. This asks the question
//! after that one: given the answer, does the mark land where it should, and does the
//! anchor fingerprint recognise it when it does. Both replay through
//! [`arin_core::record::replay`], so neither can drift from what the daemon wrote.
//!
//! ```text
//! cargo run --release --example verify -- /path/to/corpus
//! ```

use arin_core::fingerprint::Fingerprint;
use arin_core::record::{Recording, replay};
use arin_core::signature::shift_within;
use arin_core::traits::{Frame, luminance};
use arin_protocol::LogicalRect;
use std::path::Path;

/// The margin `daemon::neighbourhood` puts around an anchor, in logical points.
const CONTEXT: f64 = 120.0;

/// Where a logical rectangle lands in a frame's pixels.
fn to_pixels(frame: &Frame, rect: LogicalRect) -> (usize, usize, usize, usize) {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let sx = |v: f64| ((v / frame.logical_size[0]) * w as f64).round();
    let sy = |v: f64| ((v / frame.logical_size[1]) * h as f64).round();
    let x0 = sx(rect.x).clamp(0.0, w as f64 - 1.0) as usize;
    let y0 = sy(rect.y).clamp(0.0, h as f64 - 1.0) as usize;
    let x1 = sx(rect.x + rect.width).clamp(1.0, w as f64) as usize;
    let y1 = sy(rect.y + rect.height).clamp(1.0, h as f64) as usize;
    (x0.min(x1 - 1), y0.min(y1 - 1), x1, y1)
}

/// Brightness of one pixel, through the daemon's own function rather than a copy.
///
/// A harness that samples differently from the daemon predicts the wrong thing.
fn at(frame: &Frame, x: usize, y: usize) -> Option<f64> {
    let idx = (y * frame.width as usize + x) * 4;
    frame
        .pixels
        .get(idx..idx + 4)
        .map(|px| f64::from(luminance(px)))
}

/// The true movement of a region, by brute force over whole pixels.
///
/// The ground truth everything else is graded against: every pixel of the region compared
/// at every candidate offset, with no profiles or shortcuts in the way.
fn truth(before: &Frame, after: &Frame, area: (usize, usize, usize, usize)) -> Option<(i32, i32)> {
    let (x0, y0, x1, y1) = area;
    let mut best: Option<(i32, i32, f64)> = None;
    for dy in -72i32..=72 {
        for dx in -24i32..=24 {
            let (mut total, mut n) = (0.0, 0.0);
            let mut y = y0 as i32;
            while y < y1 as i32 {
                let sy = y - dy;
                if sy >= 0 && (sy as usize) < after.height as usize {
                    let mut x = x0 as i32;
                    while x < x1 as i32 {
                        let sx = x - dx;
                        if sx >= 0
                            && (sx as usize) < after.width as usize
                            && let (Some(a), Some(b)) = (
                                at(after, x as usize, y as usize),
                                at(before, sx as usize, sy as usize),
                            )
                        {
                            total += (a - b).abs();
                            n += 1.0;
                        }
                        x += 3;
                    }
                }
                y += 2;
            }
            if n < 64.0 {
                continue;
            }
            let score = total / n;
            if best.is_none_or(|(_, _, b)| score < b) {
                best = Some((dx, dy, score));
            }
        }
    }
    best.map(|(dx, dy, _)| (dx, dy))
}

/// The fingerprint as it was first written: one pixel per cell.
fn point_sample(frame: &Frame, rect: LogicalRect, grid: usize) -> Vec<f64> {
    let mut out = vec![0.0; grid * grid];
    for row in 0..grid {
        for col in 0..grid {
            let x = rect.x + rect.width * (col as f64 * 2.0 + 1.0) / (grid as f64 * 2.0);
            let y = rect.y + rect.height * (row as f64 * 2.0 + 1.0) / (grid as f64 * 2.0);
            let (px, py, ..) = to_pixels(frame, LogicalRect::new(x, y, 1.0, 1.0));
            out[row * grid + col] = at(frame, px, py).unwrap_or(0.0);
        }
    }
    out
}

/// What the fingerprint does now: the mean of every pixel in each cell.
fn cell_mean(frame: &Frame, rect: LogicalRect, grid: usize) -> Vec<f64> {
    let mut out = vec![0.0; grid * grid];
    for row in 0..grid {
        for col in 0..grid {
            let cell = LogicalRect::new(
                rect.x + rect.width * col as f64 / grid as f64,
                rect.y + rect.height * row as f64 / grid as f64,
                rect.width / grid as f64,
                rect.height / grid as f64,
            );
            let (x0, y0, x1, y1) = to_pixels(frame, cell);
            let (mut total, mut n) = (0.0, 0.0);
            for y in y0..y1 {
                for x in x0..x1 {
                    if let Some(v) = at(frame, x, y) {
                        total += v;
                        n += 1.0;
                    }
                }
            }
            out[row * grid + col] = if n > 0.0 { total / n } else { 0.0 };
        }
    }
    out
}

fn agreement(a: &[f64], b: &[f64], tolerance: f64) -> f64 {
    let agreed = a
        .iter()
        .zip(b)
        .filter(|(x, y)| (*x - *y).abs() <= tolerance)
        .count();
    agreed as f64 / a.len() as f64
}

/// One graded recording: where the mark was, where it should have gone, and whether it did.
struct Case<'a> {
    recording: &'a Recording,
    region: LogicalRect,
    anchor: LogicalRect,
    followed: LogicalRect,
    moved: bool,
}

/// The whole pipeline the daemon runs: measure, move, then verify.
fn end_to_end(cases: &[Case]) {
    let (mut right, mut wrong, mut refused) = (0, 0, 0);
    let (mut still_held, mut still_lost) = (0, 0);
    for case in cases {
        let (before, after) = (&case.recording.before, &case.recording.after);
        let truth = (
            case.followed.x - case.anchor.x,
            case.followed.y - case.anchor.y,
        );
        let Some(shift) = shift_within(before, after, case.region) else {
            if case.moved {
                refused += 1;
                println!("  {} refused a real move of {truth:?}", case.recording.name);
            } else {
                still_lost += 1;
            }
            continue;
        };
        // Within a few points of the truth is a mark that landed on its content.
        let close = (shift.dx - truth.0).abs() <= 12.0 && (shift.dy - truth.1).abs() <= 12.0;
        if case.moved {
            if close {
                right += 1;
            } else {
                wrong += 1;
                println!(
                    "  {} measured dx={:.0} dy={:.0}, truth dx={:.0} dy={:.0}",
                    case.recording.name, shift.dx, shift.dy, truth.0, truth.1
                );
            }
        } else if close {
            still_held += 1;
        } else {
            still_lost += 1;
            println!(
                "  {} invented dx={:.0} dy={:.0} on a still screen",
                case.recording.name, shift.dx, shift.dy
            );
        }

        // And would the fingerprint accept where it landed?
        if case.moved && close {
            let landed = LogicalRect::new(
                case.anchor.x + shift.dx,
                case.anchor.y + shift.dy,
                case.anchor.width,
                case.anchor.height,
            );
            if let (Some(was), Some(now)) = (
                Fingerprint::of(before, case.anchor),
                Fingerprint::of(after, landed),
            ) && !was.matches(&now)
            {
                println!(
                    "  {} measured correctly but verification refused it",
                    case.recording.name
                );
            }
        }
    }
    let moved = cases.iter().filter(|c| c.moved).count();
    let still = cases.len() - moved;
    println!(
        "\nestimator: {right}/{moved} moves right, {wrong} wrong, {refused} refused; \
         still screens {still_held}/{still} held, {still_lost} lost\n"
    );
}

type Sampler = fn(&Frame, LogicalRect, usize) -> Vec<f64>;

/// Every candidate fingerprint rule, graded on the two things it has to do at once.
fn sweep(cases: &[Case]) {
    let moved = cases.iter().filter(|c| c.moved).count();
    let still = cases.len() - moved;
    println!(
        "{:>6} {:>4} {:>4} {:>5}   {:>10} {:>10} {:>10}",
        "sample", "grid", "tol", "bar", "followed", "caught", "still"
    );

    let samplers: [(&str, Sampler); 2] = [("point", point_sample), ("mean", cell_mean)];
    for (name, sampler) in samplers {
        for grid in [4usize, 6, 8] {
            for tolerance in [8.0f64, 12.0, 16.0, 24.0] {
                for bar in [0.5f64, 0.625, 0.75] {
                    let (mut kept, mut caught, mut held) = (0, 0, 0);
                    for case in cases {
                        let (before, after) = (&case.recording.before, &case.recording.after);
                        let was = sampler(before, case.anchor, grid);
                        if case.moved {
                            if agreement(&was, &sampler(after, case.followed, grid), tolerance)
                                >= bar
                            {
                                kept += 1;
                            }
                            if agreement(&was, &sampler(after, case.anchor, grid), tolerance) < bar
                            {
                                caught += 1;
                            }
                        } else if agreement(&was, &sampler(after, case.anchor, grid), tolerance)
                            >= bar
                        {
                            held += 1;
                        }
                    }
                    println!(
                        "{name:>6} {grid:>4} {tolerance:>4} {bar:>5}   \
                         {kept:>4}/{moved:<5} {caught:>4}/{moved:<5} {held:>4}/{still:<5}"
                    );
                }
            }
        }
    }
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .expect("pass the corpus directory as the first argument");
    let recordings = replay(Path::new(&dir));

    // Ground truth once, then every rule graded against it.
    let mut cases = Vec::new();
    for recording in &recordings {
        let Some(region) = recording.regions.first().copied() else {
            continue;
        };
        let anchor = LogicalRect::new(
            region.x + CONTEXT,
            region.y + CONTEXT,
            (region.width - CONTEXT * 2.0).max(8.0),
            (region.height - CONTEXT * 2.0).max(8.0),
        );
        let area = to_pixels(&recording.after, region);
        let Some((dx, dy)) = truth(&recording.before, &recording.after, area) else {
            continue;
        };
        let px = recording.after.logical_size[0] / f64::from(recording.after.width);
        let py = recording.after.logical_size[1] / f64::from(recording.after.height);
        cases.push(Case {
            recording,
            region,
            anchor,
            followed: LogicalRect::new(
                anchor.x + f64::from(dx) * px,
                anchor.y + f64::from(dy) * py,
                anchor.width,
                anchor.height,
            ),
            moved: dx.abs() > 1 || dy.abs() > 1,
        });
    }

    let moved = cases.iter().filter(|c| c.moved).count();
    println!(
        "{} recordings: {moved} moved, {} still\n",
        recordings.len(),
        cases.len() - moved
    );
    end_to_end(&cases);
    sweep(&cases);
}
