//! Crop handles: hit testing, cursors and aspect-constrained dragging.

use super::*;

pub(in crate::ui::preview) fn crop_handle_points(rect: Rect) -> [Pos2; 8] {
    [
        rect.left_top(),
        rect.right_top(),
        rect.left_bottom(),
        rect.right_bottom(),
        Pos2::new(rect.center().x, rect.top()),
        Pos2::new(rect.center().x, rect.bottom()),
        Pos2::new(rect.left(), rect.center().y),
        Pos2::new(rect.right(), rect.center().y),
    ]
}

/// The crop handle under `pointer`: corners first, then anywhere along an
/// edge, then the interior. Grab zones shrink on small crops so the interior
/// stays reachable for moving.
pub(in crate::ui::preview) fn crop_handle_at(
    rect: Rect,
    pointer: Pos2,
    radius: f32,
) -> Option<CropHandle> {
    let radius = radius.min(rect.width().min(rect.height()) * 0.3).max(6.0);
    let corners = [
        (CropHandle::TopLeft, rect.left_top()),
        (CropHandle::TopRight, rect.right_top()),
        (CropHandle::BottomLeft, rect.left_bottom()),
        (CropHandle::BottomRight, rect.right_bottom()),
    ];
    if let Some((handle, _)) = corners
        .into_iter()
        .map(|(handle, point)| (handle, point.distance(pointer)))
        .filter(|(_, distance)| *distance <= radius)
        .min_by(|a, b| a.1.total_cmp(&b.1))
    {
        return Some(handle);
    }
    let within_x = (rect.left()..=rect.right()).contains(&pointer.x);
    let within_y = (rect.top()..=rect.bottom()).contains(&pointer.y);
    let edges = [
        (CropHandle::Top, (pointer.y - rect.top()).abs(), within_x),
        (
            CropHandle::Bottom,
            (pointer.y - rect.bottom()).abs(),
            within_x,
        ),
        (CropHandle::Left, (pointer.x - rect.left()).abs(), within_y),
        (
            CropHandle::Right,
            (pointer.x - rect.right()).abs(),
            within_y,
        ),
    ];
    if let Some((handle, _, _)) = edges
        .into_iter()
        .filter(|(_, distance, within)| *within && *distance <= radius)
        .min_by(|a, b| a.1.total_cmp(&b.1))
    {
        return Some(handle);
    }
    rect.contains(pointer).then_some(CropHandle::Move)
}

/// The pointer shape for hovering or dragging a crop handle on screen.
pub(in crate::ui::preview) fn crop_handle_cursor(handle: CropHandle) -> egui::CursorIcon {
    match handle {
        CropHandle::Move => egui::CursorIcon::Move,
        CropHandle::Left | CropHandle::Right => egui::CursorIcon::ResizeHorizontal,
        CropHandle::Top | CropHandle::Bottom => egui::CursorIcon::ResizeVertical,
        CropHandle::TopLeft | CropHandle::BottomRight => egui::CursorIcon::ResizeNwSe,
        CropHandle::TopRight | CropHandle::BottomLeft => egui::CursorIcon::ResizeNeSw,
    }
}

pub(in crate::ui::preview) fn sanitize_dragged_crop(
    mut crop: [f32; 4],
    handle: CropHandle,
) -> [f32; 4] {
    let min = GeometryTransform::MIN_CROP_EXTENT;
    match handle {
        CropHandle::Left | CropHandle::TopLeft | CropHandle::BottomLeft => {
            crop[0] = crop[0].clamp(0.0, crop[2] - min);
        }
        CropHandle::Right | CropHandle::TopRight | CropHandle::BottomRight => {
            crop[2] = crop[2].clamp(crop[0] + min, 1.0);
        }
        _ => {}
    }
    match handle {
        CropHandle::Top | CropHandle::TopLeft | CropHandle::TopRight => {
            crop[1] = crop[1].clamp(0.0, crop[3] - min);
        }
        CropHandle::Bottom | CropHandle::BottomLeft | CropHandle::BottomRight => {
            crop[3] = crop[3].clamp(crop[1] + min, 1.0);
        }
        _ => {}
    }
    crop
}

