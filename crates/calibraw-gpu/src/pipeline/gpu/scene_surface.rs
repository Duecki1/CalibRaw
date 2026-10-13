//! Relighting surface derived from 8-bit scene depth.
//!
//! Scene depth is stored with 256 levels, so a gentle slope arrives as flat
//! terraces separated by one-level steps; normals taken from it directly would
//! be flat with a crease at every step. The surface is first dequantized into
//! a smooth surface within every sample's quantization interval (±half a
//! level), coarse to fine: area averages of wide footprints are already
//! smooth, and each finer level predicts itself by upsampling the coarser
//! solution. Only the correction that keeps the prediction inside the
//! intervals is smoothed, so planes and evenly curved forms such as faces
//! keep their shape, terraces become ramps, and real depth edges, many levels
//! high, cannot move by more than half a level.
//! Gradients are then taken one-sidedly across depth edges so a silhouette
//! never becomes a steep wall, and a mip chain averages them for broader
//! light sizes. Above level 0, the mip chain's first channel holds the
//! nearest smooth depth of its footprint instead, so a shadow ray that passes
//! in front of every surface near it can skip testing them (relight.wgsl).
//!
//! All values are normalized relative depth (near 0, far 1) in the full-image
//! coordinate frame of the square scene-depth texture. Gradients are depth per
//! length of the image's shorter edge, so they are isotropic in the image even
//! though the texture stretches the image to a square.
//!
//! Every step computes each texel from the previous step alone, so rows are
//! processed in parallel with results identical to a serial pass.

use rayon::prelude::*;

/// Mip levels of the scene-depth texture; the smallest is 16×16 at 1024.
pub(super) const SCENE_DEPTH_MIP_LEVELS: u32 = 7;

/// Half of one 8-bit quantization step: the largest error of a stored sample
/// and, being a convex combination of them, of a bilinear resample.
const QUANTIZATION_HALF_STEP: f32 = 0.5 / 255.0;
/// Range scale of the edge-aware smoothing in normalized depth: a one-level
/// step keeps 94 % of a neighbour's weight, an edge of a twentieth of the
/// depth range none.
const EDGE_RANGE: f32 = 0.016;
/// Correction smoothing steps per pyramid level, each projected into the
/// quantization intervals.
const PROJECTED_ITERATIONS: usize = 8;
/// Unconstrained steps that round the kinks the projection leaves where the
/// solution touches an interval bound, which would otherwise facet shading.
const FINISH_ITERATIONS: usize = 3;
/// An upsampled coarser solution this far from a sample straddled a depth
/// edge; on a smooth surface it stays within about half a step.
const UNRELIABLE_GUESS: f32 = 2.0 / 255.0;
/// The coarsest pyramid level; at 1024 one of its texels averages 128².
const COARSEST_SIDE: usize = 8;
/// A per-texel depth change above this is an occlusion edge, not a slope.
const DEPTH_EDGE_STEP: f32 = 0.02;

/// One mip level: per texel `[stored depth, smooth depth, ∂depth/∂x, ∂depth/∂y]`
/// on level 0, and `[nearest smooth depth, smooth depth, ∂depth/∂x, ∂depth/∂y]`
/// above it, where the nearest depth is the minimum over the texel's level-0
/// footprint and the others are means.
pub(super) struct SurfaceLevel {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) texels: Vec<[f32; 4]>,
}

/// Image extent of one texel in shorter-edge units, per axis.
fn texel_extent(edge: u32, aspect: f32) -> [f32; 2] {
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        1.0
    };
    let edge = edge as f32;
    if aspect >= 1.0 {
        [aspect / edge, 1.0 / edge]
    } else {
        [1.0 / edge, 1.0 / (aspect * edge)]
    }
}

