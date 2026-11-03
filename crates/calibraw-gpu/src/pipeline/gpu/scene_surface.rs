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
//! Gradients are then taken one-sidedly across depth edges, sharp or blurred
//! into a ramp by the depth model, so a silhouette never becomes a steep wall
//! (or a band of one beside it), and a mip chain averages them for broader
//! light sizes. Above level 0, the mip chain's first channel holds the
//! nearest smooth depth of its footprint instead, so a shadow ray that passes
//! in front of every surface near it can skip testing them (relight.wgsl).
//!
//! All of that works on the generated map, whose levels are even and whose
//! thresholds below are in its units. The result is converted to relative distance
//! (`scene_depth`, near 0, far 1) before the mip chain, in the full-image
//! coordinate frame of the square scene-depth texture. Gradients are distance per
//! length of the image's shorter edge, so they are isotropic in the image even
//! though the texture stretches the image to a square.
//!
//! Every step computes each texel from the previous step alone, so rows are
//! processed in parallel with results identical to a serial pass.
//!
//! Depth predicted with surface normals (`SceneNormals`) takes its level-0
//! gradients from them instead (`model_gradients`): from 8-bit relative depth,
//! slopes come out tilted wherever the depth model's distances are, which
//! turns the ground toward the camera, while the normal model orients them
//! directly. The normals give each surface's direction; the scale that turns
//! them into gradients is fitted to the depth's own slopes, so gradients stay
//! consistent with the depth every reader compares them with.

use super::scene_depth::{distance_from_stored, distance_per_stored};
use rayon::prelude::*;

/// Half the scene camera's field of view as a tangent per shorter-edge length
/// (scene_lights.wgsl).
pub(super) const SCENE_FOCAL_TAN: f32 = 0.4;
/// The depth range, in units of the nearest surface's distance, of the scene
/// camera with the default Relief (`scene_camera` in scene_lights.wgsl, at
/// `SCENE_DEFAULT_RELIEF_CONTROL` 0.5): model gradients use its inverse when
/// too little of the depth is sloped to fit their scale.
pub(super) const DEFAULT_RELIEF: f32 = 2.283;
/// Bounds of the fitted scale of model gradients, the inverse of a depth
/// range: the Relief control's range (0.25 to 6) and somewhat beyond.
const MODEL_SCALE_RANGE: (f32, f32) = (0.1, 4.0);
/// The smallest cosine between a normal and the direction to the camera that
/// model gradients follow: steeper surfaces are seen edge-on, where a gradient
/// grows without bound.
const MIN_VIEW_COSINE: f32 = 0.2;
/// Texels whose model slope is at least this (squared, per shorter-edge
/// length) are sloped enough to fit the scale on.
const MIN_FIT_SLOPE_SQUARED: f32 = 0.05 * 0.05;
/// Texels nearer than this distance fit the scale; the far scenery and sky
/// have little slope left in 8-bit depth.
const MAX_FIT_DISTANCE: f32 = 0.9;
/// A distance change per texel above this marks a depth edge, whose texels do
/// not fit the scale.
const FIT_EDGE_STEP: f32 = 0.02;
/// Every this many texels along each axis is a fitting sample.
const FIT_STRIDE: usize = 4;
/// Fewer fitting samples than this keep the default scale.
const MIN_FIT_SAMPLES: usize = 1000;

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
/// Texels on each side of a texel within which a soft occlusion edge, a ramp
/// the depth model blurred a silhouette into, is recognized
/// (`line_differences`): about the widest such ramp.
const SOFT_EDGE_REACH: usize = 8;
/// Differences whose median gives the slope of the surface flanking a soft
/// edge on each side, beyond `SOFT_EDGE_REACH`.
const SOFT_EDGE_FLANK: usize = 8;
/// Depth change across 2 × `SOFT_EDGE_REACH` texels beyond what the flanking
/// slopes predict that marks a soft occlusion edge: about three 8-bit levels,
/// above the depth model's noise on continuous surfaces.
const SOFT_EDGE_EXCESS: f32 = 0.012;

