//! Does the anchor fingerprint recognise content it followed correctly?
//!
//! The estimator measures a movement and the daemon then checks that the mark landed on
//! what it started on. Live, that check was refusing nearly every correct move. This
//! replays the recorded corpus to find out whether the check can tell a followed mark
//! from a left behind one at the resolution the daemon actually captures at.
//!
//! Run with the corpus directory as the only argument:
//!
//! ```text
//! cargo run --example verify -- /path/to/corpus
//! ```

use arin_core::fingerprint::Fingerprint;
use arin_core::signature::shift_within;
use arin_core::traits::Frame;
use arin_protocol::{DisplayId, LogicalRect};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The margin `daemon::neighbourhood` puts around an anchor, in logical points.
const CONTEXT: f64 = 120.0;

struct Pair {
    name: String,
    width: usize,
    height: usize,
    logical: [f64; 2],
    region: [f64; 4],
    before: Vec<u8>,
    after: Vec<u8>,
}

fn luminance(bgra: &[u8]) -> f64 {
    let b = f64::from(bgra[0]);
    let g = f64::from(bgra[1]);
    let r = f64::from(bgra[2]);
    (r * 77.0 + g * 150.0 + b * 29.0) / 256.0
}

fn number(json: &str, key: &str) -> Option<f64> {
    let at = json.find(&format!("\"{key}\":"))? + key.len() + 3;
    let rest = &json[at..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn numbers(json: &str, key: &str) -> Vec<f64> {
    let Some(at) = json.find(&format!("\"{key}\":")) else {
        return Vec::new();
    };
    let rest = &json[at..];
    let Some(open) = rest.find('[') else {
        return Vec::new();
    };
    let mut depth = 0;
    let mut end = open;
    for (i, c) in rest[open..].char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    end = open + i;
                    break;
                }
            }
            _ => {}
        }
    }
    rest[open..=end]
        .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e'))
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect()
}

fn load(dir: &Path) -> Vec<Pair> {
    let mut pairs = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        eprintln!("cannot read {}", dir.display());
        return pairs;
    };
    let mut manifests: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    manifests.sort();

    for manifest in manifests {
        let Ok(json) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        let stem = manifest.file_stem().unwrap().to_string_lossy().to_string();
        let (Ok(before), Ok(after)) = (
            std::fs::read(dir.join(format!("{stem}-before.bgra"))),
            std::fs::read(dir.join(format!("{stem}-after.bgra"))),
        ) else {
            continue;
        };
        let logical = numbers(&json, "logical_size");
        let region = numbers(&json, "regions");
        if logical.len() < 2 || region.len() < 4 {
            continue;
        }
        pairs.push(Pair {
            name: stem,
            width: number(&json, "width").unwrap_or(0.0) as usize,
            height: number(&json, "height").unwrap_or(0.0) as usize,
            logical: [logical[0], logical[1]],
            region: [region[0], region[1], region[2], region[3]],
            before,
            after,
        });
    }
    pairs
}

/// Where a logical rectangle lands in the recorded pixels.
fn to_pixels(pair: &Pair, rect: [f64; 4]) -> (usize, usize, usize, usize) {
    let sx = |v: f64| ((v / pair.logical[0]) * pair.width as f64).round();
    let sy = |v: f64| ((v / pair.logical[1]) * pair.height as f64).round();
    let x0 = sx(rect[0]).clamp(0.0, pair.width as f64 - 1.0) as usize;
    let y0 = sy(rect[1]).clamp(0.0, pair.height as f64 - 1.0) as usize;
    let x1 = sx(rect[0] + rect[2]).clamp(1.0, pair.width as f64) as usize;
    let y1 = sy(rect[1] + rect[3]).clamp(1.0, pair.height as f64) as usize;
    (x0.min(x1 - 1), y0.min(y1 - 1), x1, y1)
}