/// Derives the relighting surface from `stored`, the `edge`×`edge` bilinear
/// resample of the stored depth. `aspect` is the image's width / height.
/// Level 0 keeps `stored` unchanged in its first channel.
pub(super) fn derive_scene_surface(stored: &[f32], edge: u32, aspect: f32) -> Vec<SurfaceLevel> {
    let side = edge as usize;
    debug_assert_eq!(stored.len(), side * side);
    let extent = texel_extent(edge, aspect);
    let smooth = dequantize(stored, side);

    let at = |x: usize, y: usize| smooth[y * side + x];
    let mut texels = vec![[0.0; 4]; side * side];
    texels
        .par_chunks_mut(side)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, texel) in row.iter_mut().enumerate() {
                let center = at(x, y);
                let gx = edge_aware_difference(
                    (x > 0).then(|| center - at(x - 1, y)),
                    (x + 1 < side).then(|| at(x + 1, y) - center),
                ) / extent[0];
                let gy = edge_aware_difference(
                    (y > 0).then(|| center - at(x, y - 1)),
                    (y + 1 < side).then(|| at(x, y + 1) - center),
                ) / extent[1];
                *texel = [stored[y * side + x], center, gx, gy];
            }
        });

    let mut levels = vec![SurfaceLevel {
        width: edge,
        height: edge,
        texels,
    }];
    while levels.len() < SCENE_DEPTH_MIP_LEVELS as usize {
        let finer = levels.last().expect("level 0 exists");
        // Level 0 holds stored depth first; above it, the nearest depth.
        let nearest = if levels.len() == 1 {
            SMOOTH_DEPTH
        } else {
            NEAREST_DEPTH
        };
        let next = downsample(finer, nearest);
        levels.push(next);
    }
    levels
}

/// Channel of the stored depth on level 0 and of the nearest depth above it.
const NEAREST_DEPTH: usize = 0;
/// Channel of the smooth depth.
const SMOOTH_DEPTH: usize = 1;

/// A per-texel depth difference that ignores occlusion edges: the mean of the
/// one-sided differences on a continuous surface, the side that stays on the
/// surface beside an edge, and flat between two edges (a texel the resample
/// spread across a silhouette, whose own slope is unknown).
fn edge_aware_difference(backward: Option<f32>, forward: Option<f32>) -> f32 {
    let usable = |difference: Option<f32>| difference.filter(|d| d.abs() <= DEPTH_EDGE_STEP);
    match (usable(backward), usable(forward)) {
        (Some(backward), Some(forward)) => 0.5 * (backward + forward),
        (Some(difference), None) | (None, Some(difference)) => difference,
        (None, None) => 0.0,
    }
}

/// Box-averages 2×2 texels. Gradients average to the mean slope of the larger
/// footprint, which is the shading a broader light integrates. The first
/// channel takes the minimum of the finer level's channel `nearest`.
fn downsample(level: &SurfaceLevel, nearest: usize) -> SurfaceLevel {
    let width = (level.width / 2).max(1);
    let height = (level.height / 2).max(1);
    let source_width = level.width as usize;
    let at = |x: u32, y: u32| {
        let x = x.min(level.width - 1) as usize;
        let y = y.min(level.height - 1) as usize;
        level.texels[y * source_width + x]
    };
    let mut texels = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let quad = [
                at(2 * x, 2 * y),
                at(2 * x + 1, 2 * y),
                at(2 * x, 2 * y + 1),
                at(2 * x + 1, 2 * y + 1),
            ];
            let mut texel: [f32; 4] = std::array::from_fn(|channel| {
                quad.iter().map(|texel| texel[channel]).sum::<f32>() * 0.25
            });
            texel[NEAREST_DEPTH] = quad
                .iter()
                .map(|texel| texel[nearest])
                .fold(f32::INFINITY, f32::min);
            texels.push(texel);
        }
    }
    SurfaceLevel {
        width,
        height,
        texels,
    }
}