/// One mip level, in distance: per texel `[stored depth, smooth depth, ∂depth/∂x,
/// ∂depth/∂y]` on level 0, and `[nearest smooth depth, smooth depth, ∂depth/∂x, ∂depth/∂y]`
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
/// resample of the stored depth, and `normals`, the same resample of the
/// x and y of the normals predicted with it (`SceneNormals`). `aspect` is the
/// image's width / height. Level 0 holds the distance of `stored` in its first
/// channel.
pub(super) fn derive_scene_surface(
    stored: &[f32],
    normals: Option<&[[f32; 2]]>,
    edge: u32,
    aspect: f32,
) -> Vec<SurfaceLevel> {
    let side = edge as usize;
    debug_assert_eq!(stored.len(), side * side);
    let extent = texel_extent(edge, aspect);
    let smooth = dequantize(stored, side);

    // Per texel along each axis; `columns` holds column x at index x.
    let rows: Vec<Vec<f32>> = smooth.par_chunks(side).map(line_differences).collect();
    let columns: Vec<Vec<f32>> = (0..side)
        .into_par_iter()
        .map(|x| {
            let column: Vec<f32> = (0..side).map(|y| smooth[y * side + x]).collect();
            line_differences(&column)
        })
        .collect();
    let mut texels = vec![[0.0; 4]; side * side];
    texels
        .par_chunks_mut(side)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, texel) in row.iter_mut().enumerate() {
                let index = y * side + x;
                let per_stored = distance_per_stored(smooth[index]);
                *texel = [
                    distance_from_stored(stored[index]),
                    distance_from_stored(smooth[index]),
                    rows[y][x] / extent[0] * per_stored,
                    columns[x][y] / extent[1] * per_stored,
                ];
            }
        });
    if let Some(normals) = normals {
        debug_assert_eq!(normals.len(), side * side);
        model_gradients(&mut texels, normals, side, extent);
    }

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

/// Replaces the gradients of level-0 `texels` (`side`×`side`, each `extent`
/// across) with those of the surfaces `normals` describe.
///
/// A surface with normal n in the scene camera (scene_lights.wgsl), whose
/// image point p lies on the ray L = (2 tan (p - centre), 1), has the depth
/// gradient (`relight_normal` inverted) g = q (k + d), with
/// q = -2 tan n.xy / (n · L), d the smooth distance and k the inverse of the
/// depth range the camera spans. q depends on the normal alone; k is fitted
/// as the weighted median that best turns q into the depth's own gradients,
/// on texels away from depth edges, the sky and far scenery.
fn model_gradients(texels: &mut [[f32; 4]], normals: &[[f32; 2]], side: usize, extent: [f32; 2]) {
    let image = [extent[0] * side as f32, extent[1] * side as f32];
    let slopes: Vec<[f32; 2]> = normals
        .par_iter()
        .enumerate()
        .map(|(index, normal)| {
            let point = [
                ((index % side) as f32 + 0.5) * extent[0],
                ((index / side) as f32 + 0.5) * extent[1],
            ];
            let ray = [
                2.0 * SCENE_FOCAL_TAN * (point[0] - 0.5 * image[0]),
                2.0 * SCENE_FOCAL_TAN * (point[1] - 0.5 * image[1]),
                1.0,
            ];
            let lateral = normal[0] * normal[0] + normal[1] * normal[1];
            let normal = if lateral > 1.0 {
                let length = lateral.sqrt();
                [normal[0] / length, normal[1] / length, 0.0]
            } else {
                [normal[0], normal[1], -(1.0 - lateral).sqrt()]
            };
            let ray_length = (ray[0] * ray[0] + ray[1] * ray[1] + 1.0).sqrt();
            let facing = -(normal[0] * ray[0] + normal[1] * ray[1] + normal[2]) / ray_length;
            let scale = 2.0 * SCENE_FOCAL_TAN / (ray_length * facing.max(MIN_VIEW_COSINE));
            [normal[0] * scale, normal[1] * scale]
        })
        .collect();

    let mut samples: Vec<(f32, f32)> = (0..side)
        .step_by(FIT_STRIDE)
        .flat_map(|y| (0..side).step_by(FIT_STRIDE).map(move |x| y * side + x))
        .filter_map(|index| {
            let [_, distance, gx, gy] = texels[index];
            let [qx, qy] = slopes[index];
            let weight = qx * qx + qy * qy;
            let edge = (gx * extent[0]).hypot(gy * extent[1]) > FIT_EDGE_STEP;
            (weight >= MIN_FIT_SLOPE_SQUARED && distance < MAX_FIT_DISTANCE && !edge)
                .then(|| ((gx * qx + gy * qy) / weight - distance, weight))
        })
        .collect();
    let scale = if samples.len() < MIN_FIT_SAMPLES {
        1.0 / DEFAULT_RELIEF
    } else {
        samples.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        let half = samples.iter().map(|sample| sample.1).sum::<f32>() * 0.5;
        let mut below = 0.0;
        samples
            .iter()
            .find(|sample| {
                below += sample.1;
                below >= half
            })
            .map_or(1.0 / DEFAULT_RELIEF, |sample| sample.0)
    }
    .clamp(MODEL_SCALE_RANGE.0, MODEL_SCALE_RANGE.1);
    texels
        .par_iter_mut()
        .zip(&slopes)
        .for_each(|(texel, slope)| {
            let reach = scale + texel[SMOOTH_DEPTH];
            texel[2] = slope[0] * reach;
            texel[3] = slope[1] * reach;
        });
}

