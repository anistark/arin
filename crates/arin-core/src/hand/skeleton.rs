//! The strokes inside a filled glyph.
//!
//! An outline font stores the shape of the ink and nothing about how it was drawn. To draw
//! one on, the glyph is rasterised, thinned to a one pixel skeleton, and the skeleton is
//! traced into paths. Those paths only steer the reveal: the ink shown is always the
//! glyph's own outline, and the line that reveals it is as thick as the glyph's thickest
//! part, so a skeleton that comes out slightly wrong is hidden under ink that is right.
//!
//! The order of the paths is a heuristic, left to right and then top to bottom, with dots
//! last. It is right for most Latin letters and plausible for most others, which is the
//! most any order taken from a shape alone can promise.

use super::in_writing_order;
use std::collections::HashSet;

/// Pixels per em the glyph is rasterised at.
///
/// Enough that a thin stroke of a handwriting font is a few pixels wide and survives
/// thinning, and small enough that a glyph costs a fraction of a millisecond.
const RESOLUTION: f64 = 96.0;

/// How wide the bands are that strokes are ordered left to right in, in ems.
const ORDER_BAND: f64 = 0.12;

/// A path shorter than this, in ems, is a dot and goes last.
const DOT: f64 = 0.08;

/// A glyph's strokes and how thick its ink is, both in ems, from its filled contours.
///
/// Contours are in ems with y growing downward and are filled by the non-zero rule, which
/// is how TrueType and CFF outlines describe holes. A glyph whose skeleton yields nothing
/// usable gets a single stroke across its middle, so it is still revealed left to right.
pub(super) fn strokes(contours: &[Vec<(f64, f64)>]) -> (Vec<Vec<(f64, f64)>>, f64) {
    let Some(grid) = Grid::fill(contours) else {
        return (Vec::new(), 0.0);
    };
    let half = grid.half_thickness();
    let thickness = (half * 2.0 + 1.0) / RESOLUTION;

    let mut skeleton = grid.clone();
    skeleton.thin();
    skeleton.unstair();
    let spur = (half * 2.0).max(3.0) as usize;
    let mut paths = skeleton.trace();
    paths.retain(|path| path.len() > spur || !skeleton.touches_junction(path));

    let mut strokes: Vec<Vec<(f64, f64)>> = paths
        .into_iter()
        .map(|path| {
            let simplified = simplify(&path, 0.7);
            simplified
                .into_iter()
                .map(|(x, y)| grid.to_em(x as f64, y as f64))
                .collect()
        })
        .filter(|stroke: &Vec<(f64, f64)>| stroke.len() >= 2)
        .collect();

    if strokes.is_empty() {
        let (min, max) = grid.bounds_em();
        let middle = (min.1 + max.1) / 2.0;
        return (
            vec![vec![(min.0, middle), (max.0, middle)]],
            (max.1 - min.1).max(thickness),
        );
    }

    strokes.sort_by(|a, b| {
        let key = |s: &Vec<(f64, f64)>| {
            let left = s.iter().map(|p| p.0).fold(f64::MAX, f64::min);
            let top = s.iter().map(|p| p.1).fold(f64::MAX, f64::min);
            ((left / ORDER_BAND).floor() as i64, top)
        };
        let (ka, kb) = (key(a), key(b));
        ka.0.cmp(&kb.0).then(ka.1.total_cmp(&kb.1))
    });
    (in_writing_order(strokes, DOT), thickness)
}

/// A rasterised glyph.
#[derive(Clone)]
struct Grid {
    width: usize,
    height: usize,
    cells: Vec<bool>,
    /// The em position of cell (0, 0)'s corner.
    origin: (f64, f64),
}

