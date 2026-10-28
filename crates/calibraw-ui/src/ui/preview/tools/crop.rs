use super::super::*;
use crate::app::{CropTool, GuideDrag};
use crate::pipeline::PerspectiveGuide;

/// Screen distance at which a crop corner or edge can be grabbed.
const CROP_HANDLE_RADIUS: f32 = 28.0;
/// Shortest straighten line or guide, in points; a shorter drag is a tap, and a guide whose end
/// is dragged this close to its other end is removed.
const MIN_LINE_LENGTH: f32 = 12.0;
/// Most perspective guides at once, as other editors allow: two along each axis.
const MAX_PERSPECTIVE_GUIDES: usize = 4;

impl Preview {
    pub(in crate::ui::preview) fn handle_crop_interaction(
        ui: &mut Ui,
        app: &mut CalibRawApp,
        layout: PreviewLayout,
    ) {
        let PreviewLayout {
            image_rect,
            viewport_rect,
            source_width,
            source_height,
            ..
        } = layout;
        if image_rect.width() <= 1.0 || image_rect.height() <= 1.0 {
            return;
        }
        let pointer = ui.input(|input| input.pointer.interact_pos());
        let primary_pressed = ui.input(|input| input.pointer.primary_pressed())
            && pointer.is_some_and(|point| {
                viewport_rect.contains(point) && ui.ctx().layer_id_at(point) == Some(ui.layer_id())
            });
        let primary_down = ui.input(|input| input.pointer.primary_down());
        let primary_released = ui.input(|input| input.pointer.primary_released());
        let quarter_turns = app.develop.geometry.quarter_turns % 4;

        if app.develop_ui.crop_tool == CropTool::Guides {
            let secondary_clicked = ui.input(|input| input.pointer.secondary_clicked())
                && pointer.is_some_and(|point| {
                    viewport_rect.contains(point)
                        && ui.ctx().layer_id_at(point) == Some(ui.layer_id())
                });
            Self::handle_guide_interaction(
                ui,
                app,
                layout,
                pointer,
                [
                    primary_pressed,
                    primary_down,
                    primary_released,
                    secondary_clicked,
                ],
            );
            return;
        }

        if app.develop_ui.crop_tool == CropTool::Straighten {
            if primary_pressed {
                if let Some(pointer) = pointer.filter(|point| image_rect.contains(*point)) {
                    let uv = crop_workspace_screen_to_source(
                        image_rect,
                        app.develop.geometry,
                        source_width,
                        source_height,
                        pointer,
                    );
                    if source_uv_inside_image(uv) {
                        app.develop_ui.straighten_drag = Some(StraightenDragState {
                            start: pointer,
                            current: pointer,
                        });
                        app.develop_ui.crop_drag = None;
                    }
                }
            }
            if primary_down {
                // Only the line's angle matters, so its end may leave the
                // image; freezing it there would skew the measured angle.
                if let (Some(pointer), Some(drag)) =
                    (pointer, app.develop_ui.straighten_drag.as_mut())
                {
                    drag.current = pointer;
                }
            }
            if primary_released {
                if let Some(drag) = app.develop_ui.straighten_drag.take() {
                    let delta = drag.current - drag.start;
                    if delta.length() >= MIN_LINE_LENGTH {
                        // The rotation acts before the aspect stretch, so the line is measured
                        // unstretched.
                        let [stretch_x, stretch_y] =
                            app.develop.geometry.displayed_stretch_factors();
                        let angle = (delta.y / stretch_y)
                            .atan2(delta.x / stretch_x)
                            .to_degrees();
                        let target = nearest_straight_axis_degrees(angle);
                        let correction = normalize_degrees(target - angle);
                        let rotation = app.develop.geometry.rotation_degrees + correction;
                        app.set_straighten_rotation(rotation, source_width, source_height);
                        // One line levels the photo; the crop handles return.
                        // A tap too short to measure keeps the tool active.
                        app.develop_ui.crop_tool = CropTool::Crop;
                    }
                }
            } else if !primary_down {
                app.develop_ui.straighten_drag = None;
            }
            return;
        }

        let display_crop_rect = crop_preview_screen_rect(
            image_rect,
            app.develop.geometry,
            source_width,
            source_height,
        );
        let grabbable = |point: Pos2| {
            viewport_rect.contains(point) && image_rect.expand(CROP_HANDLE_RADIUS).contains(point)
        };
        if primary_pressed {
            if let Some(pointer) = pointer.filter(|point| grabbable(*point)) {
                if let Some(display_handle) =
                    crop_handle_at(display_crop_rect, pointer, CROP_HANDLE_RADIUS)
                {
                    let handle = crop_source_handle_for_display(display_handle, quarter_turns);
                    let start = crop_preview_pointer_to_workspace(
                        image_rect,
                        app.develop.geometry,
                        source_width,
                        source_height,
                        pointer,
                    );
                    app.develop_ui.crop_drag = Some(CropDragState {
                        handle,
                        start,
                        crop: app.develop.geometry.crop,
                    });
                }
            }
        }

        let cursor_handle = match app.develop_ui.crop_drag {
            Some(drag) => Some(crop_source_handle_for_display(
                drag.handle,
                (4 - quarter_turns) % 4,
            )),
            None => ui
                .input(|input| input.pointer.hover_pos())
                .filter(|point| {
                    grabbable(*point) && ui.ctx().layer_id_at(*point) == Some(ui.layer_id())
                })
                .and_then(|point| crop_handle_at(display_crop_rect, point, CROP_HANDLE_RADIUS)),
        };
        if let Some(handle) = cursor_handle {
            ui.ctx().set_cursor_icon(crop_handle_cursor(handle));
        }

        if primary_down {
            if let (Some(pointer), Some(drag)) = (pointer, app.develop_ui.crop_drag) {
                let current = crop_preview_pointer_to_workspace(
                    image_rect,
                    app.develop.geometry,
                    source_width,
                    source_height,
                    pointer,
                );
                let delta = [current[0] - drag.start[0], current[1] - drag.start[1]];
                let crop =
                    Self::dragged_crop(app, drag, current, delta, source_width, source_height);
                if crop != app.develop.geometry.crop {
                    app.develop.geometry.crop = crop;
                    app.develop_ui.crop_constraint_reference = Some(crop);
                    app.note_geometry_changed();
                }
            }
        }

        if primary_released || !primary_down {
            app.develop_ui.crop_drag = None;
        }
    }

