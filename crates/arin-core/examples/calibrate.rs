//! Try candidate movement scorers against real captures.
//!
//! The thresholds in `signature` were chosen against generated patterns and do not survive
//! contact with a screen: measured on a laptop display, the correct offset for a scroll and
//! a knowingly wrong one score within 1.3 of each other, where the same two numbers on a
//! generated pattern are 85 apart. This replays recorded frame pairs through several
//! scorers so the difference between them can be seen rather than argued about.
//!
//! Record a corpus by running the daemon with `ARIN_RECORD` set to a directory, then:
//!
//! ```text
//! cargo run --release --example calibrate -- <that directory>
//! ```
//!
//! Ground truth comes from a full two dimensional comparison of the region, which is far
//! too slow to run twice a second but has no feature engineering in it to be wrong about.
//! Whatever a cheap one dimensional scorer says is judged against that.

use arin_core::record::replay;
use std::path::{Path, PathBuf};

/// One recorded pair, in the shape the scorers below want it.
struct Pair {
    name: String,
    width: usize,
    height: usize,
    logical: [f64; 2],
    region: [f64; 4],
    before: Vec<u8>,
    after: Vec<u8>,
}

/// Read the corpus through the daemon's own reader.
///
/// Only [`arin_core::record`] defines this format, so parsing it anywhere else is a second
/// definition that drifts from the first.
fn load(dir: &Path) -> Vec<Pair> {
    replay(dir)
        .into_iter()
        .filter_map(|r| {
            let region = r.regions.first().copied()?;
            Some(Pair {
                name: r.name,
                width: r.after.width as usize,
                height: r.after.height as usize,
                logical: r.after.logical_size,
                region: [region.x, region.y, region.width, region.height],
                before: r.before.pixels.to_vec(),
                after: r.after.pixels.to_vec(),
            })
        })
        .collect()
}

/// Brightness of one pixel, through the daemon's own function rather than a copy.
///
/// A harness that samples differently from the daemon predicts the wrong thing.
fn luminance(bgra: &[u8]) -> f64 {
    f64::from(arin_core::traits::luminance(bgra))
}

/// The region in frame pixels.
fn area(pair: &Pair) -> (usize, usize, usize, usize) {
    let [x, y, w, h] = pair.region;
    let sx = |v: f64| ((v / pair.logical[0]) * pair.width as f64).round() as usize;
    let sy = |v: f64| ((v / pair.logical[1]) * pair.height as f64).round() as usize;
    (
        sx(x).min(pair.width - 1),
        sy(y).min(pair.height - 1),
        sx(x + w).min(pair.width),
        sy(y + h).min(pair.height),
    )
}

/// Most samples averaged into a band. Must match `signature::BAND_SAMPLES`, or this
/// harness calibrates a rule against a feature the daemon does not compute. Getting that
/// wrong once already produced a rule that agreed offline and disagreed on the screen.
const BAND_SAMPLES: usize = 512;

/// Mean luminance per row of the region, sampled the way the daemon samples it.
fn row_means(pixels: &[u8], width: usize, a: (usize, usize, usize, usize)) -> Vec<f64> {
    let (x0, y0, x1, y1) = a;
    (y0..y1)
        .map(|y| {
            let stride = (x1 - x0).div_ceil(BAND_SAMPLES).max(1);
            let mut total = 0.0;
            let mut n = 0.0;
            let mut x = x0;
            while x < x1 {
                let idx = (y * width + x) * 4;
                if idx + 4 <= pixels.len() {
                    total += luminance(&pixels[idx..idx + 4]);
                    n += 1.0;
                }
                x += stride;
            }
            if n > 0.0 { total / n } else { 0.0 }
        })
        .collect()
}

/// The same profile with its low frequencies removed.
///
/// A window background is smooth and therefore nearly unchanged by a small shift, so it
/// contributes to every offset equally and drowns the text lines that actually move.
///
/// `window` of 1 is a plain first difference, which is the most aggressive filter there is
/// and amplifies noise along with the signal. Anything larger subtracts a moving average
/// instead, keeping more of the structure.
fn high_pass(profile: &[f64], window: usize) -> Vec<f64> {
    if window <= 1 {
        return profile.windows(2).map(|w| w[1] - w[0]).collect();
    }
    let half = window / 2;
    (0..profile.len())
        .map(|i| {
            let lo = i.saturating_sub(half);
            let hi = (i + half + 1).min(profile.len());
            let local: f64 = profile[lo..hi].iter().sum::<f64>() / (hi - lo) as f64;
            profile[i] - local
        })
        .collect()
}