impl Grid {
    /// Rasterise contours with the non-zero rule, with a blank border all round.
    fn fill(contours: &[Vec<(f64, f64)>]) -> Option<Self> {
        let points = contours.iter().flatten();
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for &(x, y) in points {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
        if min_x > max_x || min_y > max_y {
            return None;
        }
        let border = 2.0 / RESOLUTION;
        let origin = (min_x - border, min_y - border);
        let width = ((max_x - min_x) * RESOLUTION).ceil() as usize + 5;
        let height = ((max_y - min_y) * RESOLUTION).ceil() as usize + 5;
        let mut cells = vec![false; width * height];

        let mut crossings: Vec<(f64, i32)> = Vec::new();
        for row in 0..height {
            let y = origin.1 + (row as f64 + 0.5) / RESOLUTION;
            crossings.clear();
            for contour in contours {
                for i in 0..contour.len() {
                    let a = contour[i];
                    let b = contour[(i + 1) % contour.len()];
                    let direction = if a.1 <= y && b.1 > y {
                        1
                    } else if b.1 <= y && a.1 > y {
                        -1
                    } else {
                        continue;
                    };
                    let x = a.0 + (y - a.1) / (b.1 - a.1) * (b.0 - a.0);
                    crossings.push((x, direction));
                }
            }
            crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut winding = 0;
            for pair in crossings.windows(2) {
                winding += pair[0].1;
                if winding == 0 {
                    continue;
                }
                let from = ((pair[0].0 - origin.0) * RESOLUTION - 0.5).ceil().max(0.0) as usize;
                let to = ((pair[1].0 - origin.0) * RESOLUTION - 0.5).floor();
                if to < 0.0 {
                    continue;
                }
                for column in from..=(to as usize).min(width - 1) {
                    cells[row * width + column] = true;
                }
            }
        }
        if !cells.iter().any(|&c| c) {
            return None;
        }
        Some(Self {
            width,
            height,
            cells,
            origin,
        })
    }

    fn at(&self, x: isize, y: isize) -> bool {
        x >= 0
            && y >= 0
            && (x as usize) < self.width
            && (y as usize) < self.height
            && self.cells[y as usize * self.width + x as usize]
    }

    fn to_em(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.origin.0 + (x + 0.5) / RESOLUTION,
            self.origin.1 + (y + 0.5) / RESOLUTION,
        )
    }

    fn bounds_em(&self) -> ((f64, f64), (f64, f64)) {
        let (mut min, mut max) = ((usize::MAX, usize::MAX), (0, 0));
        for y in 0..self.height {
            for x in 0..self.width {
                if self.cells[y * self.width + x] {
                    min = (min.0.min(x), min.1.min(y));
                    max = (max.0.max(x), max.1.max(y));
                }
            }
        }
        (
            self.to_em(min.0 as f64, min.1 as f64),
            self.to_em(max.0 as f64, max.1 as f64),
        )
    }

    /// The largest distance from inside the ink to its edge, in pixels.
    ///
    /// A chamfer distance with weights three and four, which is within a few percent of the
    /// true distance and needs two passes rather than a search per pixel.
    fn half_thickness(&self) -> f64 {
        let (w, h) = (self.width, self.height);
        let mut distance: Vec<u32> = self
            .cells
            .iter()
            .map(|&c| if c { u32::MAX / 2 } else { 0 })
            .collect();
        let at = |d: &Vec<u32>, x: isize, y: isize| -> u32 {
            if x < 0 || y < 0 || x as usize >= w || y as usize >= h {
                0
            } else {
                d[y as usize * w + x as usize]
            }
        };
        for y in 0..h as isize {
            for x in 0..w as isize {
                let i = y as usize * w + x as usize;
                if distance[i] == 0 {
                    continue;
                }
                let best = [
                    at(&distance, x - 1, y) + 3,
                    at(&distance, x, y - 1) + 3,
                    at(&distance, x - 1, y - 1) + 4,
                    at(&distance, x + 1, y - 1) + 4,
                ]
                .into_iter()
                .min()
                .unwrap_or(0);
                distance[i] = distance[i].min(best);
            }
        }
        for y in (0..h as isize).rev() {
            for x in (0..w as isize).rev() {
                let i = y as usize * w + x as usize;
                if distance[i] == 0 {
                    continue;
                }
                let best = [
                    at(&distance, x + 1, y) + 3,
                    at(&distance, x, y + 1) + 3,
                    at(&distance, x + 1, y + 1) + 4,
                    at(&distance, x - 1, y + 1) + 4,
                ]
                .into_iter()
                .min()
                .unwrap_or(0);
                distance[i] = distance[i].min(best);
            }
        }
        f64::from(distance.into_iter().max().unwrap_or(0)) / 3.0
    }

