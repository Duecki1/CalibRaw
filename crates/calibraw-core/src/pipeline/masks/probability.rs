//! Grow and Feather. Soft coverage (generated and range masks) keeps its values: Grow
//! moves its levels, and Feather only softens its edges. Both start from the untouched
//! coverage at 0 and depend on nothing but image distances. Drawn paths are binary, so
//! they shape a signed distance to their outline instead.

use super::box_filter::{gaussian_blur, gaussian_reach};
use super::morphology::offset_coverage;
use super::*;

/// Slider values at or below this leave a mask untouched.
const SLIDER_EPSILON: f32 = 1e-5;
/// Sigma of the blur that hides a contour's teeth from a fade, per pixel of
/// [`mask_feather_radius`].
const OUTLINE_SMOOTHING_PER_RADIUS: f32 = 0.25;

pub(super) fn chamfer_distance(binary: &[u8], width: usize, height: usize, target: u8) -> Vec<f32> {
    const INF: f32 = 1.0e20;
    const DIAGONAL: f32 = std::f32::consts::SQRT_2;
    let mut distance = binary
        .iter()
        .map(|value| if *value == target { 0.0 } else { INF })
        .collect::<Vec<_>>();

    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            let mut best = distance[index];
            if x > 0 {
                best = best.min(distance[index - 1] + 1.0);
            }
            if y > 0 {
                best = best.min(distance[index - width] + 1.0);
                if x > 0 {
                    best = best.min(distance[index - width - 1] + DIAGONAL);
                }
                if x + 1 < width {
                    best = best.min(distance[index - width + 1] + DIAGONAL);
                }
            }
            distance[index] = best;
        }
    }
    for y in (0..height).rev() {
        for x in (0..width).rev() {
            let index = y * width + x;
            let mut best = distance[index];
            if x + 1 < width {
                best = best.min(distance[index + 1] + 1.0);
            }
            if y + 1 < height {
                best = best.min(distance[index + width] + 1.0);
                if x > 0 {
                    best = best.min(distance[index + width - 1] + DIAGONAL);
                }
                if x + 1 < width {
                    best = best.min(distance[index + width + 1] + DIAGONAL);
                }
            }
            distance[index] = best;
        }
    }
    distance
}

/// Raster pixels that a Grow of `grow` moves an edge, inwards when negative, on a
/// raster with a shorter edge of `short_edge` pixels.
pub(super) fn mask_grow_radius(short_edge: f32, grow: f32) -> f32 {
    grow * short_edge * 0.05
}

pub(super) fn mask_feather_radius(short_edge: f32, feather: f32) -> f32 {
    feather.max(0.0).powf(1.30) * short_edge * 0.045
}

/// Pixels around a pixel that decide its soft coverage after Grow and Feather.
pub(super) fn soft_shape_margin_pixels(short_edge: f32, grow: f32, feather: f32) -> f32 {
    let radius = mask_feather_radius(short_edge, feather);
    mask_grow_radius(short_edge, grow.abs())
        + radius
        + gaussian_reach(OUTLINE_SMOOTHING_PER_RADIUS * radius) as f32
        + 2.0
}

/// Moves the levels of soft `coverage` by a Grow of `grow`.
pub(super) fn grow_soft_coverage(coverage: &mut [f32], width: u32, height: u32, grow: f32) {
    let grow = grow.clamp(-32.0, 32.0);
    if grow.abs() > SLIDER_EPSILON {
        offset_coverage(
            coverage,
            width as usize,
            height as usize,
            mask_grow_radius(width.min(height) as f32, grow),
        );
    }
}

/// Applies Grow, then Feather, to `coverage` sampled from the generated mask `source`.
pub(super) fn shape_generated_coverage(
    coverage: &mut [f32],
    width: u32,
    height: u32,
    grow: f32,
    feather: f32,
    source: &MaskImage,
) {
    let grow = grow.clamp(-32.0, 32.0);
    let feather = feather.clamp(0.0, 32.0);
    // Grow moves the outline by a known distance, so a drag of it reuses this contour.
    let outline = (feather > SLIDER_EPSILON)
        .then(|| {
            let binary = coverage
                .iter()
                .map(|value| u8::from(*value >= 0.5))
                .collect();
            raster_cache::prepared_contour(binary, width as usize, height as usize)
        })
        .flatten();
    grow_soft_coverage(coverage, width, height, grow);
    if let Some(outline) = outline {
        let edge = width.min(height) as f32;
        feather_edges(
            coverage,
            width as usize,
            height as usize,
            &outline.signed_distance,
            mask_grow_radius(edge, grow),
            mask_feather_radius(edge, feather),
            source,
        );
    }
    coverage
        .par_iter_mut()
        .for_each(|value| *value = value.clamp(0.0, 1.0));
}