/// Mean absolute difference at an offset. Lower is better. Today's score.
fn mad(a: &[f64], b: &[f64], k: i32) -> Option<f64> {
    let n = a.len() as i32;
    let first = k.max(0);
    let last = (n + k).min(n);
    if (last - first) * 2 < n {
        return None;
    }
    let mut total = 0.0;
    for i in first..last {
        total += (b[i as usize] - a[(i - k) as usize]).abs();
    }
    Some(total / f64::from(last - first))
}

/// Normalised cross correlation at an offset. Higher is better, capped at 1.
fn ncc(a: &[f64], b: &[f64], k: i32) -> Option<f64> {
    let n = a.len() as i32;
    let first = k.max(0);
    let last = (n + k).min(n);
    let count = last - first;
    if count * 2 < n {
        return None;
    }
    let (mut sa, mut sb) = (0.0, 0.0);
    for i in first..last {
        sa += a[(i - k) as usize];
        sb += b[i as usize];
    }
    let (ma, mb) = (sa / f64::from(count), sb / f64::from(count));

    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for i in first..last {
        let x = a[(i - k) as usize] - ma;
        let y = b[i as usize] - mb;
        num += x * y;
        da += x * x;
        db += y * y;
    }
    if da <= f64::EPSILON || db <= f64::EPSILON {
        return None;
    }
    Some(num / (da * db).sqrt())
}

/// Per-row means, but computed separately for each vertical strip of the region.
///
/// One mean per row throws away everything about *where* in the row the ink was, which is
/// most of what distinguishes one line of text from another. Splitting the region into a
/// few strips keeps a coarse version of that at a few times the cost.
fn strip_means(
    pixels: &[u8],
    width: usize,
    a: (usize, usize, usize, usize),
    strips: usize,
) -> Vec<Vec<f64>> {
    let (x0, y0, x1, y1) = a;
    let span = (x1 - x0).max(1);
    (0..strips)
        .map(|s| {
            let sx0 = x0 + s * span / strips;
            let sx1 = (x0 + (s + 1) * span / strips).max(sx0 + 1).min(x1);
            row_means(pixels, width, (sx0, y0, sx1, y1))
        })
        .collect()
}

/// Mean absolute difference summed across every strip.
fn strip_mad(before: &[Vec<f64>], after: &[Vec<f64>], k: i32) -> Option<f64> {
    let mut total = 0.0;
    for (a, b) in before.iter().zip(after) {
        total += mad(a, b, k)?;
    }
    Some(total / before.len() as f64)
}

/// The offsets a cheap scorer thinks are worth a closer look.
fn candidates(scores: &[(i32, f64)], take: usize) -> Vec<i32> {
    let mut sorted = scores.to_vec();
    sorted.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut picked: Vec<i32> = Vec::new();
    for (offset, _) in sorted {
        if picked.iter().all(|p| (p - offset).abs() > 2) {
            picked.push(offset);
        }
        if picked.len() == take {
            break;
        }
    }
    picked
}

/// Ground truth: compare the region's actual pixels at each offset.
///
/// No feature engineering to be wrong about, and far too slow for a timer.
fn reference(pair: &Pair, a: (usize, usize, usize, usize), k: i32) -> Option<f64> {
    let (x0, y0, x1, y1) = a;
    let rows = (y1 - y0) as i32;
    let first = y0 as i32 + k.max(0);
    let last = (y1 as i32 + k.min(0)).min(y1 as i32);
    if (last - first) * 2 < rows {
        return None;
    }
    let mut total = 0.0;
    let mut n = 0.0;
    let mut y = first;
    while y < last {
        let mut x = x0;
        while x < x1 {
            let i = (y as usize * pair.width + x) * 4;
            let j = ((y - k) as usize * pair.width + x) * 4;
            if i + 4 <= pair.after.len() && j + 4 <= pair.before.len() {
                total +=
                    (luminance(&pair.after[i..i + 4]) - luminance(&pair.before[j..j + 4])).abs();
                n += 1.0;
            }
            x += 4;
        }
        y += 1;
    }
    if n > 0.0 { Some(total / n) } else { None }
}

