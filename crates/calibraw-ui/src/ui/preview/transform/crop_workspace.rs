//! The crop workspace: mapping between the uncropped source and the screen, and clipping.

use super::*;

pub(in crate::ui::preview) fn editable_source_uv(uv: [f32; 2]) -> Option<[f32; 2]> {
    const EDGE_EPSILON: f32 = 1e-4;
    if !uv[0].is_finite()
        || !uv[1].is_finite()
        || uv[0] < -EDGE_EPSILON
        || uv[0] > 1.0 + EDGE_EPSILON
        || uv[1] < -EDGE_EPSILON
        || uv[1] > 1.0 + EDGE_EPSILON
    {
        return None;
    }
    Some([uv[0].clamp(0.0, 1.0), uv[1].clamp(0.0, 1.0)])
}

pub(in crate::ui::preview) fn crop_workspace_source_to_screen(
    image_rect: Rect,
    geometry: GeometryTransform,
    source_width: u32,
    source_height: u32,
    source_uv: [f32; 2],
) -> Pos2 {
    let geometry = geometry.sanitized();
    let ([center_x, center_y], _) = geometry_crop_metrics(geometry, source_width, source_height);
    let source_width_f = source_width.max(1) as f32;
    let source_height_f = source_height.max(1) as f32;
    let source_x = source_uv[0] * source_width_f;
    let source_y = source_uv[1] * source_height_f;
    let transformed = geometry_forward_affine(geometry, source_x - center_x, source_y - center_y);
    let pre_quarter = [center_x + transformed[0], center_y + transformed[1]];
    let canvas_point = quarter_rotate_image_point(
        geometry.quarter_turns,
        source_width_f,
        source_height_f,
        pre_quarter,
    );
    let (canvas_width, canvas_height) = if geometry.quarter_turns.is_multiple_of(2) {
        (source_width_f, source_height_f)
    } else {
        (source_height_f, source_width_f)
    };
    normalized_to_screen(
        image_rect,
        [
            canvas_point[0] / canvas_width,
            canvas_point[1] / canvas_height,
        ],
    )
}

pub(in crate::ui::preview) fn crop_workspace_screen_to_source(
    image_rect: Rect,
    geometry: GeometryTransform,
    source_width: u32,
    source_height: u32,
    screen: Pos2,
) -> [f32; 2] {
    let geometry = geometry.sanitized();
    let ([center_x, center_y], _) = geometry_crop_metrics(geometry, source_width, source_height);
    let source_width_f = source_width.max(1) as f32;
    let source_height_f = source_height.max(1) as f32;
    let (canvas_width, canvas_height) = if geometry.quarter_turns.is_multiple_of(2) {
        (source_width_f, source_height_f)
    } else {
        (source_height_f, source_width_f)
    };
    let canvas_uv = screen_to_normalized_unclamped(image_rect, screen);
    let canvas_point = [canvas_uv[0] * canvas_width, canvas_uv[1] * canvas_height];
    let pre_quarter = quarter_unrotate_image_point(
        geometry.quarter_turns,
        source_width_f,
        source_height_f,
        canvas_point,
    );
    let source_delta = geometry_inverse_affine(
        geometry,
        pre_quarter[0] - center_x,
        pre_quarter[1] - center_y,
    );
    [
        (center_x + source_delta[0]) / source_width_f,
        (center_y + source_delta[1]) / source_height_f,
    ]
}

pub(in crate::ui::preview) fn source_uv_bbox(
    points: impl IntoIterator<Item = [f32; 2]>,
) -> crate::app::PreviewUvRect {
    let mut min = [1.0_f32, 1.0_f32];
    let mut max = [0.0_f32, 0.0_f32];
    for point in points {
        min[0] = min[0].min(point[0]);
        min[1] = min[1].min(point[1]);
        max[0] = max[0].max(point[0]);
        max[1] = max[1].max(point[1]);
    }
    min[0] = min[0].clamp(0.0, 1.0);
    min[1] = min[1].clamp(0.0, 1.0);
    max[0] = max[0].clamp(0.0, 1.0);
    max[1] = max[1].clamp(0.0, 1.0);
    if max[0] <= min[0] {
        if min[0] >= 1.0 {
            min[0] = 1.0 - 1e-6;
            max[0] = 1.0;
        } else {
            max[0] = (min[0] + 1e-6).min(1.0);
        }
    }
    if max[1] <= min[1] {
        if min[1] >= 1.0 {
            min[1] = 1.0 - 1e-6;
            max[1] = 1.0;
        } else {
            max[1] = (min[1] + 1e-6).min(1.0);
        }
    }
    crate::app::PreviewUvRect { min, max }
}