/// A smooth surface within half a quantization step of `stored`
/// (`side`×`side`), approached coarse to fine.
fn dequantize(stored: &[f32], side: usize) -> Vec<f32> {
    if side <= COARSEST_SIDE || !side.is_multiple_of(2) {
        // Each texel averages a wide footprint; the steps are dithered away.
        return stored.to_vec();
    }
    let coarse_side = side / 2;
    // Averages of stored samples stay within half a step of the averaged
    // depth, so every level keeps the same interval constraint.
    let coarse = dequantize(&average_quads(stored, side), coarse_side);
    let mut prediction = upsample(&coarse, coarse_side);
    // Coarse texels straddling a depth edge blur it; there the prediction is
    // levels away from the samples and would leave a ramp beside the edge.
    for (value, &sample) in prediction.iter_mut().zip(stored) {
        if (*value - sample).abs() > UNRELIABLE_GUESS {
            *value = sample;
        }
    }
    let weights = NeighbourWeights::new(stored, side);
    let mut correction = vec![0.0; side * side];
    for iteration in 0..PROJECTED_ITERATIONS + FINISH_ITERATIONS {
        if iteration < PROJECTED_ITERATIONS {
            for ((correction, &prediction), &sample) in
                correction.iter_mut().zip(&prediction).zip(stored)
            {
                *correction = (prediction + *correction).clamp(
                    sample - QUANTIZATION_HALF_STEP,
                    sample + QUANTIZATION_HALF_STEP,
                ) - prediction;
            }
        }
        correction = weights.diffuse(&correction);
    }
    prediction
        .iter()
        .zip(&correction)
        .map(|(prediction, correction)| prediction + correction)
        .collect()
}

fn average_quads(values: &[f32], side: usize) -> Vec<f32> {
    let half = side / 2;
    let mut averaged = Vec::with_capacity(half * half);
    for y in 0..half {
        for x in 0..half {
            let top = 2 * y * side + 2 * x;
            let bottom = top + side;
            averaged
                .push(0.25 * (values[top] + values[top + 1] + values[bottom] + values[bottom + 1]));
        }
    }
    averaged
}

/// Bilinear upsampling to twice the side. The outer quarter texel is
/// extrapolated, so linear ramps stay linear up to the border.
fn upsample(coarse: &[f32], coarse_side: usize) -> Vec<f32> {
    let side = coarse_side * 2;
    let last = coarse_side.saturating_sub(1);
    let axis: Vec<(usize, usize, f32)> = (0..side)
        .map(|i| {
            let p = (i as f32 + 0.5) * 0.5 - 0.5;
            let lo = (p.floor().max(0.0) as usize).min(last.saturating_sub(1));
            (lo, (lo + 1).min(last), p - lo as f32)
        })
        .collect();
    let mut fine = Vec::with_capacity(side * side);
    for &(y0, y1, fy) in &axis {
        for &(x0, x1, fx) in &axis {
            let at = |x: usize, y: usize| coarse[y * coarse_side + x];
            let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * fx;
            let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * fx;
            fine.push(top + (bottom - top) * fy);
        }
    }
    fine
}

/// Edge-aware weights between 4-neighbours, from the stored depth.
struct NeighbourWeights {
    side: usize,
    /// Weight between a texel and its right neighbour; zero in the last column.
    right: Vec<f32>,
    /// Weight between a texel and the one below; zero in the last row.
    down: Vec<f32>,
}

impl NeighbourWeights {
    fn new(stored: &[f32], side: usize) -> Self {
        let weight = |a: f32, b: f32| {
            let difference = (a - b) / EDGE_RANGE;
            (-difference * difference).exp()
        };
        let mut right = vec![0.0; side * side];
        let mut down = vec![0.0; side * side];
        right
            .par_chunks_mut(side)
            .zip(down.par_chunks_mut(side))
            .enumerate()
            .for_each(|(y, (right, down))| {
                for x in 0..side {
                    let index = y * side + x;
                    if x + 1 < side {
                        right[x] = weight(stored[index], stored[index + 1]);
                    }
                    if y + 1 < side {
                        down[x] = weight(stored[index], stored[index + side]);
                    }
                }
            });
        Self { side, right, down }
    }