/// The true movement of the region, by brute force over whole pixels.
///
/// This is the ground truth the estimator is graded against: every pixel of the region
/// compared at every candidate offset, with no profiles or shortcuts in the way.
fn truth(pair: &Pair, area: (usize, usize, usize, usize)) -> Option<(i32, i32, f64)> {
    let (x0, y0, x1, y1) = area;
    let mut best: Option<(i32, i32, f64)> = None;
    for dy in -72i32..=72 {
        for dx in -24i32..=24 {
            let mut total = 0.0;
            let mut n = 0.0;
            let mut y = y0 as i32;
            while y < y1 as i32 {
                let sy = y - dy;
                if sy >= 0 && (sy as usize) < pair.height {
                    let mut x = x0 as i32;
                    while x < x1 as i32 {
                        let sx = x - dx;
                        if sx >= 0 && (sx as usize) < pair.width {
                            let i = (y as usize * pair.width + x as usize) * 4;
                            let j = (sy as usize * pair.width + sx as usize) * 4;
                            if i + 4 <= pair.after.len() && j + 4 <= pair.before.len() {
                                total += (luminance(&pair.after[i..i + 4])
                                    - luminance(&pair.before[j..j + 4]))
                                .abs();
                                n += 1.0;
                            }
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
    best
}

/// The fingerprint as it is written today: one pixel per cell.
fn point_sample(pixels: &[u8], pair: &Pair, rect: [f64; 4], grid: usize) -> Vec<f64> {
    let mut out = vec![0.0; grid * grid];
    for row in 0..grid {
        for col in 0..grid {
            let at_x = rect[0] + rect[2] * (col as f64 * 2.0 + 1.0) / (grid as f64 * 2.0);
            let at_y = rect[1] + rect[3] * (row as f64 * 2.0 + 1.0) / (grid as f64 * 2.0);
            let x = ((at_x / pair.logical[0]) * pair.width as f64)
                .round()
                .clamp(0.0, pair.width as f64 - 1.0) as usize;
            let y = ((at_y / pair.logical[1]) * pair.height as f64)
                .round()
                .clamp(0.0, pair.height as f64 - 1.0) as usize;
            let idx = (y * pair.width + x) * 4;
            if let Some(px) = pixels.get(idx..idx + 4) {
                out[row * grid + col] = luminance(px);
            }
        }
    }
    out
}

/// The proposal: the mean of every pixel in each cell rather than one pixel from it.
fn cell_mean(pixels: &[u8], pair: &Pair, rect: [f64; 4], grid: usize) -> Vec<f64> {
    let mut out = vec![0.0; grid * grid];
    for row in 0..grid {
        for col in 0..grid {
            let cx0 = rect[0] + rect[2] * col as f64 / grid as f64;
            let cx1 = rect[0] + rect[2] * (col + 1) as f64 / grid as f64;
            let cy0 = rect[1] + rect[3] * row as f64 / grid as f64;
            let cy1 = rect[1] + rect[3] * (row + 1) as f64 / grid as f64;
            let (x0, y0, x1, y1) = to_pixels(pair, [cx0, cy0, cx1 - cx0, cy1 - cy0]);
            let mut total = 0.0;
            let mut n = 0.0;
            for y in y0..y1 {
                for x in x0..x1 {
                    let idx = (y * pair.width + x) * 4;
                    if let Some(px) = pixels.get(idx..idx + 4) {
                        total += luminance(px);
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

type Sampler = fn(&[u8], &Pair, [f64; 4], usize) -> Vec<f64>;

/// Rebuild a recorded capture as the daemon's own frame type.
fn as_frame(pair: &Pair, pixels: &[u8]) -> Frame {
    Frame {
        display: DisplayId(1),
        scale: pair.width as f64 / pair.logical[0],
        logical_size: pair.logical,
        width: pair.width as u32,
        height: pair.height as u32,
        pixels: Arc::from(pixels),
    }
}

/// The whole pipeline the daemon runs: measure, move, then verify.
fn end_to_end(cases: &[(&Pair, [f64; 4], [f64; 4], bool)]) {
    let (mut right, mut wrong, mut refused) = (0, 0, 0);
    let (mut still_kept, mut still_lost) = (0, 0);
    for (pair, anchor, followed, real) in cases {
        let before = as_frame(pair, &pair.before);
        let after = as_frame(pair, &pair.after);
        let region = LogicalRect::new(
            pair.region[0],
            pair.region[1],
            pair.region[2],
            pair.region[3],
        );
        let truth = (followed[0] - anchor[0], followed[1] - anchor[1]);
        let Some(shift) = shift_within(&before, &after, region) else {
            if *real {
                refused += 1;
                println!("  {} refused a real move of {truth:?}", pair.name);
            } else {
                still_lost += 1;
                println!("  {} refused a still screen", pair.name);
            }
            continue;
        };
        // Within a few points of the truth is a mark that landed on its content.
        let close = (shift.dx - truth.0).abs() <= 12.0 && (shift.dy - truth.1).abs() <= 12.0;
        if *real {
            if close {
                right += 1;
            } else {
                wrong += 1;
                println!(
                    "  {} measured dx={:.0} dy={:.0}, truth dx={:.0} dy={:.0}",
                    pair.name, shift.dx, shift.dy, truth.0, truth.1
                );
            }
        } else if close {
            still_kept += 1;
        } else {
            still_lost += 1;
            println!(
                "  {} invented dx={:.0} dy={:.0} on a still screen",
                pair.name, shift.dx, shift.dy
            );
        }

        // And would the fingerprint accept where it landed?
        if *real && close {
            let landed = LogicalRect::new(
                anchor[0] + shift.dx,
                anchor[1] + shift.dy,
                anchor[2],
                anchor[3],
            );
            let was = Fingerprint::of(
                &before,
                LogicalRect::new(anchor[0], anchor[1], anchor[2], anchor[3]),
            );
            let now = Fingerprint::of(&after, landed);
            if let (Some(was), Some(now)) = (was, now) {
                if !was.matches(&now) {
                    println!(
                        "  {} measured correctly but verification refused it",
                        pair.name
                    );
                }
            }
        }
    }
    let moved = cases.iter().filter(|c| c.3).count();
    let still = cases.len() - moved;
    println!(
        "\nestimator: {right}/{moved} moves right, {wrong} wrong, {refused} refused; still screens {still_kept}/{still} held, {still_lost} lost\n"
    );
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .expect("pass the corpus directory as the first argument");
    let pairs = load(Path::new(&dir));

    // Ground truth once, then every candidate rule graded against it.
    let mut cases = Vec::new();
    for pair in &pairs {
        let anchor = [
            pair.region[0] + CONTEXT,
            pair.region[1] + CONTEXT,
            (pair.region[2] - CONTEXT * 2.0).max(8.0),
            (pair.region[3] - CONTEXT * 2.0).max(8.0),
        ];
        let Some((dx, dy, _)) = truth(pair, to_pixels(pair, pair.region)) else {
            continue;
        };
        let px = pair.logical[0] / pair.width as f64;
        let py = pair.logical[1] / pair.height as f64;
        let followed = [
            anchor[0] + f64::from(dx) * px,
            anchor[1] + f64::from(dy) * py,
            anchor[2],
            anchor[3],
        ];
        cases.push((pair, anchor, followed, dx.abs() > 1 || dy.abs() > 1));
    }
    let moved = cases.iter().filter(|c| c.3).count();
    let still = cases.len() - moved;
    println!("{} pairs: {moved} moved, {still} still\n", pairs.len());
    end_to_end(&cases);
    println!(
        "{:>6} {:>4} {:>4} {:>4}   {:>10} {:>10} {:>10}",
        "sample", "grid", "tol", "bar", "followed", "caught", "still"
    );

    let samplers: [(&str, Sampler); 2] = [("point", point_sample), ("mean", cell_mean)];
    for (name, sampler) in samplers {
        for grid in [4usize, 6, 8] {
            for tolerance in [8.0f64, 12.0, 16.0, 24.0] {
                for bar in [0.5f64, 0.625, 0.75] {
                    let (mut kept, mut caught, mut still_kept) = (0, 0, 0);
                    for (pair, anchor, followed, real) in &cases {
                        let before = sampler(&pair.before, pair, *anchor, grid);
                        if *real {
                            let landed = sampler(&pair.after, pair, *followed, grid);
                            if agreement(&before, &landed, tolerance) >= bar {
                                kept += 1;
                            }
                            let stale = sampler(&pair.after, pair, *anchor, grid);
                            if agreement(&before, &stale, tolerance) < bar {
                                caught += 1;
                            }
                        } else {
                            let same = sampler(&pair.after, pair, *anchor, grid);
                            if agreement(&before, &same, tolerance) >= bar {
                                still_kept += 1;
                            }
                        }
                    }
                    let flag = if kept == moved && still_kept == still && caught * 2 >= moved {
                        " <-"
                    } else {
                        ""
                    };
                    println!(
                        "{name:>6} {grid:>4} {tolerance:>4} {bar:>4}   {kept:>4}/{moved:<5} {caught:>4}/{moved:<5} {still_kept:>4}/{still:<5}{flag}"
                    );
                }
            }
        }
    }
}