    /// Draws perspective guides, moves their ends and removes them, refitting the correction to
    /// them after every change. `buttons` holds whether the primary button was pressed, is down
    /// or was released and whether a secondary click landed over the preview. A guide is removed
    /// by a secondary click on it or by dragging one of its ends onto the other.
    fn handle_guide_interaction(
        ui: &Ui,
        app: &mut CalibRawApp,
        layout: PreviewLayout,
        pointer: Option<Pos2>,
        [primary_pressed, primary_down, primary_released, secondary_clicked]: [bool; 4],
    ) {
        let screen_ends = guide_screen_ends(app, layout);
        let end_at = |point: Pos2| {
            screen_ends
                .iter()
                .enumerate()
                .flat_map(|(guide, ends)| (0..2).map(move |end| (guide, end, ends[end])))
                .map(|(guide, end, position)| (guide, end, position.distance(point)))
                .filter(|(_, _, distance)| *distance <= handles::POINT_REACH)
                .min_by(|a, b| a.2.total_cmp(&b.2))
                .map(|(guide, end, _)| (guide, end))
        };
        let to_source = |point: Pos2| {
            let uv = crop_workspace_screen_to_source(
                layout.image_rect,
                app.develop.geometry,
                layout.source_width,
                layout.source_height,
                point,
            );
            (uv[0].is_finite() && uv[1].is_finite()).then_some(uv)
        };

        if primary_pressed {
            app.develop_ui.guide_drag = pointer.and_then(|pointer| match end_at(pointer) {
                Some((guide, end)) => Some(GuideDrag::MoveEnd {
                    guide,
                    end,
                    current: pointer,
                }),
                None => (app.develop_ui.perspective_guides.len() < MAX_PERSPECTIVE_GUIDES
                    && layout.image_rect.contains(pointer)
                    && to_source(pointer).is_some_and(source_uv_inside_image))
                .then_some(GuideDrag::Draw {
                    start: pointer,
                    current: pointer,
                }),
            });
        }
        if primary_down {
            if let (
                Some(pointer),
                Some(GuideDrag::Draw { current, .. } | GuideDrag::MoveEnd { current, .. }),
            ) = (pointer, app.develop_ui.guide_drag.as_mut())
            {
                *current = pointer;
            }
        }
        if primary_released {
            let changed = match app.develop_ui.guide_drag.take() {
                Some(GuideDrag::Draw { start, current }) => {
                    let ends = (start.distance(current) >= MIN_LINE_LENGTH)
                        .then(|| to_source(start).zip(to_source(current)))
                        .flatten();
                    ends.map(|(start, end)| {
                        app.develop_ui
                            .perspective_guides
                            .push(PerspectiveGuide { start, end });
                    })
                    .is_some()
                }
                Some(GuideDrag::MoveEnd {
                    guide,
                    end,
                    current,
                }) => {
                    let other = screen_ends.get(guide).map(|ends| ends[1 - end]);
                    let guides = &mut app.develop_ui.perspective_guides;
                    match (other, to_source(current)) {
                        (Some(other), _) if other.distance(current) < MIN_LINE_LENGTH => {
                            guides.remove(guide);
                            true
                        }
                        (Some(_), Some(position)) => {
                            let moved = &mut guides[guide];
                            if end == 0 {
                                moved.start = position;
                            } else {
                                moved.end = position;
                            }
                            true
                        }
                        _ => false,
                    }
                }
                None => false,
            };
            if changed {
                app.apply_perspective_guides();
            }
        } else if !primary_down {
            app.develop_ui.guide_drag = None;
        }

        if secondary_clicked && app.develop_ui.guide_drag.is_none() {
            let hit = pointer.and_then(|pointer| {
                screen_ends
                    .iter()
                    .enumerate()
                    .map(|(guide, ends)| (guide, distance_to_segment(pointer, ends[0], ends[1])))
                    .filter(|(_, distance)| *distance <= handles::GUIDE_REACH)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(guide, _)| guide)
            });
            if let Some(guide) = hit {
                app.develop_ui.perspective_guides.remove(guide);
                app.apply_perspective_guides();
            }
        }