    /// One Jacobi step of edge-aware diffusion; the texel itself has weight 1.
    /// A neighbour beyond the border is the linear extrapolation through the
    /// opposite one, so slopes are not flattened toward the edges.
    fn diffuse(&self, values: &[f32]) -> Vec<f32> {
        let side = self.side;
        let mut diffused = vec![0.0; values.len()];
        diffused
            .par_chunks_mut(side)
            .enumerate()
            .for_each(|(y, row)| self.diffuse_row(values, y, row));
        diffused
    }

    /// One row of `diffuse`. Interior texels have all four neighbours and
    /// take the same sums in the same order as `diffuse_texel`, without its
    /// border cases.
    fn diffuse_row(&self, values: &[f32], y: usize, row: &mut [f32]) {
        let side = self.side;
        if y == 0 || y + 1 == side || side < 3 {
            for (x, diffused) in row.iter_mut().enumerate() {
                *diffused = self.diffuse_texel(values, x, y);
            }
            return;
        }
        row[0] = self.diffuse_texel(values, 0, y);
        row[side - 1] = self.diffuse_texel(values, side - 1, y);
        let start = y * side;
        for (x, diffused) in row.iter_mut().enumerate().take(side - 1).skip(1) {
            let index = start + x;
            let center = values[index];
            let mut sum = center;
            let mut weights = 1.0;
            let (left, right) = (self.right[index - 1], self.right[index]);
            sum += left * values[index - 1] + right * values[index + 1];
            weights += left + right;
            let (up, down) = (self.down[index - side], self.down[index]);
            sum += up * values[index - side] + down * values[index + side];
            weights += up + down;
            *diffused = sum / weights;
        }
    }

