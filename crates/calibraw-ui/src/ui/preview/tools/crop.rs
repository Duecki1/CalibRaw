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
                        let rotation = app.develop.geometry.rotation_degrees + correction;
                        app.set_straighten_rotation(rotation, source_width, source_height);
                        // One line levels the photo; the crop handles return.
                        // A tap too short to measure keeps the tool active.
                        app.develop_ui.straighten_tool_active = false;
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
    /// the drag started), kept inside the rotated and sheared source. The
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

        if let Some(line) = app.develop_ui.straighten_drag {
            let stroke = Stroke::new(2.0, Color32::WHITE);
            painter.line_segment([line.start, line.current], stroke);
            for end in [line.start, line.current] {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Draws a straighten line from `start` to `end` over a 600×400 image
    /// and returns the app afterwards.
    fn draw_straighten_line(start: Pos2, end: Pos2) -> CalibRawApp {
        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx);
        let mut app = CalibRawApp::empty(&ctx);
        app.develop_ui.straighten_tool_active = true;
        let image_rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(600.0, 400.0));
        let layout = PreviewLayout {
            image_rect,
            visible_rect: image_rect,
            viewport_rect: image_rect,
            source_width: 3000,
            source_height: 2000,
        };
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        for events in [
            vec![egui::Event::PointerMoved(start)],
            vec![button(start, true)],
            vec![egui::Event::PointerMoved(end)],
            vec![button(end, false)],
        ] {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(image_rect),
                    events,
                    ..Default::default()
                },
                |ui| Preview::handle_crop_interaction(ui, &mut app, layout),
            );
        }
        app
    }

    #[test]
    fn a_straighten_line_levels_the_photo_and_ends_the_tool() {
        let app = draw_straighten_line(Pos2::new(100.0, 200.0), Pos2::new(500.0, 230.0));
        assert!(!app.develop_ui.straighten_tool_active);
        assert!(app.develop_ui.straighten_drag.is_none());
        assert!(app.develop.geometry.rotation_degrees.abs() > 1.0);
    }

    #[test]
    fn a_tap_too_short_to_measure_keeps_the_straighten_tool() {
        let app = draw_straighten_line(Pos2::new(100.0, 200.0), Pos2::new(104.0, 201.0));
        assert!(app.develop_ui.straighten_tool_active);
        assert_eq!(app.develop.geometry.rotation_degrees, 0.0);
    }

    #[test]
    fn rotated_crop_handles_follow_the_pointer() {
        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx);
        let mut app = CalibRawApp::empty(&ctx);
        app.develop.geometry.rotation_degrees = 12.0;
        app.develop.geometry.vertical_transform = 4.0;
        app.develop.geometry.crop = [0.3, 0.3, 0.6, 0.6];
        let (width, height) = (3000, 2000);
        let image_rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(600.0, 400.0));
        let delta = [0.03, -0.02];
        let shift = egui::vec2(delta[0] * 600.0, delta[1] * 400.0);
        let geometry = app.develop.geometry;
        let before = crop_preview_screen_rect(image_rect, geometry, width, height);
        for (handle, moves_min, moves_max) in [
            (CropHandle::Move, [true, true], [true, true]),
            (CropHandle::Left, [true, false], [false, false]),
            (CropHandle::Bottom, [false, false], [false, true]),
            (CropHandle::TopRight, [false, true], [true, false]),
        ] {
            let drag = CropDragState {
                handle,
                start: [0.5, 0.5],
                crop: geometry.crop,
            };
            let current = [0.5 + delta[0], 0.5 + delta[1]];
            let mut dragged = geometry;
            dragged.crop = Preview::dragged_crop(&app, drag, current, delta, width, height);
            let after = crop_preview_screen_rect(image_rect, dragged, width, height);
            let expected_min = before.min
                + egui::vec2(
                    if moves_min[0] { shift.x } else { 0.0 },
                    if moves_min[1] { shift.y } else { 0.0 },
                );
            let expected_max = before.max
                + egui::vec2(
                    if moves_max[0] { shift.x } else { 0.0 },
                    if moves_max[1] { shift.y } else { 0.0 },
                );
            assert!(
                after.min.distance(expected_min) < 0.05 && after.max.distance(expected_max) < 0.05,
                "{handle:?}: {before:?} -> {after:?}"
            );
        }
    }
}