pub(in crate::ui::preview) fn visible_rect_sample_points(rect: Rect, nonlinear: bool) -> Vec<Pos2> {
    let steps = if nonlinear { 10 } else { 1 };
    let mut points = Vec::with_capacity((steps + 1) * (steps + 1));
    for y in 0..=steps {
        let ty = y as f32 / steps as f32;
        for x in 0..=steps {
            let tx = x as f32 / steps as f32;
            points.push(Pos2::new(
                rect.left() + rect.width() * tx,
                rect.top() + rect.height() * ty,
            ));
        }
    }
    points
}

pub(in crate::ui::preview) fn crop_workspace_visible_source_uv(
    image_rect: Rect,
    visible_rect: Rect,
    geometry: GeometryTransform,
    lens_geometry: Option<&LensGeometryMap>,
    source_width: u32,
    source_height: u32,
) -> crate::app::PreviewUvRect {
    source_uv_bbox(
        visible_rect_sample_points(visible_rect, lens_geometry.is_some())
            .into_iter()
            .map(|point| {
                crop_workspace_screen_to_native_source(
                    image_rect,
                    geometry,
                    lens_geometry,
                    source_width,
                    source_height,
                    point,
                )
            }),
    )
}

pub(in crate::ui::preview) fn native_source_to_corrected_uv(
    lens_geometry: &LensGeometryMap,
    source_width: u32,
    source_height: u32,
    source_uv: [f32; 2],
) -> [f32; 2] {
    if source_uv[0] < 0.0 || source_uv[0] > 1.0 || source_uv[1] < 0.0 || source_uv[1] > 1.0 {
        return source_uv;
    }
    let width = source_width.saturating_sub(1).max(1) as f32;
    let height = source_height.saturating_sub(1).max(1) as f32;
    let corrected = lens_geometry.corrected_position_for_raster(
        source_uv[0] * width,
        source_uv[1] * height,
        source_width,
        source_height,
    );
    [corrected[0] / width, corrected[1] / height]
}

pub(in crate::ui::preview) fn crop_workspace_screen_to_native_source(
    image_rect: Rect,
    geometry: GeometryTransform,
    lens_geometry: Option<&LensGeometryMap>,
    source_width: u32,
    source_height: u32,
    screen: Pos2,
) -> [f32; 2] {
    let corrected_uv =
        crop_workspace_screen_to_source(image_rect, geometry, source_width, source_height, screen);
    corrected_uv_to_native_source(corrected_uv, lens_geometry, source_width, source_height)
}

pub(in crate::ui::preview) fn corrected_uv_to_native_source(
    corrected_uv: [f32; 2],
    lens_geometry: Option<&LensGeometryMap>,
    source_width: u32,
    source_height: u32,
) -> [f32; 2] {
    let Some(lens_geometry) = lens_geometry else {
        return corrected_uv;
    };
    if corrected_uv[0] < 0.0
        || corrected_uv[0] > 1.0
        || corrected_uv[1] < 0.0
        || corrected_uv[1] > 1.0
    {
        return corrected_uv;
    }
    let width = source_width.saturating_sub(1).max(1) as f32;
    let height = source_height.saturating_sub(1).max(1) as f32;
    let source = lens_geometry.source_position_for_raster(
        corrected_uv[0] * width,
        corrected_uv[1] * height,
        source_width,
        source_height,
    );
    [source[0] / width, source[1] / height]
}

pub(in crate::ui::preview) fn crop_preview_screen_rect(
    image_rect: Rect,
    geometry: GeometryTransform,
    source_width: u32,
    source_height: u32,
) -> Rect {
    let geometry = geometry.sanitized();
    let source_width_f = source_width.max(1) as f32;
    let source_height_f = source_height.max(1) as f32;
    let crop = geometry.crop;
    let crop_center = [
        (crop[0] + crop[2]) * 0.5 * source_width_f,
        (crop[1] + crop[3]) * 0.5 * source_height_f,
    ];
    let display_center = quarter_rotate_image_point(
        geometry.quarter_turns,
        source_width_f,
        source_height_f,
        crop_center,
    );
    let crop_width = (crop[2] - crop[0]) * source_width_f;
    let crop_height = (crop[3] - crop[1]) * source_height_f;
    let (canvas_width, canvas_height, display_width, display_height) =
        if geometry.quarter_turns.is_multiple_of(2) {
            (source_width_f, source_height_f, crop_width, crop_height)
        } else {
            (source_height_f, source_width_f, crop_height, crop_width)
        };
    let center = normalized_to_screen(
        image_rect,
        [
            display_center[0] / canvas_width,
            display_center[1] / canvas_height,
        ],
    );
    Rect::from_center_size(
        center,
        egui::vec2(
            display_width / canvas_width * image_rect.width(),
            display_height / canvas_height * image_rect.height(),
        ),
    )
}

