//! The horizon of the ground in scene depth, for Wet Surfaces
//! (scene_analysis.wgsl).
//!
//! Distance, derived from stored depth, comes nearer toward the bottom of the image
//! over the ground, and how fast depends on the photo: a nearby object squeezes
//! the ground's range. So a surface counts as ground by the direction of its
//! gradient, with some slope at all, which walls (sideways) and surfaces facing
//! the camera (nearly none) do not have. The same rule marks ground in the scene map; here it tells
//! whether the photo has ground and how far up it reaches: walking from
//! the near edge of the image along each line toward the horizon until the
//! ground ends.
//!
//! The estimate depends only on the depth result and the direction of up, and
//! reads a coarse grid of block means, so it takes under a millisecond.

use super::*;

/// Grid cells along the depth's longer edge.
const GRID_LONG: usize = 96;
/// Cells on each side between which a gradient is measured.
const SPAN: usize = 2;
/// How much of the depth model's inverse-depth range lies beyond the farthest
/// surface: `SCENE_DISTANCE_INVERSE_SHIFT` in common.wgsl.
pub(super) const SCENE_DISTANCE_INVERSE_SHIFT: f32 = 0.115;
/// Distance at and beyond which nothing is ground: the sky and far scenery.
/// The end of `SCENE_GROUND_FAR` in scene_analysis.wgsl.
const FAR_DISTANCE: f32 = 0.77;
/// Slope in distance per shorter edge from which a surface can be ground, the
/// middle of `SCENE_GROUND_SLOPE` in scene_analysis.wgsl.
const MIN_SLOPE: f32 = 0.12;
/// Cosine of the angle from up within which a gradient can be ground, about 28
/// degrees, the middle of `SCENE_GROUND_ALIGNMENT` in scene_analysis.wgsl.
const MIN_ALIGNMENT: f32 = 0.89;
/// Cells a walk toward the horizon may cross off the ground, such as a thin rail.
const MAX_GAP: usize = 3;
/// Which share of the lines' reach, from the lowest, gives the horizon. A line
/// across an open stretch of ground reaches farther than one ending at an object
/// standing on it.
const REACH_SHARE: f32 = 0.85;
/// Beyond the last ground that can be seen, in shorter edges: the horizon lies
/// above it.
const HORIZON_MARGIN: f32 = 0.02;
/// Share of the image's cells that must be ground for the photo to have any.
const MIN_GROUND_SHARE: f32 = 0.03;

/// The position along the unit vector `up`, in shorter-edge units from the
/// image's top-left corner, of the horizon of the ground in `depth`. `None`
/// when the photo shows no ground.
pub(super) fn estimate_ground_horizon(depth: &MaskImage, up: [f32; 2]) -> Option<f32> {
    if !scene_depth::valid_scene_depth(depth) {
        return None;
    }
    let long = depth.width.max(depth.height) as usize;
    let block = (long / GRID_LONG).max(1);
    let columns = depth.width as usize / block;
    let rows = depth.height as usize / block;
    if columns <= 2 * SPAN || rows <= 2 * SPAN {
        return None;
    }
    let distances: Vec<f32> = block_means(depth, block, columns, rows)
        .into_iter()
        .map(distance_from_depth)
        .collect();
    let at = |x: usize, y: usize| distances[y * columns + x];
    // Shorter-edge lengths per cell.
    let cell = block as f32 / depth.width.min(depth.height) as f32;
    let span = 2.0 * SPAN as f32 * cell;
    let mut ground = vec![false; columns * rows];
    let mut count = 0usize;
    for y in SPAN..rows - SPAN {
        for x in SPAN..columns - SPAN {
            if at(x, y) >= FAR_DISTANCE {
                continue;
            }
            let slope = [
                (at(x + SPAN, y) - at(x - SPAN, y)) / span,
                (at(x, y + SPAN) - at(x, y - SPAN)) / span,
            ];
            let magnitude = slope[0].hypot(slope[1]);
            if magnitude >= MIN_SLOPE
                && (slope[0] * up[0] + slope[1] * up[1]) >= MIN_ALIGNMENT * magnitude
            {
                ground[y * columns + x] = true;
                count += 1;
            }
        }
    }
    if (count as f32) < MIN_GROUND_SHARE * (columns * rows) as f32 {
        return None;
    }

    // Walk along parallel lines toward the horizon, starting at the far side of
    // the image from it.
    let across = [-up[1], up[0]];
    let center = [columns as f32 * 0.5, rows as f32 * 0.5];
    let half = 0.5 * columns.max(rows) as f32 * std::f32::consts::SQRT_2;
    let lines = columns.max(rows);
    let mut reaches = Vec::with_capacity(lines);
    for line in 0..lines {
        let offset = (line as f32 + 0.5) / lines as f32 * 2.0 * half - half;
        let mut gap = 0;
        let mut reach = None;
        let mut on_ground = false;
        for step in 0..2 * lines {
            let travel = step as f32 - half;
            let x = center[0] + across[0] * offset + up[0] * travel;
            let y = center[1] + across[1] * offset + up[1] * travel;
            if x < 0.0 || y < 0.0 || x >= columns as f32 || y >= rows as f32 {
                if on_ground {
                    break;
                }
                continue;
            }
            if ground[y as usize * columns + x as usize] {
                on_ground = true;
                gap = 0;
                reach = Some((x + 0.5) * cell * up[0] + (y + 0.5) * cell * up[1]);
            } else if on_ground {
                gap += 1;
                if gap > MAX_GAP {
                    break;
                }
            }
        }
        reaches.extend(reach);
    }
    if reaches.is_empty() {
        return None;
    }
    reaches.sort_unstable_by(f32::total_cmp);
    let index = ((reaches.len() - 1) as f32 * REACH_SHARE) as usize;
    Some(reaches[index] + HORIZON_MARGIN)
}

/// Relative distance of a depth: `scene_distance_from_depth` in common.wgsl.
fn distance_from_depth(depth: f32) -> f32 {
    depth * SCENE_DISTANCE_INVERSE_SHIFT / (1.0 - depth + SCENE_DISTANCE_INVERSE_SHIFT)
}

/// The depth's mean over blocks of `block` × `block` pixels, which also undoes
/// most of its 8-bit quantization.
fn block_means(depth: &MaskImage, block: usize, columns: usize, rows: usize) -> Vec<f32> {
    let width = depth.width as usize;
    let mut means = vec![0.0; columns * rows];
    // Every block's pixels would be slow to read at 4096 pixels and add
    // nothing: a few evenly spaced ones average the steps out.
    let stride = (block / 4).max(1);
    for y in 0..rows {
        for x in 0..columns {
            let mut sum = 0u32;
            let mut count = 0u32;
            for row in (y * block..(y + 1) * block).step_by(stride) {
                for column in (x * block..(x + 1) * block).step_by(stride) {
                    sum += u32::from(depth.pixels[row * width + column]);
                    count += 1;
                }
            }
            means[y * columns + x] = sum as f32 / (count as f32 * 255.0);
        }
    }
    means
}