        let hovered_end = ui
            .input(|input| input.pointer.hover_pos())
            .filter(|point| ui.ctx().layer_id_at(*point) == Some(ui.layer_id()))
            .and_then(end_at);
        if matches!(app.develop_ui.guide_drag, Some(GuideDrag::MoveEnd { .. })) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if hovered_end.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
    }

    /// The crop for `drag` with the pointer at `current` (`delta` from where
    /// the drag started), kept inside the transformed source. The
    /// pointer positions are crop workspace coordinates; the handles follow
    /// them there, and the result is converted back to a stored crop.
    fn dragged_crop(
        app: &CalibRawApp,
        drag: CropDragState,
        current: [f32; 2],
        delta: [f32; 2],
        source_width: u32,
        source_height: u32,
    ) -> [f32; 4] {
        let geometry = app.develop.geometry;
        let start = crop_to_workspace(geometry, drag.crop, source_width, source_height);
        let mut crop = start;
        match drag.handle {
            CropHandle::Move => {
                for (edge, value) in crop.iter_mut().enumerate() {
                    *value += delta[edge % 2];
                }
                let moved = crop_from_workspace(geometry, crop, source_width, source_height);
                return geometry.constrain_crop_move_to_transformed_source(
                    drag.crop,
                    [moved[0] - drag.crop[0], moved[1] - drag.crop[1]],
                    source_width,
                    source_height,
                );
            }
            CropHandle::Left => crop[0] += delta[0],
            CropHandle::Right => crop[2] += delta[0],
            CropHandle::Top => crop[1] += delta[1],
            CropHandle::Bottom => crop[3] += delta[1],
            CropHandle::TopLeft => {
                crop[0] += delta[0];
                crop[1] += delta[1];
            }
            CropHandle::TopRight => {
                crop[2] += delta[0];
                crop[1] += delta[1];
            }
            CropHandle::BottomLeft => {
                crop[0] += delta[0];
                crop[3] += delta[1];
            }
            CropHandle::BottomRight => {
                crop[2] += delta[0];
                crop[3] += delta[1];
            }
        }
        crop = sanitize_dragged_crop(crop, drag.handle);
        let Some(normalized_ratio) = app
            .develop
            .loaded_raw
            .as_ref()
            .and_then(|raw| geometry.normalized_crop_aspect(raw.width, raw.height))
        else {
            return constrain_free_workspace_crop_resize(
                geometry,
                start,
                crop,
                source_width,
                source_height,
            );
        };
        crop = if is_crop_corner(drag.handle) {
            constrain_crop_corner_aspect(app, start, current, drag.handle).unwrap_or(crop)
        } else {
            constrain_crop_aspect(crop, drag.handle, normalized_ratio)
        };
        geometry.constrain_crop_drag_to_transformed_source(
            drag.crop,
            crop_from_workspace(geometry, crop, source_width, source_height),
            source_width,
            source_height,
        )
    }

    pub(in crate::ui::preview) fn paint_crop_overlay(
        ui: &mut Ui,
        app: &CalibRawApp,
        layout: PreviewLayout,
    ) {
        let PreviewLayout {
            image_rect,
            visible_rect,
            viewport_rect,
            source_width,
            source_height,
            ..
        } = layout;
        let painter = ui.painter_at(viewport_rect);
        let crop_rect = crop_preview_screen_rect(
            image_rect,
            app.develop.geometry,
            source_width,
            source_height,
        );
        let visible_crop = crop_rect.intersect(visible_rect);
        if visible_crop.width() <= 0.0 || visible_crop.height() <= 0.0 {
            return;
        }
        let image_polygon = crop_workspace_image_polygon(
            image_rect,
            app.develop.geometry,
            source_width,
            source_height,
        );
        let shade = Color32::from_black_alpha(150);
        for rect in [
            Rect::from_min_max(
                visible_rect.min,
                Pos2::new(visible_rect.right(), visible_crop.top()),
            ),
            Rect::from_min_max(
                Pos2::new(visible_rect.left(), visible_crop.bottom()),
                visible_rect.max,
            ),
            Rect::from_min_max(
                Pos2::new(visible_rect.left(), visible_crop.top()),
                Pos2::new(visible_crop.left(), visible_crop.bottom()),
            ),
            Rect::from_min_max(
                Pos2::new(visible_crop.right(), visible_crop.top()),
                Pos2::new(visible_rect.right(), visible_crop.bottom()),
            ),
        ] {
            let rect = rect.intersect(visible_rect);
            if rect.width() <= 0.0 || rect.height() <= 0.0 {
                continue;
            }
            let clipped = clip_polygon_to_rect(&image_polygon, rect);
            if clipped.len() >= 3 {
                painter.add(Shape::convex_polygon(
                    clipped,
                    shade,
                    Stroke::new(0.0, Color32::TRANSPARENT),
                ));
            }
        }

        let border_stroke = Stroke::new(2.0, Color32::WHITE);
        for (a, b) in crop_rect_segments(crop_rect) {
            if let Some([a, b]) = clip_crop_workspace_segment_to_source_image(
                image_rect,
                app.develop.geometry,
                source_width,
                source_height,
                a,
                b,
            ) {
                painter.line_segment([a, b], border_stroke);
            }
        }
        for fraction in [1.0 / 3.0, 2.0 / 3.0] {
            let x = egui::lerp(crop_rect.left()..=crop_rect.right(), fraction);
            let y = egui::lerp(crop_rect.top()..=crop_rect.bottom(), fraction);
            for (a, b) in [
                (
                    Pos2::new(x, crop_rect.top()),
                    Pos2::new(x, crop_rect.bottom()),
                ),
                (
                    Pos2::new(crop_rect.left(), y),
                    Pos2::new(crop_rect.right(), y),
                ),
            ] {
                if let Some([a, b]) = clip_crop_workspace_segment_to_source_image(
                    image_rect,
                    app.develop.geometry,
                    source_width,
                    source_height,
                    a,
                    b,
                ) {
                    painter.line_segment([a, b], Stroke::new(1.0, Color32::from_white_alpha(115)));
                }
            }
        }

        for point in crop_handle_points(crop_rect) {
            let uv = crop_workspace_screen_to_source(
                image_rect,
                app.develop.geometry,
                source_width,
                source_height,
                point,
            );
            if source_uv_inside_image(uv) {
                handles::paint_point(&painter, point, 5.5, Color32::WHITE);
            }
        }

        let mut lines: Vec<[Pos2; 2]> = Vec::new();
        if let Some(line) = app.develop_ui.straighten_drag {
            lines.push([line.start, line.current]);
        }
        if app.develop_ui.crop_tool == CropTool::Guides {
            lines.extend(guide_screen_ends(app, layout));
            match app.develop_ui.guide_drag {
                Some(GuideDrag::Draw { start, current }) => lines.push([start, current]),
                Some(GuideDrag::MoveEnd {
                    guide,
                    end,
                    current,
                }) => {
                    if let Some(ends) = lines.get_mut(guide) {
                        ends[end] = current;
                    }
                }
                None => {}
            }
        }
        let stroke = Stroke::new(2.0, Color32::WHITE);
        for ends in lines {
            painter.line_segment(ends, stroke);
            for end in ends {
                handles::paint_point(
                    &painter,
                    end,
                    handles::SECONDARY_POINT_RADIUS,
                    Color32::WHITE,
                );
            }
        }
    }
}

/// Where each perspective guide's ends are on screen, in the order of the guides.
fn guide_screen_ends(app: &CalibRawApp, layout: PreviewLayout) -> Vec<[Pos2; 2]> {
    let mapping = GeometryMapping::new(
        app.develop.geometry,
        layout.source_width,
        layout.source_height,
    );
    let canvas = mapping.canvas_uv_from_source_uv();
    app.develop_ui
        .perspective_guides
        .iter()
        .map(|guide| {
            [guide.start, guide.end]
                .map(|uv| normalized_to_screen(layout.image_rect, canvas.apply(uv)))
        })
        .collect()
}