/// Channel of the stored depth on level 0 and of the nearest depth above it.
const NEAREST_DEPTH: usize = 0;
/// Channel of the smooth depth.
const SMOOTH_DEPTH: usize = 1;

/// Per-texel depth differences along one row or column of smooth depth.
///
/// The depth model blurs silhouettes into ramps up to about a dozen texels
/// wide whose steps stay below `DEPTH_EDGE_STEP`; read as slopes, they turn
/// the surface beside every foreground edge away from or toward the light, a
/// dark or bright band that looks like a short shadow. Within
/// `SOFT_EDGE_REACH` texels of such a ramp, depth changes more than the
/// slopes flanking the window predict (their median differences, so one
/// ramp in a flank does not skew them); there a texel takes the slope of the
/// flank whose surface it lies on. Elsewhere, including on curved surfaces
/// and creases, whose flanks predict the change, `edge_aware_difference`
/// applies. Beyond the line's ends, depth continues with the median slope
/// at that end.
fn line_differences(line: &[f32]) -> Vec<f32> {
    let n = line.len();
    let local = |i: usize| {
        edge_aware_difference(
            (i > 0).then(|| line[i] - line[i - 1]),
            (i + 1 < n).then(|| line[i + 1] - line[i]),
        )
    };
    let pad = SOFT_EDGE_REACH + SOFT_EDGE_FLANK;
    if n <= SOFT_EDGE_FLANK {
        return (0..n).map(local).collect();
    }
    let start_slope = median_difference(&line[..=SOFT_EDGE_FLANK]);
    let end_slope = median_difference(&line[n - 1 - SOFT_EDGE_FLANK..]);
    // `line` extended by `pad` texels at both ends.
    let extended: Vec<f32> = (0..n + 2 * pad)
        .map(|e| {
            if e < pad {
                line[0] - (pad - e) as f32 * start_slope
            } else if e >= pad + n {
                line[n - 1] + (e + 1 - pad - n) as f32 * end_slope
            } else {
                line[e - pad]
            }
        })
        .collect();
    // flank[e]: median of the `SOFT_EDGE_FLANK` differences from extended
    // texel e onward.
    let flank: Vec<f32> = extended
        .windows(SOFT_EDGE_FLANK + 1)
        .map(median_difference)
        .collect();
    let reach = SOFT_EDGE_REACH as f32;
    (0..n)
        .map(|i| {
            let e = i + pad;
            let before = flank[e - pad];
            let after = flank[e + SOFT_EDGE_REACH];
            let near_before = extended[e - SOFT_EDGE_REACH];
            let near_after = extended[e + SOFT_EDGE_REACH];
            let excess = near_after - near_before - reach * (before + after);
            if excess.abs() <= SOFT_EDGE_EXCESS {
                return local(i);
            }
            // Blended by how close the texel lies to each flank's surface, so
            // texels across the ramp do not flip between the two slopes.
            let off_before = (line[i] - (near_before + reach * before)).abs();
            let off_after = (line[i] - (near_after - reach * after)).abs();
            let toward_before = off_after / (off_before + off_after).max(f32::MIN_POSITIVE);
            after + (before - after) * toward_before
        })
        .collect()
}

/// The median of the `SOFT_EDGE_FLANK` differences between consecutive
/// `values`, the mean of the middle two.
fn median_difference(values: &[f32]) -> f32 {
    let mut differences = [0.0_f32; SOFT_EDGE_FLANK];
    for (difference, pair) in differences.iter_mut().zip(values.windows(2)) {
        *difference = pair[1] - pair[0];
    }
    differences.sort_unstable_by(f32::total_cmp);
    0.5 * (differences[SOFT_EDGE_FLANK / 2 - 1] + differences[SOFT_EDGE_FLANK / 2])
}

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