pub(in crate::ui::preview) fn is_crop_corner(handle: CropHandle) -> bool {
    matches!(
        handle,
        CropHandle::TopLeft
            | CropHandle::TopRight
            | CropHandle::BottomLeft
            | CropHandle::BottomRight
    )
}

pub(in crate::ui::preview) fn constrain_crop_corner_aspect(
    app: &CalibRawApp,
    original_crop: [f32; 4],
    pointer: [f32; 2],
    handle: CropHandle,
) -> Option<[f32; 4]> {
    let raw = app.develop.loaded_raw.as_ref()?;
    let normalized_ratio = app
        .develop
        .geometry
        .normalized_crop_aspect(raw.width, raw.height)?;

    let (anchor_x, anchor_y, x_sign, y_sign) = match handle {
        CropHandle::TopLeft => (original_crop[2], original_crop[3], -1.0, -1.0),
        CropHandle::TopRight => (original_crop[0], original_crop[3], 1.0, -1.0),
        CropHandle::BottomLeft => (original_crop[2], original_crop[1], -1.0, 1.0),
        CropHandle::BottomRight => (original_crop[0], original_crop[1], 1.0, 1.0),
        _ => return None,
    };

    let desired_width = (pointer[0] - anchor_x).abs();
    let desired_height = (pointer[1] - anchor_y).abs();

    let inv_ratio = 1.0 / normalized_ratio;
    let projected_width =
        (desired_width + desired_height * inv_ratio) / (1.0 + inv_ratio * inv_ratio);

    let max_width_from_x = if x_sign < 0.0 {
        anchor_x
    } else {
        1.0 - anchor_x
    };
    let max_height_from_y = if y_sign < 0.0 {
        anchor_y
    } else {
        1.0 - anchor_y
    };
    let max_width = max_width_from_x.min(max_height_from_y * normalized_ratio);

    let min_extent = crate::pipeline::GeometryTransform::MIN_CROP_EXTENT;
    let min_width = min_extent.max(min_extent * normalized_ratio);
    let width = projected_width.clamp(min_width.min(max_width), max_width);
    let height = width / normalized_ratio;

    let dragged_x = anchor_x + x_sign * width;
    let dragged_y = anchor_y + y_sign * height;
    Some(match handle {
        CropHandle::TopLeft => [dragged_x, dragged_y, anchor_x, anchor_y],
        CropHandle::TopRight => [anchor_x, dragged_y, dragged_x, anchor_y],
        CropHandle::BottomLeft => [dragged_x, anchor_y, anchor_x, dragged_y],
        CropHandle::BottomRight => [anchor_x, anchor_y, dragged_x, dragged_y],
        _ => return None,
    })
}

pub(in crate::ui::preview) fn constrain_crop_aspect(
    app: &CalibRawApp,
    mut crop: [f32; 4],
    handle: CropHandle,
) -> [f32; 4] {
    let Some(normalized_ratio) = app.develop.loaded_raw.as_ref().and_then(|raw| {
        app.develop
            .geometry
            .normalized_crop_aspect(raw.width, raw.height)
    }) else {
        return crop;
    };
    let width = crop[2] - crop[0];
    let height = crop[3] - crop[1];
    let target_height = width / normalized_ratio;
    let target_width = height * normalized_ratio;

    let horizontal_edge = matches!(handle, CropHandle::Left | CropHandle::Right);
    if horizontal_edge
        || (target_height <= 1.0 && (target_height - height).abs() <= (target_width - width).abs())
    {
        let new_height =
            target_height.clamp(crate::pipeline::GeometryTransform::MIN_CROP_EXTENT, 1.0);
        let center = (crop[1] + crop[3]) * 0.5;
        crop[1] = (center - new_height * 0.5).clamp(0.0, 1.0 - new_height);
        crop[3] = crop[1] + new_height;
    } else {
        let new_width =
            target_width.clamp(crate::pipeline::GeometryTransform::MIN_CROP_EXTENT, 1.0);
        let center = (crop[0] + crop[2]) * 0.5;
        crop[0] = (center - new_width * 0.5).clamp(0.0, 1.0 - new_width);
        crop[2] = crop[0] + new_width;
    }
    crop
}
