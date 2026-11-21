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
//! Unlike OpenCV, the cost compares the frames' colour difference less its local mean,
//! as far as the multi-band blend reaches: blending hides a difference that varies more
//! slowly than its coarsest band (shading, a slight exposure mismatch), but not an edge
//! the frames place apart. A seam run down a smooth ceiling the frames shade slightly
//! differently was otherwise pushed to cross the ceiling line wherever the shading
//! matched, though parallax had shifted the line there by ten-odd pixels, leaving a
//! visible step. Photomontage compares gradients for the same reason when its blend
//! removes smooth differences (Agarwala et al., "Interactive Digital Photomontage",
//! SIGGRAPH 2004).
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
/// `gains[k]`, for a blend whose coarsest band spans `blend_width` output pixels at
/// `output_scale` pixels per plane unit.
pub(super) fn find(
    grid: Grid,
    overviews: &[Overview],
    gains: &[f64],
    blend_width: f64,
    output_scale: f64,
) -> Seams {
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
    // Two box passes of this radius reach twice as far, as far as the coarsest band.
    let radius = (0.5 * blend_width * grid.scale / output_scale)
        .round()
        .max(1.0) as usize;
    for first in 0..overviews.len() {
        for second in first + 1..overviews.len() {
            cut_pair(
                &mut labels,
                w,
                radius,
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
/// `frames[1]` by a minimum cut, with the cost [`detail_differences`] gives for a local
/// mean over `radius`. Pixels around them owned by either frame anchor the cut to that
/// frame; pixels of other frames are left out.
fn cut_pair(
    labels: &mut [u16],
    grid_width: usize,
    radius: usize,
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
    let rh = ry1 - ry0;
    let differences: Vec<[f32; 3]> = (0..rw * rh)
        .map(|n| {
            let (x, y) = (rx0 + n % rw, ry0 + n / rw);
            let (pa, pb) = (sample(encoded[0], a, x, y), sample(encoded[1], b, x, y));
            [pa[0] - pb[0], pa[1] - pb[1], pa[2] - pb[2]]
        })
        .collect();
    let shared: Vec<bool> = (0..rw * rh)
        .map(|n| covered(rx0 + n % rw, ry0 + n / rw))
        .collect();
    let costs = detail_differences(rw, &differences, &shared, radius);
    for y in ry0..ry1 {
        for x in rx0..rx1 {
            if role[node(x, y)] == Role::Outside {
                continue;
            }
            for (nx, ny) in [(x + 1, y), (x, y + 1)] {
                if nx >= rx1 || ny >= ry1 || role[node(nx, ny)] == Role::Outside {
                    continue;
                }
                let mut weight = costs[node(x, y)] + costs[node(nx, ny)] + WEIGHT_EPS;
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

/// The length of each colour difference less the mean difference within `radius`
/// pixels (two box passes over the `width`-wide region's `shared` pixels, the ones
/// both frames cover). Pixels not shared keep their whole difference; the bad-region
/// penalty decides there.
fn detail_differences(
    width: usize,
    differences: &[[f32; 3]],
    shared: &[bool],
    radius: usize,
) -> Vec<f32> {
    // Weighted sums: the shared differences and, last, their count.
    let mut sums: Vec<[f64; 4]> = differences
        .iter()
        .zip(shared)
        .map(|(d, &s)| {
            if s {
                [f64::from(d[0]), f64::from(d[1]), f64::from(d[2]), 1.0]
            } else {
                [0.0; 4]
            }
        })
        .collect();
    for _ in 0..2 {
        sums = box_rows(width, &sums, radius);
        sums = box_columns(width, &sums, radius);
    }
    differences
        .par_iter()
        .zip(shared)
        .zip(&sums)
        .map(|((&d, &s), sum)| {
            if s && sum[3] > 0.0 {
                let mean = [0, 1, 2].map(|c| (sum[c] / sum[3]) as f32);
                distance(d, mean)
            } else {
                distance(d, [0.0; 3])
            }
        })
        .collect()
}

/// Sums of `values` over `2·radius + 1` samples along each row, clipped at the ends.
fn box_rows(width: usize, values: &[[f64; 4]], radius: usize) -> Vec<[f64; 4]> {
    let mut out = vec![[0.0; 4]; values.len()];
    out.par_chunks_mut(width)
        .zip(values.par_chunks(width))
        .for_each(|(out, row)| {
            let mut prefix = vec![[0.0f64; 4]; width + 1];
            for (i, value) in row.iter().enumerate() {
                prefix[i + 1] = std::array::from_fn(|c| prefix[i][c] + value[c]);
            }
            for (i, sum) in out.iter_mut().enumerate() {
                let (lo, hi) = (i.saturating_sub(radius), (i + radius + 1).min(width));
                *sum = std::array::from_fn(|c| prefix[hi][c] - prefix[lo][c]);
            }
        });
    out
}

/// Sums of `values` over `2·radius + 1` samples down each column, clipped at the ends.
fn box_columns(width: usize, values: &[[f64; 4]], radius: usize) -> Vec<[f64; 4]> {
    let height = values.len() / width;
    // Row prefix sums: `prefix[y]` holds the sum of rows above `y`.
    let mut prefix = vec![[0.0f64; 4]; (height + 1) * width];
    for y in 0..height {
        for x in 0..width {
            let above = prefix[y * width + x];
            let value = values[y * width + x];
            prefix[(y + 1) * width + x] = std::array::from_fn(|c| above[c] + value[c]);
        }
    }
    let mut out = vec![[0.0; 4]; values.len()];
    out.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        let (lo, hi) = (y.saturating_sub(radius), (y + radius + 1).min(height));
        for (x, sum) in row.iter_mut().enumerate() {
            let (top, bottom) = (prefix[lo * width + x], prefix[hi * width + x]);
            *sum = std::array::from_fn(|c| bottom[c] - top[c]);
        }
    });
    out
}