    /// One texel of `diffuse`, at any position.
    fn diffuse_texel(&self, values: &[f32], x: usize, y: usize) -> f32 {
        let side = self.side;
        let index = y * side + x;
        let center = values[index];
        let mut sum = center;
        let mut weights = 1.0;
        // (weight, value) toward lower and higher coordinates per axis.
        let axes = [
            (
                (x > 0).then(|| (self.right[index - 1], values[index - 1])),
                (x + 1 < side).then(|| (self.right[index], values[index + 1])),
            ),
            (
                (y > 0).then(|| (self.down[index - side], values[index - side])),
                (y + 1 < side).then(|| (self.down[index], values[index + side])),
            ),
        ];
        for axis in axes {
            let (lower, higher) = match axis {
                (Some(lower), Some(higher)) => (lower, higher),
                (Some((weight, value)), None) => ((weight, value), (weight, 2.0 * center - value)),
                (None, Some((weight, value))) => ((weight, 2.0 * center - value), (weight, value)),
                (None, None) => continue,
            };
            sum += lower.0 * lower.1 + higher.0 * higher.1;
            weights += lower.0 + higher.0;
        }
        sum / weights
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The production edge: smoothing and edge handling work in texels.
    const EDGE: u32 = 1024;

    fn quantized(edge: u32, depth: impl Fn(f32, f32) -> f32) -> Vec<f32> {
        (0..edge * edge)
            .map(|i| {
                let x = (i % edge) as f32 / (edge - 1) as f32;
                let y = (i / edge) as f32 / (edge - 1) as f32;
                (depth(x, y).clamp(0.0, 1.0) * 255.0).round() / 255.0
            })
            .collect()
    }

    fn channel(level: &SurfaceLevel, channel: usize) -> Vec<f32> {
        level.texels.iter().map(|texel| texel[channel]).collect()
    }

    #[test]
    fn levels_halve_down_to_the_mip_count_and_keep_stored_depth() {
        let stored = quantized(EDGE, |x, y| 0.2 + 0.3 * x + 0.1 * y);
        let levels = derive_scene_surface(&stored, EDGE, 1.5);
        assert_eq!(levels.len(), SCENE_DEPTH_MIP_LEVELS as usize);
        for (index, level) in levels.iter().enumerate() {
            assert_eq!(level.width, EDGE >> index);
            assert_eq!(level.height, EDGE >> index);
            assert_eq!(level.texels.len(), (level.width * level.height) as usize);
            assert!(level.texels.iter().flatten().all(|v| v.is_finite()));
        }
        assert_eq!(channel(&levels[0], 0), stored);
    }

    #[test]
    fn terraces_of_a_gentle_slope_become_a_ramp() {
        // Sixteen levels across the image: terraces 64 texels wide.
        let stored = quantized(EDGE, |x, _| 0.3 + 16.0 / 255.0 * x);
        let level = &derive_scene_surface(&stored, EDGE, 1.0)[0];
        let expected = 16.0 / 255.0;
        let interior = (16..EDGE as usize - 16)
            .flat_map(|y| (16..EDGE as usize - 16).map(move |x| (x, y)))
            .map(|(x, y)| level.texels[y * EDGE as usize + x]);
        for texel in interior {
            assert!(
                (texel[2] - expected).abs() < 0.15 * expected,
                "gradient {} far from {expected}",
                texel[2]
            );
            assert!(texel[3].abs() < 0.05 * expected, "{}", texel[3]);
            // The finishing steps may round a kink slightly past its interval.
            assert!((texel[1] - texel[0]).abs() <= QUANTIZATION_HALF_STEP + 0.25 / 255.0);
        }
    }

    #[test]
    fn curved_surfaces_keep_smooth_gradients_instead_of_facets() {
        // A bowl spanning 26 levels: its slope changes continuously, as on a
        // face. Errors are measured over 9×9 texels, the smallest footprint
        // Relight averages normals over.
        let side = EDGE as usize;
        let depth = |x: f32, y: f32| 0.3 + 0.4 * ((x - 0.5).powi(2) + (y - 0.5).powi(2));
        let stored = quantized(EDGE, depth);
        let smooth = &derive_scene_surface(&stored, EDGE, 1.0)[0];
        let raw: Vec<[f32; 4]> = (0..side * side)
            .map(|i| {
                let (x, y) = (i % side, i / side);
                let at = |x: usize| stored[y * side + x];
                let gx = if x > 0 && x + 1 < side {
                    (at(x + 1) - at(x - 1)) * 0.5 * EDGE as f32
                } else {
                    0.0
                };
                [0.0, 0.0, gx, 0.0]
            })
            .collect();
        let worst_error = |texels: &[[f32; 4]]| {
            let mut worst = 0.0_f32;
            for y in (64..side - 64).step_by(13) {
                for x in (64..side - 64).step_by(13) {
                    let mut mean = 0.0;
                    for dy in 0..9 {
                        for dx in 0..9 {
                            mean += texels[(y + dy - 4) * side + x + dx - 4][2] / 81.0;
                        }
                    }
                    let expected = 0.8 * (x as f32 / (side - 1) as f32 - 0.5);
                    worst = worst.max((mean - expected).abs());
                }
            }
            worst
        };
        let dequantized = worst_error(&smooth.texels);
        let quantized = worst_error(&raw);
        assert!(dequantized < 0.12, "worst gradient error {dequantized}");
        assert!(
            dequantized * 3.0 < quantized,
            "dequantized {dequantized} vs stored {quantized}"
        );
    }

    #[test]
    fn occlusion_edges_stay_in_place_without_steep_gradients() {
        let stored = quantized(EDGE, |x, _| if x < 0.5 { 0.1 } else { 0.8 });
        let levels = derive_scene_surface(&stored, EDGE, 1.0);
        for (texel, &sample) in levels[0].texels.iter().zip(&stored) {
            assert!((texel[1] - sample).abs() <= QUANTIZATION_HALF_STEP + 1e-6);
            assert!(texel[2].abs() < 1e-3 && texel[3].abs() < 1e-3, "{texel:?}");
        }
        for level in &levels[1..] {
            assert!(level.texels.iter().all(|t| t[2].abs() < 1e-3));
        }
    }

    #[test]
    fn gradients_are_measured_per_shorter_edge_in_both_orientations() {
        // A slope of 0.2 per image width: per shorter edge that is 0.2 / aspect
        // in landscape and 0.2 in portrait.
        let stored: Vec<f32> = (0..EDGE * EDGE)
            .map(|i| 0.4 + 0.2 * (i % EDGE) as f32 / EDGE as f32)
            .collect();
        for (aspect, expected) in [(2.0, 0.1), (0.5, 0.2)] {
            let level = &derive_scene_surface(&stored, EDGE, aspect)[0];
            let center = level.texels[(EDGE / 2 * EDGE + EDGE / 2) as usize];
            assert!((center[2] - expected).abs() < 0.01 * expected, "{center:?}");
        }
    }

    #[test]
    fn interior_diffusion_matches_the_general_texel_rule() {
        let side = 37;
        let stored = quantized(side as u32, |x, y| {
            0.5 + 0.3 * (9.0 * x).sin() * (7.0 * y).cos() + if x > 0.6 { 0.2 } else { 0.0 }
        });
        let weights = NeighbourWeights::new(&stored, side);
        let values: Vec<f32> = (0..side * side)
            .map(|i| ((i * 7919) % 101) as f32 / 101.0)
            .collect();
        let diffused = weights.diffuse(&values);
        for y in 0..side {
            for x in 0..side {
                assert_eq!(
                    diffused[y * side + x].to_bits(),
                    weights.diffuse_texel(&values, x, y).to_bits(),
                    "texel ({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn mips_hold_the_nearest_smooth_depth_of_their_footprint() {
        let stored = quantized(EDGE, |x, y| 0.5 + 0.3 * (9.0 * x).sin() * (5.0 * y).cos());
        let levels = derive_scene_surface(&stored, EDGE, 1.0);
        let smooth = channel(&levels[0], SMOOTH_DEPTH);
        for (index, level) in levels.iter().enumerate().skip(1) {
            let scale = 1 << index;
            for (x, y) in [(0, 0), (3, 5), (level.width - 1, level.height - 1)] {
                let nearest = (0..scale)
                    .flat_map(|dy| (0..scale).map(move |dx| (dx, dy)))
                    .map(|(dx, dy)| smooth[((y * scale + dy) * EDGE + x * scale + dx) as usize])
                    .fold(f32::INFINITY, f32::min);
                let texel = level.texels[(y * level.width + x) as usize];
                assert_eq!(texel[NEAREST_DEPTH], nearest, "level {index} ({x}, {y})");
                assert!(texel[NEAREST_DEPTH] <= texel[SMOOTH_DEPTH]);
            }
        }
    }

    #[test]
    fn mips_average_gradients_of_the_finer_level() {
        let stored = quantized(EDGE, |x, y| 0.5 + 0.2 * (6.0 * x).sin() * y);
        let levels = derive_scene_surface(&stored, EDGE, 1.0);
        let (fine, coarse) = (&levels[0], &levels[1]);
        let at = |x: usize, y: usize| fine.texels[y * EDGE as usize + x];
        for (x, y) in [(0, 0), (10, 20), (31, 63)] {
            let expected: f32 = [
                at(2 * x, 2 * y),
                at(2 * x + 1, 2 * y),
                at(2 * x, 2 * y + 1),
                at(2 * x + 1, 2 * y + 1),
            ]
            .iter()
            .map(|t| t[2])
            .sum::<f32>()
                * 0.25;
            let actual = coarse.texels[y * coarse.width as usize + x][2];
            assert!((actual - expected).abs() < 1e-5);
        }
    }
}
