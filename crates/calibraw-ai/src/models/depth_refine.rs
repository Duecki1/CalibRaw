//! Aligns scene-depth edges with the photograph's edges.
//!
//! The depth model predicts a few hundred pixels across. Enlarged to the
//! photo, its silhouettes are blocky and often a few photo pixels off the
//! photo's own edges, and effects that change steeply across depth (fog and
//! its light glow, relighting) trace them. A guided filter over about one and
//! a half model pixels, guided by the photo the depth was predicted from,
//! moves each depth edge onto the photo's edge where the colours on either
//! side differ, and otherwise softens the blocks into a smooth transition.
//! Smooth slopes and flat depth keep their values.
//!
//! The filter's output follows its guide linearly, steeply so beside a depth
//! edge, and would copy the photo's texture and noise beside every silhouette
//! into depth: a ragged fringe that Relight shades as a jagged edge. The guide
//! is therefore the photo's luminance with fine texture smoothed away first,
//! by an edge-preserving filter that keeps the outlines of objects.

use super::mask_refine::{guided_filter_by_luminance, luminance_guide, self_guided_filter};
use anyhow::Result;

/// Filter radius in model pixels: enough to reach across the enlarged blocks
/// and the model's misplacement of an edge.
const RADIUS_MODEL_PIXELS: f32 = 1.5;
/// Regularization in squared luminance units: luminance contrast well below
/// this (about 3% of the range) is treated as texture, not as an edge.
const EPSILON: f32 = 1e-3;
/// Radius in photo pixels of the texture-removing passes over the guide:
/// about the grain of gravel, foliage and sensor noise.
const TEXTURE_RADIUS: u32 = 4;
/// Variance in squared luminance units below which the guide's variation is
/// texture to remove: a standard deviation of 5% of the range.
const TEXTURE_EPSILON: f32 = 2.5e-3;
/// Texture-removing passes; each flattens what the previous one left.
const TEXTURE_PASSES: usize = 3;

/// Aligns the edges of `depth` (normalized, near 0 and far 1), enlarged to
/// `width` × `height` from a prediction `model_width` pixels across, with
/// `photo_rgba`, the photo it was predicted from at the same size.
pub(super) fn align_depth_edges(
    depth: &mut [f32],
    photo_rgba: &[u8],
    width: u32,
    height: u32,
    model_width: u32,
) -> Result<()> {
    anyhow::ensure!(model_width > 0, "depth model output has no width");
    anyhow::ensure!(
        photo_rgba.len() == width as usize * height as usize * 4,
        "depth-alignment photo dimensions mismatch"
    );
    let model_pixel = width as f32 / model_width as f32;
    let radius = (RADIUS_MODEL_PIXELS * model_pixel).round().max(1.0) as u32;
    let mut guide = luminance_guide(photo_rgba);
    for _ in 0..TEXTURE_PASSES {
        self_guided_filter(&mut guide, width, height, TEXTURE_RADIUS, TEXTURE_EPSILON)?;
    }
    guided_filter_by_luminance(&guide, depth, width, height, radius, EPSILON)?;
    for value in depth {
        *value = value.clamp(0.0, 1.0);
    }
    Ok(())
}