pub(in crate::ui::preview) fn crop_source_handle_for_display(
    handle: CropHandle,
    quarter_turns: u8,
) -> CropHandle {
    use CropHandle::*;
    match quarter_turns % 4 {
        0 => handle,
        1 => match handle {
            TopLeft => BottomLeft,
            TopRight => TopLeft,
            BottomRight => TopRight,
            BottomLeft => BottomRight,
            Top => Left,
            Right => Top,
            Bottom => Right,
            Left => Bottom,
            Move => Move,
        },
        2 => match handle {
            TopLeft => BottomRight,
            TopRight => BottomLeft,
            BottomRight => TopLeft,
            BottomLeft => TopRight,
            Top => Bottom,
            Right => Left,
            Bottom => Top,
            Left => Right,
            Move => Move,
        },
        _ => match handle {
            TopLeft => TopRight,
            TopRight => BottomRight,
            BottomRight => BottomLeft,
            BottomLeft => TopLeft,
            Top => Right,
            Right => Bottom,
            Bottom => Left,
            Left => Top,
            Move => Move,
        },
    }
}

pub(in crate::ui::preview) fn crop_preview_pointer_to_source_normalized(
    image_rect: Rect,
    quarter_turns: u8,
    source_width: u32,
    source_height: u32,
    pointer: Pos2,
) -> [f32; 2] {
    let source_width_f = source_width.max(1) as f32;
    let source_height_f = source_height.max(1) as f32;
    let (canvas_width, canvas_height) = if quarter_turns.is_multiple_of(2) {
        (source_width_f, source_height_f)
    } else {
        (source_height_f, source_width_f)
    };
    let canvas_uv = screen_to_normalized_unclamped(image_rect, pointer);
    let source_point = quarter_unrotate_image_point(
        quarter_turns,
        source_width_f,
        source_height_f,
        [canvas_uv[0] * canvas_width, canvas_uv[1] * canvas_height],
    );
    [
        source_point[0] / source_width_f,
        source_point[1] / source_height_f,
    ]
}

pub(in crate::ui::preview) fn source_uv_inside_image(uv: [f32; 2]) -> bool {
    const EPSILON: f32 = 1e-4;
    uv[0].is_finite()
        && uv[1].is_finite()
        && uv[0] >= -EPSILON
        && uv[0] <= 1.0 + EPSILON
        && uv[1] >= -EPSILON
        && uv[1] <= 1.0 + EPSILON
}

pub(in crate::ui::preview) fn normalize_degrees(mut degrees: f32) -> f32 {
    while degrees > 180.0 {
        degrees -= 360.0;
    }
    while degrees <= -180.0 {
        degrees += 360.0;
    }
    degrees
}

pub(in crate::ui::preview) fn nearest_straight_axis_degrees(angle: f32) -> f32 {
    (angle / 90.0).round() * 90.0
}

