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

use super::mask_refine::guided_filter;
use anyhow::Result;

/// Filter radius in model pixels: enough to reach across the enlarged blocks
/// and the model's misplacement of an edge.
const RADIUS_MODEL_PIXELS: f32 = 1.5;
/// Regularization in squared luminance units: luminance contrast well below
/// this (about 3% of the range) is treated as texture, not as an edge.
const EPSILON: f32 = 1e-3;

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
    let model_pixel = width as f32 / model_width as f32;
    let radius = (RADIUS_MODEL_PIXELS * model_pixel).round().max(1.0) as u32;
    guided_filter(photo_rgba, depth, width, height, radius, EPSILON)?;
    for value in depth {
        *value = value.clamp(0.0, 1.0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{imageops::FilterType, ImageBuffer, Luma};

    const MODEL_WIDTH: u32 = 16;
    const MODEL_HEIGHT: u32 = 8;
    const WIDTH: u32 = 128;
    const HEIGHT: u32 = 64;

    /// A model prediction that varies only across, enlarged like `restore_depth`.
    fn enlarged(depth: impl Fn(u32) -> f32) -> Vec<f32> {
        let model: Vec<f32> = (0..MODEL_WIDTH * MODEL_HEIGHT)
            .map(|i| depth(i % MODEL_WIDTH))
            .collect();
        let image = ImageBuffer::<Luma<f32>, _>::from_raw(MODEL_WIDTH, MODEL_HEIGHT, model)
            .expect("model image");
        image::imageops::resize(&image, WIDTH, HEIGHT, FilterType::Triangle).into_raw()
    }

    /// A photo whose grey level is `level(x)` in column `x`.
    fn photo(level: impl Fn(u32) -> u8) -> Vec<u8> {
        (0..WIDTH * HEIGHT)
            .flat_map(|i| {
                let value = level(i % WIDTH);
                [value, value, value, 255]
            })
            .collect()
    }

    fn aligned(mut depth: Vec<f32>, photo: &[u8]) -> Vec<f32> {
        align_depth_edges(&mut depth, photo, WIDTH, HEIGHT, MODEL_WIDTH).expect("aligned");
        depth
    }

    fn row(depth: &[f32]) -> &[f32] {
        let start = (HEIGHT / 2 * WIDTH) as usize;
        &depth[start..start + WIDTH as usize]
    }

    #[test]
    fn a_depth_step_jumps_where_the_photo_has_its_edge() {
        // The model's step lies between model columns 7 and 8 (photo column
        // 64), smeared over a model pixel by the enlargement. The photo's edge
        // lies six pixels to either side of it.
        let step = enlarged(|x| if x < 8 { 0.2 } else { 0.8 });
        for edge in [58, 70] {
            let depth = aligned(step.clone(), &photo(|x| if x < edge { 40 } else { 200 }));
            let depth = row(&depth);
            let steepest = (40..88)
                .max_by(|&a, &b| {
                    let rise = |x: usize| depth[x] - depth[x - 1];
                    rise(a).total_cmp(&rise(b))
                })
                .expect("columns");
            assert_eq!(steepest, edge as usize, "photo edge {edge}: {depth:?}");
            assert!(
                depth[edge as usize] - depth[edge as usize - 1] > 0.2,
                "photo edge {edge}: the jump is weak: {depth:?}"
            );
        }
    }

    #[test]
    fn slopes_keep_their_depth() {
        let slope = enlarged(|x| 0.1 + 0.05 * x as f32);
        let depth = aligned(slope.clone(), &photo(|_| 120));
        // Away from the frame, where the filter window is complete.
        for x in 24..(WIDTH - 24) as usize {
            assert!(
                (row(&depth)[x] - row(&slope)[x]).abs() < 1e-3,
                "column {x}: {} became {}",
                row(&slope)[x],
                row(&depth)[x]
            );
        }
    }

    #[test]
    fn flat_depth_ignores_photo_texture() {
        let flat = enlarged(|_| 0.4);
        let depth = aligned(flat, &photo(|x| if x % 2 == 0 { 20 } else { 230 }));
        assert!(depth.iter().all(|value| (value - 0.4).abs() < 1e-5));
    }

    #[test]
    fn rejects_mismatched_dimensions() {
        let mut depth = vec![0.0; 4];
        assert!(align_depth_edges(&mut depth, &[0; 16], 3, 3, MODEL_WIDTH).is_err());
        assert!(align_depth_edges(&mut depth, &[0; 16], 2, 2, 0).is_err());
    }
}
