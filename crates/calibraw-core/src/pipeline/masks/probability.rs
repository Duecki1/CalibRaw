//! Turning binary or AI masks into feathered probability masks.

use super::*;

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

pub(super) fn shape_probability_mask(
    mask: &mut [f32],
    width: u32,
    height: u32,
    grow: f32,
    feather: f32,
) {
    shape_probability_mask_with_radius(mask, width, height, grow, feather, None);
}

pub(super) fn shape_probability_mask_from_source(
    coverage: &mut [f32],
    width: u32,
    height: u32,
    grow: f32,
    feather: f32,
    source: &MaskImage,
) {
    let needs_distance_feather = feather > 1e-5
        && (grow.abs() > 1e-5 || mask_feather_radius(width.min(height) as f32, feather) > 1.0);
    let core_radius = needs_distance_feather
        .then(|| source_mask_core_radius(source, width, height))
        .flatten()
        .map(|radius| {
            let inward_grow = grow.min(0.0) * width.min(height) as f32 * 0.05;
            (radius + inward_grow).max(0.5)
        });
    shape_probability_mask_with_radius(coverage, width, height, grow, feather, core_radius);
}

pub(super) fn source_mask_core_radius(source: &MaskImage, width: u32, height: u32) -> Option<f32> {
    if source.width == 0 || source.height == 0 {
        return None;
    }
    let sample_width = (source.sampling_rect[2] - source.sampling_rect[0])
        .abs()
        .max(1e-6);
    let sample_height = (source.sampling_rect[3] - source.sampling_rect[1])
        .abs()
        .max(1e-6);
    let scale_x = width as f32 / source.width as f32 / sample_width;
    let scale_y = height as f32 / source.height as f32 / sample_height;
    raster_cache::source_frontier(source).core_radius(scale_x, scale_y)
}

fn shape_probability_mask_with_radius(
    mask: &mut [f32],
    width: u32,
    height: u32,
    grow: f32,
    feather: f32,
    core_radius: Option<f32>,
) {
    if width == 0 || height == 0 || mask.is_empty() {
        return;
    }

    let grow = grow.clamp(-32.0, 32.0);
    let feather = feather.clamp(0.0, 32.0);
    if grow.abs() <= 1e-5 && feather <= 1e-5 {
        mask.par_iter_mut()
            .for_each(|value| *value = value.clamp(0.0, 1.0));
        return;
    }

    let raw_radius = mask_feather_radius(width.min(height) as f32, feather);
    // Fine feathers retain the generated matte's subpixel alpha. Blend into
    // contour feathering over a short interval so moving the slider never
    // makes a visible jump between the two methods.
    if grow.abs() <= 1e-5 && raw_radius < 3.0 {
        let original = (raw_radius > 1.0).then(|| mask.to_vec());
        blur_probability_mask(mask, width as usize, height as usize, raw_radius);
        if let Some(mut contoured) = original {
            shape_distance_mask(
                &mut contoured,
                width,
                height,
                grow,
                feather,
                false,
                core_radius,
            );
            let blend = smoothstep(1.0, 3.0, raw_radius);
            mask.par_iter_mut()
                .zip(contoured.into_par_iter())
                .for_each(|(blurred, contour)| {
                    *blurred += (contour - *blurred) * blend;
                });
        }
        return;
    }

    shape_distance_mask(mask, width, height, grow, feather, false, core_radius);
}

pub(super) fn mask_feather_radius(short_edge: f32, feather: f32) -> f32 {
    feather.max(0.0).powf(1.30) * short_edge * 0.045
}

// Both generated masks and drawn paths use the same signed-distance grow and
// feather widths. Paths keep their drawn outline as the outer feather edge.
pub(super) fn shape_distance_mask(
    mask: &mut [f32],
    width: u32,
    height: u32,
    grow: f32,
    feather: f32,
    feather_inside: bool,
    core_radius: Option<f32>,
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
    let grow_radius = grow * edge * 0.05;
    let mut feather_radius = mask_feather_radius(edge, feather);
    if feather_radius > 0.0 {
        feather_radius = feather_radius.min(core_radius.unwrap_or(contour.deepest_inside * 0.8));
    }

    mask.par_iter_mut().enumerate().for_each(|(index, value)| {
        let confidence_offset = (*value - 0.5) * 0.5;
        let signed_distance = contour.signed_distance[index] + confidence_offset + grow_radius;
        *value = if feather_radius <= 1e-5 {
            smoothstep(-0.75, 0.75, signed_distance)
        } else if feather_inside {
            smoothstep(0.0, feather_radius, signed_distance)
        } else {
            smoothstep(-feather_radius, feather_radius, signed_distance)
        };
    });
}

pub(super) fn blur_probability_mask(mask: &mut [f32], width: usize, height: usize, radius: f32) {
    if width == 0 || height == 0 || radius <= 1e-5 {
        return;
    }
    let selected = mask.iter().map(|value| *value >= 0.5).collect::<Vec<_>>();
    let integer = radius.floor() as usize;
    let fraction = radius - integer as f32;
    let mut horizontal = vec![0.0; mask.len()];

    for y in 0..height {
        let row = &mask[y * width..(y + 1) * width];
        let mut sum = row[..=integer.min(width - 1)].iter().sum::<f32>();
        for x in 0..width {
            let left = x.saturating_sub(integer);
            let right = (x + integer).min(width - 1);
            let mut weighted = sum;
            let mut weight = (right - left + 1) as f32;
            if fraction > 0.0 {
                if let Some(extra) = x.checked_sub(integer + 1) {
                    weighted += row[extra] * fraction;
                    weight += fraction;
                }
                if x + integer + 1 < width {
                    weighted += row[x + integer + 1] * fraction;
                    weight += fraction;
                }
            }
            horizontal[y * width + x] = weighted / weight;
            if x >= integer {
                sum -= row[x - integer];
            }
            if x + integer + 1 < width {
                sum += row[x + integer + 1];
            }
        }
    }

    for x in 0..width {
        let mut sum = (0..=integer.min(height - 1))
            .map(|y| horizontal[y * width + x])
            .sum::<f32>();
        for y in 0..height {
            let top = y.saturating_sub(integer);
            let bottom = (y + integer).min(height - 1);
            let mut weighted = sum;
            let mut weight = (bottom - top + 1) as f32;
            if fraction > 0.0 {
                if let Some(extra) = y.checked_sub(integer + 1) {
                    weighted += horizontal[extra * width + x] * fraction;
                    weight += fraction;
                }
                if y + integer + 1 < height {
                    weighted += horizontal[(y + integer + 1) * width + x] * fraction;
                    weight += fraction;
                }
            }
            mask[y * width + x] = (weighted / weight).clamp(0.0, 1.0);
            if y >= integer {
                sum -= horizontal[(y - integer) * width + x];
            }
            if y + integer + 1 < height {
                sum += horizontal[(y + integer + 1) * width + x];
            }
        }
    }
    mask.iter_mut().zip(selected).for_each(|(value, selected)| {
        if selected {
            *value = (value.max(0.5) + 1e-6).min(1.0);
        } else {
            *value = (value.min(0.5) - 1e-6).max(0.0);
        }
    });
}