pub(in crate::ui::preview) fn crop_workspace_image_polygon(
    image_rect: Rect,
    geometry: GeometryTransform,
    source_width: u32,
    source_height: u32,
) -> Vec<Pos2> {
    [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
        .into_iter()
        .map(|uv| {
            crop_workspace_source_to_screen(image_rect, geometry, source_width, source_height, uv)
        })
        .collect()
}

pub(in crate::ui::preview) fn clip_polygon_to_rect(polygon: &[Pos2], rect: Rect) -> Vec<Pos2> {
    fn clip_axis(
        input: &[Pos2],
        inside: impl Fn(Pos2) -> bool,
        intersect: impl Fn(Pos2, Pos2) -> Pos2,
    ) -> Vec<Pos2> {
        if input.is_empty() {
            return Vec::new();
        }
        let mut output = Vec::with_capacity(input.len() + 4);
        let mut previous = *input.last().unwrap();
        let mut previous_inside = inside(previous);
        for &current in input {
            let current_inside = inside(current);
            if current_inside != previous_inside {
                output.push(intersect(previous, current));
            }
            if current_inside {
                output.push(current);
            }
            previous = current;
            previous_inside = current_inside;
        }
        output
    }

    let mut output = polygon.to_vec();
    let left = rect.left();
    output = clip_axis(
        &output,
        |p| p.x >= left,
        |a, b| {
            let denom = b.x - a.x;
            let t = if denom.abs() <= f32::EPSILON {
                0.0
            } else {
                (left - a.x) / denom
            };
            Pos2::new(left, a.y + (b.y - a.y) * t)
        },
    );
    let right = rect.right();
    output = clip_axis(
        &output,
        |p| p.x <= right,
        |a, b| {
            let denom = b.x - a.x;
            let t = if denom.abs() <= f32::EPSILON {
                0.0
            } else {
                (right - a.x) / denom
            };
            Pos2::new(right, a.y + (b.y - a.y) * t)
        },
    );
    let top = rect.top();
    output = clip_axis(
        &output,
        |p| p.y >= top,
        |a, b| {
            let denom = b.y - a.y;
            let t = if denom.abs() <= f32::EPSILON {
                0.0
            } else {
                (top - a.y) / denom
            };
            Pos2::new(a.x + (b.x - a.x) * t, top)
        },
    );
    let bottom = rect.bottom();
    clip_axis(
        &output,
        |p| p.y <= bottom,
        |a, b| {
            let denom = b.y - a.y;
            let t = if denom.abs() <= f32::EPSILON {
                0.0
            } else {
                (bottom - a.y) / denom
            };
            Pos2::new(a.x + (b.x - a.x) * t, bottom)
        },
    )
}

pub(in crate::ui::preview) fn crop_rect_segments(rect: Rect) -> [(Pos2, Pos2); 4] {
    [
        (rect.left_top(), rect.right_top()),
        (rect.right_top(), rect.right_bottom()),
        (rect.right_bottom(), rect.left_bottom()),
        (rect.left_bottom(), rect.left_top()),
    ]
}

pub(in crate::ui::preview) fn liang_barsky_clip_test(
    p: f32,
    q: f32,
    t0: &mut f32,
    t1: &mut f32,
) -> bool {
    const CLIP_EPSILON: f32 = 1.0e-5;
    if p.abs() <= CLIP_EPSILON {
        return q >= -CLIP_EPSILON;
    }
    let r = q / p;
    if p < 0.0 {
        if r > *t1 + CLIP_EPSILON {
            return false;
        }
        if r > *t0 {
            *t0 = r;
        }
    } else {
        if r < *t0 - CLIP_EPSILON {
            return false;
        }
        if r < *t1 {
            *t1 = r;
        }
    }
    true
}

pub(in crate::ui::preview) fn clip_crop_workspace_segment_to_source_image(
    image_rect: Rect,
    geometry: GeometryTransform,
    source_width: u32,
    source_height: u32,
    a: Pos2,
    b: Pos2,
) -> Option<[Pos2; 2]> {
    let start =
        crop_workspace_screen_to_source(image_rect, geometry, source_width, source_height, a);
    let end = crop_workspace_screen_to_source(image_rect, geometry, source_width, source_height, b);
    let delta = [end[0] - start[0], end[1] - start[1]];
    let mut t0 = 0.0_f32;
    let mut t1 = 1.0_f32;
    if !liang_barsky_clip_test(-delta[0], start[0], &mut t0, &mut t1)
        || !liang_barsky_clip_test(delta[0], 1.0 - start[0], &mut t0, &mut t1)
        || !liang_barsky_clip_test(-delta[1], start[1], &mut t0, &mut t1)
        || !liang_barsky_clip_test(delta[1], 1.0 - start[1], &mut t0, &mut t1)
        || t1 + 1.0e-5 < t0
    {
        return None;
    }
    t0 = t0.clamp(0.0, 1.0);
    t1 = t1.clamp(t0, 1.0);
    let source_a = [start[0] + delta[0] * t0, start[1] + delta[1] * t0];
    let source_b = [start[0] + delta[0] * t1, start[1] + delta[1] * t1];
    let source_a = [source_a[0].clamp(0.0, 1.0), source_a[1].clamp(0.0, 1.0)];
    let source_b = [source_b[0].clamp(0.0, 1.0), source_b[1].clamp(0.0, 1.0)];
    Some([
        crop_workspace_source_to_screen(
            image_rect,
            geometry,
            source_width,
            source_height,
            source_a,
        ),
        crop_workspace_source_to_screen(
            image_rect,
            geometry,
            source_width,
            source_height,
            source_b,
        ),
    ])
}
