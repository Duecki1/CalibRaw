//! Where each frame takes over from another, on the seam grid.
//!
//! Every pixel starts with the frame it lies deepest inside (a Voronoi split by distance
//! from the frame edges, the idea behind Enblend's nearest-feature transform). Then each
//! overlapping pair's shared pixels are relabelled by a minimum graph cut, pair by pair
//! as OpenCV's `PairwiseSeamFinder` runs, with the colour cost OpenCV's `Stitcher` uses
//! by default (`GraphCutSeamFinder` with `COST_COLOR`, `modules/stitching/src/
//! seam_finders.cpp`, BSD-3-Clause; after Kwatra et al., "Graphcut Textures", SIGGRAPH
//! 2003). A cut is cheap only where the frames agree, so seams go around moving
//! subjects and around near objects that parallax shifts between the frames. (The
//! colour-and-gradient cost would make cuts through busy detail cheap, and halves such
//! objects.)
//!
//! The grid is coarse (Enblend's `--coarse-mask` likewise finds seams at an eighth of
//! the resolution); the full-resolution mask is sharp, as Enblend fills its scaled-up
//! seam contour (`createMask`, `src/mask.h`), so frames never mix across the seam and
//! each band of the blend widens the transition only as far as its own scale.

use super::maxflow::Graph;
use super::overview::Overview;
use super::projection::Grid;
use rayon::prelude::*;

/// A grid pixel no frame covers.
pub(super) const NO_FRAME: u16 = u16::MAX;
/// OpenCV's defaults, for values on a 0–255 scale.
const TERMINAL_COST: f32 = 10_000.0;
const BAD_REGION_PENALTY: f32 = 1_000.0;
const WEIGHT_EPS: f32 = 1.0;

/// The frame each seam-grid pixel takes its picture from.
pub(super) struct Seams {
    pub(super) grid: Grid,
    pub(super) labels: Vec<u16>,
}

impl Seams {
    /// The frame owning fractional grid position `(x, y)`: the label with the largest
    /// bilinear weight among the four nearest pixels, so seams run smoothly between grid
    /// pixels instead of in steps of whole grid pixels.
    pub(super) fn owner(&self, x: f64, y: f64) -> u16 {
        let (w, h) = (self.grid.width, self.grid.height);
        let x = x.clamp(0.0, (w - 1) as f64);
        let y = y.clamp(0.0, (h - 1) as f64);
        let (ix, iy) = (x.floor() as usize, y.floor() as usize);
        let (jx, jy) = ((ix + 1).min(w - 1), (iy + 1).min(h - 1));
        let (fx, fy) = (x - ix as f64, y - iy as f64);
        let taps = [
            (self.labels[iy * w + ix], (1.0 - fx) * (1.0 - fy)),
            (self.labels[iy * w + jx], fx * (1.0 - fy)),
            (self.labels[jy * w + ix], (1.0 - fx) * fy),
            (self.labels[jy * w + jx], fx * fy),
        ];
        let weight = |label: u16| -> f64 {
            taps.iter()
                .filter(|(l, _)| *l == label)
                .map(|(_, w)| w)
                .sum()
        };
        // Ties go to the earliest tap, so the choice is deterministic.
        taps.iter()
            .map(|&(label, _)| (label, weight(label)))
            .fold((NO_FRAME, f64::NEG_INFINITY), |best, candidate| {
                if candidate.1 > best.1 {
                    candidate
                } else {
                    best
                }
            })
            .0
    }
}

/// A frame's overview prepared for the cost: cube-root encoded values on a 0–255
/// scale, zero where the frame does not cover.
struct Encoded {
    values: Vec<[f32; 3]>,
}

/// Finds the seams between `overviews` (on `grid`) after multiplying frame `k` by
/// `gains[k]`.
pub(super) fn find(grid: Grid, overviews: &[Overview], gains: &[f64]) -> Seams {
    let (w, h) = (grid.width, grid.height);
    let mut labels: Vec<u16> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let mut best = (NO_FRAME, f32::NEG_INFINITY);
            for (k, overview) in overviews.iter().enumerate() {
                if let Some(index) = overview.index(x, y).filter(|&j| overview.covered[j]) {
                    if overview.depth[index] > best.1 {
                        best = (k as u16, overview.depth[index]);
                    }
                }
            }
            best.0
        })
        .collect();
    let encoded: Vec<Encoded> = overviews
        .par_iter()
        .zip(gains)
        .map(|(overview, &gain)| encode(overview, gain as f32))
        .collect();
    for first in 0..overviews.len() {
        for second in first + 1..overviews.len() {
            cut_pair(
                &mut labels,
                w,
                [&overviews[first], &overviews[second]],
                [&encoded[first], &encoded[second]],
                [first as u16, second as u16],
            );
        }
    }
    Seams { grid, labels }
}

