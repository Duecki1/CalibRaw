use super::*;

mod affine;
mod crop_handles;
mod crop_workspace;
mod source_projection;
pub(super) use affine::*;
pub(super) use crop_handles::*;
pub(super) use crop_workspace::*;
pub(crate) use source_projection::*;

pub(super) fn fitted_image_size(available: egui::Vec2, image_aspect: f32) -> egui::Vec2 {
    let available_aspect = available.x / available.y.max(1.0);
    if available_aspect > image_aspect {
        egui::vec2(available.y * image_aspect, available.y)
    } else {
        egui::vec2(available.x, available.x / image_aspect.max(f32::EPSILON))
    }
}

pub(super) fn zoomed_image_rect(
    outer_rect: Rect,
    base_size: egui::Vec2,
    zoom: f32,
    center: [f32; 2],
) -> Rect {
    let size = base_size * zoom;
    let min = Pos2::new(
        outer_rect.center().x - center[0] * size.x,
        outer_rect.center().y - center[1] * size.y,
    );
    Rect::from_min_size(min, size)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn transform_preview_about_screen_points(
    outer_rect: Rect,
    current_image_rect: Rect,
    base_size: egui::Vec2,
    zoom: &mut f32,
    center: &mut [f32; 2],
    anchor_screen: Pos2,
    target_screen: Pos2,
    zoom_factor: f32,
) -> bool {
    if !zoom_factor.is_finite()
        || zoom_factor <= 0.0
        || !anchor_screen.is_finite()
        || !target_screen.is_finite()
    {
        return false;
    }
    let previous_zoom = *zoom;
    let previous_center = *center;
    let anchor_uv = [
        (anchor_screen.x - current_image_rect.left()) / current_image_rect.width().max(1.0),
        (anchor_screen.y - current_image_rect.top()) / current_image_rect.height().max(1.0),
    ];

    *zoom = (previous_zoom * zoom_factor).clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM);
    let new_size = base_size * *zoom;
    let new_min = Pos2::new(
        target_screen.x - anchor_uv[0] * new_size.x,
        target_screen.y - anchor_uv[1] * new_size.y,
    );
    *center = [
        (outer_rect.center().x - new_min.x) / new_size.x.max(1.0),
        (outer_rect.center().y - new_min.y) / new_size.y.max(1.0),
    ];
    clamp_preview_center(center, outer_rect.size(), new_size);

    (*zoom - previous_zoom).abs() > f32::EPSILON
        || (center[0] - previous_center[0]).abs() > f32::EPSILON
        || (center[1] - previous_center[1]).abs() > f32::EPSILON
}

pub(super) fn clamp_preview_center(center: &mut [f32; 2], viewport: egui::Vec2, image: egui::Vec2) {
    for (axis, center_axis) in center.iter_mut().enumerate() {
        let viewport_axis = if axis == 0 { viewport.x } else { viewport.y };
        let image_axis = if axis == 0 { image.x } else { image.y };
        if image_axis <= viewport_axis + 0.5 {
            *center_axis = 0.5;
        } else {
            let half_visible = (viewport_axis / (2.0 * image_axis)).clamp(0.0, 0.5);
            *center_axis = center_axis.clamp(half_visible, 1.0 - half_visible);
        }
    }
}

pub(super) fn preview_uv_changed(
    left: crate::app::PreviewUvRect,
    right: crate::app::PreviewUvRect,
) -> bool {
    left.min
        .into_iter()
        .chain(left.max)
        .zip(right.min.into_iter().chain(right.max))
        .any(|(left, right)| (left - right).abs() > 1e-6)
}

pub(super) fn screen_to_normalized_unclamped(rect: Rect, point: Pos2) -> [f32; 2] {
    [
        (point.x - rect.left()) / rect.width().max(1.0),
        (point.y - rect.top()) / rect.height().max(1.0),
    ]
}

pub(super) fn normalized_to_screen(rect: Rect, point: [f32; 2]) -> Pos2 {
    Pos2::new(
        rect.left() + point[0] * rect.width(),
        rect.top() + point[1] * rect.height(),
    )
}