/// Best offset and how far clear of its nearest distant rival it is.
struct Peak {
    best: i32,
    score: f64,
    /// Kept for trying acceptance rules that compare the winner against its rivals.
    #[allow(dead_code)]
    rival: f64,
}

fn peak_of(scores: &[(i32, f64)], higher_is_better: bool) -> Option<Peak> {
    let best = scores.iter().copied().reduce(|a, b| {
        if (higher_is_better && b.1 > a.1) || (!higher_is_better && b.1 < a.1) {
            b
        } else {
            a
        }
    })?;
    let rival = scores
        .iter()
        .filter(|(k, _)| (k - best.0).abs() > 2)
        .map(|(_, s)| *s)
        .fold(
            if higher_is_better {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            },
            |acc, s| {
                if higher_is_better {
                    acc.max(s)
                } else {
                    acc.min(s)
                }
            },
        );
    Some(Peak {
        best: best.0,
        score: best.1,
        rival,
    })
}

/// High-pass windows to compare. 1 is a plain first difference.
const FILTERS: &[(&str, usize)] = &[
    ("difference", 1),
    ("window-3", 3),
    ("window-5", 5),
    ("window-9", 9),
    ("window-17", 17),
    ("none", 0),
];

fn main() {
    let dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .expect("usage: calibrate <corpus directory>");
    let pairs = load(&dir);
    println!("{} pairs\n", pairs.len());

    println!(
        "{:<16} {:>5} | {:>5} {:>6} {:>6} {:>6} | {:>5} {:>6} {:>6} | {:>5}",
        "pair", "truth", "mad", "@best", "@0", "gain", "ncc", "@best", "@0", "agree"
    );
    println!("{}", "-".repeat(84));

    let mut agree_mad = 0;
    let mut agree_ncc = 0;
    let mut moved = 0;
    let (mut strips_total, mut strips_right) = (0, 0);
    let (mut strips_still, mut strips_still_right) = (0, 0);
    let (mut confirm_total, mut confirm_right) = (0, 0);
    // filter name -> (agreements on moving pairs, of which correct, agreements on still)
    let mut tallies: std::collections::BTreeMap<&str, (usize, usize, usize)> =
        std::collections::BTreeMap::new();

    for pair in &pairs {
        let a = area(pair);
        let rows = (a.3 - a.1) as i32;
        let reach = rows / 2;

        let truth = peak_of(
            &(-reach..=reach)
                .filter_map(|k| reference(pair, a, k).map(|s| (k, s)))
                .collect::<Vec<_>>(),
            false,
        );
        let Some(truth) = truth else { continue };

        let before = row_means(&pair.before, pair.width, a);
        let after = row_means(&pair.after, pair.width, a);

        let mad_peak = peak_of(
            &(-reach..=reach)
                .filter_map(|k| mad(&before, &after, k).map(|s| (k, s)))
                .collect::<Vec<_>>(),
            false,
        );

        let hb = high_pass(&before, 1);
        let ha = high_pass(&after, 1);
        let ncc_peak = peak_of(
            &(-reach..=reach)
                .filter_map(|k| ncc(&hb, &ha, k).map(|s| (k, s)))
                .collect::<Vec<_>>(),
            true,
        );

        let Some(m) = mad_peak else { continue };
        let m_best = m.best;

        // Strips: the same projection, kept per vertical slice of the region.
        let sb = strip_means(&pair.before, pair.width, a, 8);
        let sa = strip_means(&pair.after, pair.width, a, 8);
        if let Some(p) = peak_of(
            &(-reach..=reach)
                .filter_map(|k| strip_mad(&sb, &sa, k).map(|s| (k, s)))
                .collect::<Vec<_>>(),
            false,
        ) {
            let staying = strip_mad(&sb, &sa, 0).unwrap_or(f64::INFINITY);
            let gain = if p.score > f64::EPSILON {
                staying / p.score
            } else {
                f64::INFINITY
            };
            let sharpness = if p.score > f64::EPSILON {
                p.rival / p.score
            } else {
                f64::INFINITY
            };
            println!(
                "  strips {:<14} truth={:<5} best={:<5} score={:<7.2} gain={:<6.2} sharp={:<6.2}",
                pair.name, truth.best, p.best, p.score, gain, sharpness
            );
            if truth.best != 0 {
                strips_total += 1;
                if (p.best - truth.best).abs() <= 2 {
                    strips_right += 1;
                }
            } else {
                strips_still += 1;
                if p.best.abs() <= 2 {
                    strips_still_right += 1;
                }
            }
        }

        // Propose with the cheap scorer, confirm with a strided two dimensional check of
        // only the handful of offsets it liked.
        let proposals = candidates(
            &(-reach..=reach)
                .filter_map(|k| mad(&before, &after, k).map(|s| (k, s)))
                .collect::<Vec<_>>(),
            5,
        );
        if let Some(best) = proposals
            .iter()
            .filter_map(|k| reference(pair, a, *k).map(|s| (*k, s)))
            .min_by(|x, y| x.1.total_cmp(&y.1))
            && truth.best != 0
        {
            confirm_total += 1;
            if (best.0 - truth.best).abs() <= 2 {
                confirm_right += 1;
            }
        }
        let Some(n) = ncc_peak else { continue };
        for (name, window) in FILTERS {
            let hb = high_pass(&before, *window);
            let ha = high_pass(&after, *window);
            if let Some(p) = peak_of(
                &(-reach..=reach)
                    .filter_map(|k| ncc(&hb, &ha, k).map(|s| (k, s)))
                    .collect::<Vec<_>>(),
                true,
            ) {
                let agrees = (p.best - m_best).abs() <= 2;
                let right = (p.best - truth.best).abs() <= 2;
                let tally = tallies.entry(*name).or_default();
                if truth.best != 0 {
                    if agrees {
                        tally.0 += 1;
                        if right {
                            tally.1 += 1;
                        }
                    }
                } else if agrees {
                    tally.2 += 1;
                }
            }
        }

        let m_ok = (m.best - truth.best).abs() <= 2;
        let n_ok = (n.best - truth.best).abs() <= 2;
        if truth.best != 0 {
            moved += 1;
            if m_ok {
                agree_mad += 1;
            }
            if n_ok {
                agree_ncc += 1;
            }
        }

        // What "nothing moved" scores, which is the hypothesis a shift has to beat.
        let mad_still = mad(&before, &after, 0).unwrap_or(f64::INFINITY);
        let ncc_still = ncc(&hb, &ha, 0).unwrap_or(0.0);
        // How much better shifting explains the change than staying put.
        let gain = if m.score > f64::EPSILON {
            mad_still / m.score
        } else {
            f64::INFINITY
        };

        println!(
            "{:<16} {:>5} | {:>5} {:>6.2} {:>6.2} {:>6.2} | {:>5} {:>6.3} {:>6.3} | {:>5}",
            pair.name,
            truth.best,
            m.best,
            m.score,
            mad_still,
            gain,
            n.best,
            n.score,
            ncc_still,
            match (m_ok, n_ok) {
                (true, true) => "both",
                (false, true) => "ncc",
                (true, false) => "mad",
                (false, false) => "none",
            }
        );
    }

    println!();
    println!("pairs where content genuinely moved: {moved}");
    println!();
    println!(
        "{:<14} {:>10} {:>10} {:>12}",
        "high-pass", "agreed", "of which ok", "still agreed"
    );
    for (name, (agreed, right, still)) in &tallies {
        println!("{name:<14} {agreed:>10} {right:>10} {still:>12}");
    }
    println!("  mean-per-row MAD agreed with truth: {agree_mad}");
    println!("  high-passed NCC agreed with truth:  {agree_ncc}");
    println!("  eight strips of row means:          {strips_right} of {strips_total}");
    println!("  ... and on still pairs, said still:  {strips_still_right} of {strips_still}");
    println!("  cheap proposal, 2D confirmation:    {confirm_right} of {confirm_total}");
}