    /// Thin to a one pixel skeleton, Zhang and Suen's way.
    fn thin(&mut self) {
        loop {
            let mut changed = false;
            for pass in 0..2 {
                let mut doomed = Vec::new();
                for y in 0..self.height as isize {
                    for x in 0..self.width as isize {
                        if !self.at(x, y) {
                            continue;
                        }
                        let n = [
                            self.at(x, y - 1),
                            self.at(x + 1, y - 1),
                            self.at(x + 1, y),
                            self.at(x + 1, y + 1),
                            self.at(x, y + 1),
                            self.at(x - 1, y + 1),
                            self.at(x - 1, y),
                            self.at(x - 1, y - 1),
                        ];
                        let neighbours = n.iter().filter(|&&b| b).count();
                        if !(2..=6).contains(&neighbours) {
                            continue;
                        }
                        let transitions = (0..8).filter(|&i| !n[i] && n[(i + 1) % 8]).count();
                        if transitions != 1 {
                            continue;
                        }
                        let (p2, p4, p6, p8) = (n[0], n[2], n[4], n[6]);
                        let keep = if pass == 0 {
                            p2 && p4 && p6 || p4 && p6 && p8
                        } else {
                            p2 && p4 && p8 || p2 && p6 && p8
                        };
                        if !keep {
                            doomed.push(y as usize * self.width + x as usize);
                        }
                    }
                }
                changed |= !doomed.is_empty();
                for i in doomed {
                    self.cells[i] = false;
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// Finish thinning what Zhang and Suen leave two pixels thick.
    ///
    /// Their method thins in parallel, which leaves diagonal runs two pixels thick and
    /// stepped like a staircase. Read as junctions, those pixels would cut a loop into
    /// pieces at every turn. A sequential pass removes every pixel whose neighbours are
    /// still connected to each other without it.
    fn unstair(&mut self) {
        let ring = [
            (0, -1),
            (1, -1),
            (1, 0),
            (1, 1),
            (0, 1),
            (-1, 1),
            (-1, 0),
            (-1, -1),
        ];
        loop {
            let mut changed = false;
            for y in 0..self.height as isize {
                for x in 0..self.width as isize {
                    if !self.at(x, y) {
                        continue;
                    }
                    let around: Vec<(isize, isize)> = ring
                        .iter()
                        .copied()
                        .filter(|&(dx, dy)| self.at(x + dx, y + dy))
                        .collect();
                    // Removable when what it touches still touches without it, and when
                    // it is not the end of a line.
                    let redundant = around.len() >= 2 && components(&around) == 1;
                    if redundant {
                        self.cells[y as usize * self.width + x as usize] = false;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }

    fn neighbours(&self, (x, y): (usize, usize)) -> Vec<(usize, usize)> {
        let mut out = Vec::with_capacity(8);
        for dy in -1isize..=1 {
            for dx in -1isize..=1 {
                if (dx, dy) != (0, 0) && self.at(x as isize + dx, y as isize + dy) {
                    out.push(((x as isize + dx) as usize, (y as isize + dy) as usize));
                }
            }
        }
        out
    }

    fn is_node(&self, cell: (usize, usize)) -> bool {
        self.neighbours(cell).len() != 2
    }

    fn touches_junction(&self, path: &[(usize, usize)]) -> bool {
        [path.first(), path.last()]
            .into_iter()
            .flatten()
            .any(|&end| self.neighbours(end).len() >= 3)
    }

    /// Walk the skeleton into paths between its ends and junctions, then its loops.
    fn trace(&self) -> Vec<Vec<(usize, usize)>> {
        let mut walked: HashSet<((usize, usize), (usize, usize))> = HashSet::new();
        let edge = |a: (usize, usize), b: (usize, usize)| if a < b { (a, b) } else { (b, a) };
        let mut paths = Vec::new();
        let cells: Vec<(usize, usize)> = (0..self.height)
            .flat_map(|y| (0..self.width).map(move |x| (x, y)))
            .filter(|&(x, y)| self.cells[y * self.width + x])
            .collect();

        for &start in cells.iter().filter(|&&c| self.is_node(c)) {
            // A pixel on its own is a dot thinned to nothing more.
            if self.neighbours(start).is_empty() {
                paths.push(vec![start, start]);
                continue;
            }
            for next in self.neighbours(start) {
                if walked.contains(&edge(start, next)) {
                    continue;
                }
                let mut path = vec![start];
                let (mut previous, mut current) = (start, next);
                walked.insert(edge(previous, current));
                loop {
                    path.push(current);
                    if self.is_node(current) {
                        break;
                    }
                    let Some(step) = self
                        .neighbours(current)
                        .into_iter()
                        .find(|&n| n != previous && !walked.contains(&edge(current, n)))
                    else {
                        break;
                    };
                    walked.insert(edge(current, step));
                    previous = current;
                    current = step;
                }
                paths.push(path);
            }
        }

        // Whatever is left is closed loops with no ends, such as an o.
        for &start in &cells {
            let Some(next) = self
                .neighbours(start)
                .into_iter()
                .find(|&n| !walked.contains(&edge(start, n)))
            else {
                continue;
            };
            let mut path = vec![start];
            let (mut previous, mut current) = (start, next);
            walked.insert(edge(previous, current));
            while current != start {
                path.push(current);
                let Some(step) = self
                    .neighbours(current)
                    .into_iter()
                    .find(|&n| n != previous && !walked.contains(&edge(current, n)))
                else {
                    break;
                };
                walked.insert(edge(current, step));
                previous = current;
                current = step;
            }
            path.push(start);
            paths.push(path);
        }
        paths
    }
}

/// How many groups a pixel's neighbours fall into, counting touching diagonally.
fn components(around: &[(isize, isize)]) -> usize {
    let mut seen = vec![false; around.len()];
    let mut groups = 0;
    for start in 0..around.len() {
        if seen[start] {
            continue;
        }
        groups += 1;
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(i) = stack.pop() {
            for j in 0..around.len() {
                let touching = (around[i].0 - around[j].0).abs() <= 1
                    && (around[i].1 - around[j].1).abs() <= 1;
                if !seen[j] && touching {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
    }
    groups
}

/// Ramer, Douglas and Peucker: drop points within `epsilon` of the line through their
/// neighbours, keeping the ends.
fn simplify(path: &[(usize, usize)], epsilon: f64) -> Vec<(usize, usize)> {
    if path.len() < 3 {
        return path.to_vec();
    }
    let (first, last) = (path[0], path[path.len() - 1]);
    let (ax, ay) = (first.0 as f64, first.1 as f64);
    let (bx, by) = (last.0 as f64, last.1 as f64);
    let length = (bx - ax).hypot(by - ay);
    let (index, distance) = path
        .iter()
        .enumerate()
        .skip(1)
        .take(path.len() - 2)
        .map(|(i, &(x, y))| {
            let (x, y) = (x as f64, y as f64);
            let d = if length > 0.0 {
                ((bx - ax) * (ay - y) - (ax - x) * (by - ay)).abs() / length
            } else {
                (x - ax).hypot(y - ay)
            };
            (i, d)
        })
        .fold(
            (0, 0.0),
            |best, (i, d)| if d > best.1 { (i, d) } else { best },
        );
    if distance <= epsilon {
        return vec![first, last];
    }
    let mut left = simplify(&path[..=index], epsilon);
    let right = simplify(&path[index..], epsilon);
    left.pop();
    left.extend(right);
    left
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Vec<(f64, f64)> {
        vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
    }

    #[test]
    fn a_bar_is_one_stroke_along_it() {
        let (strokes, thickness) = strokes(&[rect(0.0, 0.0, 0.6, 0.08)]);
        assert_eq!(strokes.len(), 1, "{strokes:?}");
        let s = &strokes[0];
        let run = (s.last().unwrap().0 - s[0].0).abs();
        assert!(run > 0.4, "{s:?}");
        assert!((thickness - 0.08).abs() < 0.03, "{thickness}");
    }

    #[test]
    fn a_horizontal_bar_is_drawn_from_its_left() {
        let (strokes, _) = strokes(&[rect(0.0, 0.0, 0.6, 0.08)]);
        let s = &strokes[0];
        assert!(s[0].0 < s.last().unwrap().0);
    }

    #[test]
    fn a_ring_is_one_closed_stroke() {
        let outer: Vec<(f64, f64)> = (0..64)
            .map(|i| {
                let a = i as f64 / 64.0 * std::f64::consts::TAU;
                (0.3 * a.cos(), 0.3 * a.sin())
            })
            .collect();
        let inner: Vec<(f64, f64)> = outer
            .iter()
            .rev()
            .map(|&(x, y)| (x * 0.7, y * 0.7))
            .collect();
        let (strokes, _) = strokes(&[outer, inner]);
        assert_eq!(strokes.len(), 1, "{strokes:?}");
        let s = &strokes[0];
        let gap = (s[0].0 - s.last().unwrap().0).hypot(s[0].1 - s.last().unwrap().1);
        assert!(gap < 0.05, "a ring should come back to its start: {gap}");
    }

    #[test]
    fn a_cross_gives_strokes_and_a_dot_goes_last() {
        let vertical = rect(0.27, 0.0, 0.06, 0.6);
        let horizontal = rect(0.0, 0.27, 0.6, 0.06);
        let dot = rect(0.9, 0.0, 0.05, 0.05);
        let (strokes, _) = strokes(&[vertical, horizontal, dot]);
        assert!(strokes.len() >= 2, "{strokes:?}");
        let last = strokes.last().unwrap();
        assert!(
            last.iter().all(|p| p.0 > 0.8),
            "the dot should be last: {strokes:?}"
        );
    }

    #[test]
    fn nothing_to_fill_has_no_strokes() {
        assert!(strokes(&[]).0.is_empty());
    }
}