/// Softens the edges of `coverage` over `radius` raster pixels to either side, and
/// nowhere else. The fade can only pull a pixel towards 50%, never past it, so the
/// outline stays where it is, soft areas of the matte that are already gentler keep
/// their values, and pixels away from an edge are not touched. `signed_distance`
/// measures from the outline before a Grow of `grow_radius` pixels.
fn feather_edges(
    coverage: &mut [f32],
    width: usize,
    height: usize,
    signed_distance: &[f32],
    grow_radius: f32,
    radius: f32,
    source: &MaskImage,
) {
    let sigma = OUTLINE_SMOOTHING_PER_RADIUS * radius;
    let mut smoothed = signed_distance.to_vec();
    gaussian_blur(&mut smoothed, width, height, sigma);
    let farthest = radius + gaussian_reach(sigma) as f32;
    let room = raster_cache::source_edge_room(source);
    let [u0, v0, u1, v1] = source.sampling_rect;
    let source_x_per_pixel = (u1 - u0) * source.width as f32 / width as f32;
    let source_y_per_pixel = (v1 - v0) * source.height as f32 / height as f32;
    let source_origin = [u0 * source.width as f32, v0 * source.height as f32];
    let grow_in_source = grow_radius * source_x_per_pixel;
    coverage
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            let source_y = source_origin[1] + (y as f32 + 0.5) * source_y_per_pixel;
            for (x, value) in row.iter_mut().enumerate() {
                let index = y * width + x;
                let distance = signed_distance[index] + grow_radius;
                if distance.abs() >= farthest {
                    continue;
                }
                let source_x = source_origin[0] + (x as f32 + 0.5) * source_x_per_pixel;
                let room = room.room.at(source_x, source_y, grow_in_source) / source_x_per_pixel;
                let room = room.clamp(0.0, radius);
                // Only a fade at full width can hide the teeth: smoothing would pull a
                // strand's narrow fade off the strand.
                let smoothing = smoothstep(0.5 * radius, radius, room);
                let distance = distance + (smoothed[index] - signed_distance[index]) * smoothing;
                let fade = smoothstep(-room, room, distance);
                *value = if *value >= 0.5 {
                    value.min(fade.max(0.5))
                } else {
                    value.max(fade.min(0.5))
                };
            }
        });
}

/// Shapes the binary coverage of a drawn path by its signed distance to the
/// outline. The outline stays the outer edge of a feather, which fades inwards.
pub(super) fn shape_path_coverage(
    mask: &mut [f32],
    width: u32,
    height: u32,
    grow: f32,
    feather: f32,
) {
    if width == 0 || height == 0 || mask.is_empty() {
        return;
    }
    let width = width as usize;
    let height = height as usize;
    let binary = mask
        .iter()
        .map(|value| u8::from(*value >= 0.5))
        .collect::<Vec<_>>();
    let Some(contour) = raster_cache::prepared_contour(binary, width, height) else {
        return;
    };
    let edge = width.min(height) as f32;
    let grow_radius = mask_grow_radius(edge, grow);
    let mut feather_radius = mask_feather_radius(edge, feather);
    if feather_radius > 0.0 {
        feather_radius = feather_radius.min(contour.deepest_inside * 0.8);
    }

    mask.par_iter_mut().enumerate().for_each(|(index, value)| {
        let confidence_offset = (*value - 0.5) * 0.5;
        let signed_distance = contour.signed_distance[index] + confidence_offset + grow_radius;
        *value = if feather_radius <= SLIDER_EPSILON {
            smoothstep(-0.75, 0.75, signed_distance)
        } else {
            smoothstep(0.0, feather_radius, signed_distance)
        };
    });
}
