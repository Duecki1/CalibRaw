use super::super::*;

/// Screen distance at which a crop corner or edge can be grabbed.
const CROP_HANDLE_RADIUS: f32 = 28.0;

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

        if app.develop_ui.straighten_tool_active {
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
                    if delta.length() >= 12.0 {
                        let angle = delta.y.atan2(delta.x).to_degrees();
                        let target = nearest_straight_axis_degrees(angle);
                        let correction = normalize_degrees(target - angle);
                        let previous = app.develop.geometry.rotation_degrees;
                        app.develop.geometry.rotation_degrees =
                            (previous + correction).clamp(-45.0, 45.0);
                        if (app.develop.geometry.rotation_degrees - previous).abs() > 1e-4 {
                            let reference =
                                if let Some(reference) = app.develop_ui.crop_constraint_reference {
                                    reference
                                } else {
                                    let reference = app.develop.geometry.crop;
                                    app.develop_ui.crop_constraint_reference = Some(reference);
                                    reference
                                };
                            app.develop.geometry.crop = reference;
                            app.develop
                                .geometry
                                .fit_crop_inside_transformed_source(source_width, source_height);
                            app.note_geometry_changed();
                        }
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
                    let start = crop_preview_pointer_to_source_normalized(
                        image_rect,
                        quarter_turns,
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
                let current = crop_preview_pointer_to_source_normalized(
                    image_rect,
                    quarter_turns,
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

    /// The crop for `drag` with the pointer at `current` (`delta` from where
    /// the drag started), kept inside the rotated and sheared source.
    fn dragged_crop(
        app: &CalibRawApp,
        drag: CropDragState,
        current: [f32; 2],
        delta: [f32; 2],
        source_width: u32,
        source_height: u32,
    ) -> [f32; 4] {
        let geometry = app.develop.geometry;
        if drag.handle == CropHandle::Move {
            return geometry.constrain_crop_move_to_transformed_source(
                drag.crop,
                delta,
                source_width,
                source_height,
            );
        }
        let mut crop = drag.crop;
        match drag.handle {
            CropHandle::Move => {}
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
        let aspect_locked = app.develop.loaded_raw.as_ref().is_some_and(|raw| {
            geometry
                .normalized_crop_aspect(raw.width, raw.height)
                .is_some()
        });
        if !aspect_locked {
            return geometry.constrain_free_crop_resize_to_transformed_source(
                drag.crop,
                crop,
                source_width,
                source_height,
            );
        }
        crop = if is_crop_corner(drag.handle) {
            constrain_crop_corner_aspect(app, drag.crop, current, drag.handle).unwrap_or(crop)
        } else {
            constrain_crop_aspect(app, crop, drag.handle)
        };
        geometry.constrain_crop_drag_to_transformed_source(
            drag.crop,
            crop,
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
                painter.circle_filled(point, 5.5, Color32::WHITE);
                painter.circle_stroke(point, 7.5, Stroke::new(1.5, Color32::BLACK));
            }
        }

        if let Some(line) = app.develop_ui.straighten_drag {
            let stroke = Stroke::new(2.0, Color32::WHITE);
            painter.line_segment([line.start, line.current], stroke);
            painter.circle_filled(line.start, 4.0, Color32::WHITE);
            painter.circle_filled(line.current, 4.0, Color32::WHITE);
        }
    }
}
