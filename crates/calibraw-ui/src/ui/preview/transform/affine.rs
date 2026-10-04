//! Affine geometry transforms: rotation, quarter turns and crop placement.

use super::*;

pub(in crate::ui::preview) fn geometry_forward_affine(
    geometry: GeometryTransform,
    dx: f32,
    dy: f32,
) -> [f32; 2] {
    let geometry = geometry.sanitized();
    let fx = if geometry.flip_horizontal { -1.0 } else { 1.0 };
    let fy = if geometry.flip_vertical { -1.0 } else { 1.0 };
    let shx = geometry.horizontal_transform.to_radians().tan();
    let shy = geometry.vertical_transform.to_radians().tan();
    let angle = geometry.rotation_degrees.to_radians();
    let c = angle.cos();
    let s = angle.sin();

    let flipped_x = dx * fx;
    let flipped_y = dy * fy;
    let sheared_x = flipped_x + shx * flipped_y;
    let sheared_y = shy * flipped_x + flipped_y;
    [c * sheared_x - s * sheared_y, s * sheared_x + c * sheared_y]
}

pub(in crate::ui::preview) fn geometry_inverse_affine(
    geometry: GeometryTransform,
    dx: f32,
    dy: f32,
) -> [f32; 2] {
    let geometry = geometry.sanitized();
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

pub(in crate::ui::preview) fn quarter_rotate_delta(
    quarter_turns: u8,
    dx: f32,
    dy: f32,
) -> [f32; 2] {
    match quarter_turns % 4 {
        0 => [dx, dy],
        1 => [-dy, dx],
        2 => [-dx, -dy],
        _ => [dy, -dx],
    }
}

pub(in crate::ui::preview) fn quarter_unrotate_delta(
    quarter_turns: u8,
    dx: f32,
    dy: f32,
) -> [f32; 2] {
    match quarter_turns % 4 {
        0 => [dx, dy],
        1 => [dy, -dx],
        2 => [-dx, -dy],
        _ => [-dy, dx],
    }
}

pub(in crate::ui::preview) fn geometry_forward_linear(
    geometry: GeometryTransform,
    dx: f32,
    dy: f32,
) -> [f32; 2] {
    let affine = geometry_forward_affine(geometry, dx, dy);
    quarter_rotate_delta(geometry.quarter_turns, affine[0], affine[1])
}

pub(in crate::ui::preview) fn geometry_inverse_linear(
    geometry: GeometryTransform,
    dx: f32,
    dy: f32,
) -> [f32; 2] {
    let affine = quarter_unrotate_delta(geometry.quarter_turns, dx, dy);
    geometry_inverse_affine(geometry, affine[0], affine[1])
}

pub(in crate::ui::preview) fn quarter_rotate_image_point(
    quarter_turns: u8,
    source_width: f32,
    source_height: f32,
    point: [f32; 2],
) -> [f32; 2] {
    match quarter_turns % 4 {
        0 => point,
        1 => [source_height - point[1], point[0]],
        2 => [source_width - point[0], source_height - point[1]],
        _ => [point[1], source_width - point[0]],
    }
}

pub(in crate::ui::preview) fn quarter_unrotate_image_point(
    quarter_turns: u8,
    source_width: f32,
    source_height: f32,
    point: [f32; 2],
) -> [f32; 2] {
    match quarter_turns % 4 {
        0 => point,
        1 => [point[1], source_height - point[0]],
        2 => [source_width - point[0], source_height - point[1]],
        _ => [source_width - point[1], point[0]],
    }
}

pub(in crate::ui::preview) fn geometry_crop_metrics(
    geometry: GeometryTransform,
    source_width: u32,
    source_height: u32,
) -> ([f32; 2], [f32; 2]) {
    let geometry = geometry.sanitized();
    let source_width = source_width.max(1) as f32;
    let source_height = source_height.max(1) as f32;
    let crop = geometry.crop;
    (
        [
            (crop[0] + crop[2]) * 0.5 * source_width,
            (crop[1] + crop[3]) * 0.5 * source_height,
        ],
        [
            (crop[2] - crop[0]) * source_width,
            (crop[3] - crop[1]) * source_height,
        ],
    )
}

pub(in crate::ui::preview) fn final_geometry_source_to_screen(
    image_rect: Rect,
    geometry: GeometryTransform,
    source_width: u32,
    source_height: u32,
    source_uv: [f32; 2],
) -> Pos2 {
    let geometry = geometry.sanitized();
    let ([center_x, center_y], [crop_width, crop_height]) =
        geometry_crop_metrics(geometry, source_width, source_height);
    let source_x = source_uv[0] * source_width.max(1) as f32;
    let source_y = source_uv[1] * source_height.max(1) as f32;
    let transformed = geometry_forward_linear(geometry, source_x - center_x, source_y - center_y);
    let (output_width, output_height) = if geometry.quarter_turns.is_multiple_of(2) {
        (crop_width, crop_height)
    } else {
        (crop_height, crop_width)
    };
    let output_uv = [
        0.5 + transformed[0] / output_width.max(f32::EPSILON),
        0.5 + transformed[1] / output_height.max(f32::EPSILON),
    ];
    normalized_to_screen(image_rect, output_uv)
}

pub(in crate::ui::preview) fn final_geometry_screen_to_source(
    image_rect: Rect,
    geometry: GeometryTransform,
    source_width: u32,
    source_height: u32,
    screen: Pos2,
) -> [f32; 2] {
    let geometry = geometry.sanitized();
    let ([center_x, center_y], [crop_width, crop_height]) =
        geometry_crop_metrics(geometry, source_width, source_height);
    let output_uv = screen_to_normalized_unclamped(image_rect, screen);
    let (output_width, output_height) = if geometry.quarter_turns.is_multiple_of(2) {
        (crop_width, crop_height)
    } else {
        (crop_height, crop_width)
    };
    let output_dx = (output_uv[0] - 0.5) * output_width;
    let output_dy = (output_uv[1] - 0.5) * output_height;
    let source_delta = geometry_inverse_linear(geometry, output_dx, output_dy);
    [
        (center_x + source_delta[0]) / source_width.max(1) as f32,
        (center_y + source_delta[1]) / source_height.max(1) as f32,
    ]
}
