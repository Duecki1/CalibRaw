//! Keeping a rotated crop inside the transformed source image.

use super::*;

fn inverse_affine_corner_delta(geometry: GeometryTransform, dx: f32, dy: f32) -> [f32; 2] {
    let fx = if geometry.flip_horizontal { -1.0 } else { 1.0 };
    let fy = if geometry.flip_vertical { -1.0 } else { 1.0 };
    let shx = geometry.horizontal_transform.to_radians().tan();
    let shy = geometry.vertical_transform.to_radians().tan();
    let angle = geometry.rotation_degrees.to_radians();
    let c = angle.cos();
    let s = angle.sin();
    let a = c * fx - s * shy * fx;
    let b = c * shx * fy - s * fy;
    let c2 = s * fx + c * shy * fx;
    let d = s * shx * fy + c * fy;
    let determinant = a * d - b * c2;
    if determinant.abs() < 1e-6 {
        return [0.0, 0.0];
    }
    [
        (d * dx - b * dy) / determinant,
        (-c2 * dx + a * dy) / determinant,
    ]
}

fn transformed_crop_corner_deltas(
    geometry: GeometryTransform,
    crop_width: f32,
    crop_height: f32,
) -> [[f32; 2]; 4] {
    let half_width = crop_width * 0.5;
    let half_height = crop_height * 0.5;
    [
        inverse_affine_corner_delta(geometry, -half_width, -half_height),
        inverse_affine_corner_delta(geometry, half_width, -half_height),
        inverse_affine_corner_delta(geometry, half_width, half_height),
        inverse_affine_corner_delta(geometry, -half_width, half_height),
    ]
}

pub(super) fn feasible_crop_center_bounds(
    geometry: GeometryTransform,
    crop_width: f32,
    crop_height: f32,
    source_width: f32,
    source_height: f32,
) -> Option<([f32; 2], [f32; 2])> {
    let corners = transformed_crop_corner_deltas(geometry, crop_width, crop_height);
    let min_dx = corners
        .iter()
        .map(|point| point[0])
        .fold(f32::INFINITY, f32::min);
    let max_dx = corners
        .iter()
        .map(|point| point[0])
        .fold(f32::NEG_INFINITY, f32::max);
    let min_dy = corners
        .iter()
        .map(|point| point[1])
        .fold(f32::INFINITY, f32::min);
    let max_dy = corners
        .iter()
        .map(|point| point[1])
        .fold(f32::NEG_INFINITY, f32::max);
    let x_bounds = [-min_dx, source_width - max_dx];
    let y_bounds = [-min_dy, source_height - max_dy];
    if x_bounds[0] <= x_bounds[1] + 1e-4 && y_bounds[0] <= y_bounds[1] + 1e-4 {
        Some((x_bounds, y_bounds))
    } else {
        None
    }
}

pub(super) fn crop_fits_transformed_source(
    geometry: GeometryTransform,
    crop: [f32; 4],
    source_width: u32,
    source_height: u32,
) -> bool {
    let crop = sanitized_crop(crop);
    let source_width_f = source_width.max(1) as f32;
    let source_height_f = source_height.max(1) as f32;
    let crop_width = (crop[2] - crop[0]) * source_width_f;
    let crop_height = (crop[3] - crop[1]) * source_height_f;
    let center_x = (crop[0] + crop[2]) * 0.5 * source_width_f;
    let center_y = (crop[1] + crop[3]) * 0.5 * source_height_f;
    const EPSILON: f32 = 1e-3;
    transformed_crop_corner_deltas(geometry, crop_width, crop_height)
        .into_iter()
        .all(|delta| {
            let x = center_x + delta[0];
            let y = center_y + delta[1];
            x >= -EPSILON
                && x <= source_width_f + EPSILON
                && y >= -EPSILON
                && y <= source_height_f + EPSILON
        })
}