fn encode(overview: &Overview, gain: f32) -> Encoded {
    let values = overview
        .rgb
        .par_iter()
        .zip(&overview.covered)
        .map(|(rgb, &covered)| {
            if covered {
                rgb.map(|v| (v * gain).max(0.0).cbrt() * 255.0)
            } else {
                [0.0; 3]
            }
        })
        .collect();
    Encoded { values }
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// Relabels the pixels both frames cover and currently own between `frames[0]` and
/// `frames[1]` by a minimum cut. Pixels around them owned by either frame anchor the cut
/// to that frame; pixels of other frames are left out.
fn cut_pair(
    labels: &mut [u16],
    grid_width: usize,
    overviews: [&Overview; 2],
    encoded: [&Encoded; 2],
    frames: [u16; 2],
) {
    let [a, b] = overviews;
    let x0 = a.rect[0].max(b.rect[0]);
    let y0 = a.rect[1].max(b.rect[1]);
    let x1 = a.rect[2].min(b.rect[2]);
    let y1 = a.rect[3].min(b.rect[3]);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let free = |x: usize, y: usize| {
        let label = labels[y * grid_width + x];
        (label == frames[0] || label == frames[1]) && a.covers(x, y) && b.covers(x, y)
    };
    // The region's bounding box, grown by one pixel of anchors.
    let mut bounds = [usize::MAX, usize::MAX, 0, 0];
    for y in y0..y1 {
        for x in x0..x1 {
            if free(x, y) {
                bounds = [
                    bounds[0].min(x),
                    bounds[1].min(y),
                    bounds[2].max(x + 1),
                    bounds[3].max(y + 1),
                ];
            }
        }
    }
    if bounds[0] == usize::MAX {
        return;
    }
    let grid_height = labels.len() / grid_width;
    let rx0 = bounds[0].saturating_sub(1);
    let ry0 = bounds[1].saturating_sub(1);
    let rx1 = (bounds[2] + 1).min(grid_width);
    let ry1 = (bounds[3] + 1).min(grid_height);
    let rw = rx1 - rx0;
    let node = |x: usize, y: usize| (y - ry0) * rw + (x - rx0);
    let mut graph = Graph::new(rw * (ry1 - ry0), 2 * rw * (ry1 - ry0));
    // Which pixels take part: free ones and anchors owned by either frame.
    let mut role = vec![Role::Outside; rw * (ry1 - ry0)];
    for y in ry0..ry1 {
        for x in rx0..rx1 {
            let label = labels[y * grid_width + x];
            let n = node(x, y);
            role[n] = if free(x, y) {
                Role::Free
            } else if label == frames[0] {
                Role::First
            } else if label == frames[1] {
                Role::Second
            } else {
                Role::Outside
            };
            match role[n] {
                Role::Free | Role::Outside => {}
                Role::First => graph.add_terminal_weights(n, TERMINAL_COST, 0.0),
                Role::Second => graph.add_terminal_weights(n, 0.0, TERMINAL_COST),
            }
        }
    }
    let sample = |e: &Encoded, o: &Overview, x: usize, y: usize| {
        o.index(x, y).map_or([0.0; 3], |i| e.values[i])
    };
    let covered = |x: usize, y: usize| a.covers(x, y) && b.covers(x, y);
    for y in ry0..ry1 {
        for x in rx0..rx1 {
            if role[node(x, y)] == Role::Outside {
                continue;
            }
            for (nx, ny) in [(x + 1, y), (x, y + 1)] {
                if nx >= rx1 || ny >= ry1 || role[node(nx, ny)] == Role::Outside {
                    continue;
                }
                let (pa, pb) = (sample(encoded[0], a, x, y), sample(encoded[1], b, x, y));
                let (qa, qb) = (sample(encoded[0], a, nx, ny), sample(encoded[1], b, nx, ny));
                let mut weight = distance(pa, pb) + distance(qa, qb) + WEIGHT_EPS;
                if !covered(x, y) || !covered(nx, ny) {
                    weight += BAD_REGION_PENALTY;
                }
                graph.add_edges(node(x, y), node(nx, ny), weight, weight);
            }
        }
    }
    graph.max_flow();
    for y in ry0..ry1 {
        for x in rx0..rx1 {
            if role[node(x, y)] == Role::Free {
                labels[y * grid_width + x] = if graph.in_source_segment(node(x, y)) {
                    frames[0]
                } else {
                    frames[1]
                };
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Free,
    First,
    Second,
    Outside,
}
