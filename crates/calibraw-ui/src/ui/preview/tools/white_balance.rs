use super::super::*;

impl Preview {
    pub(in crate::ui::preview) fn handle_white_balance_picker(
        ui: &Ui,
        app: &mut CalibRawApp,
        layout: PreviewLayout,
        response: &egui::Response,
    ) {
        let PreviewLayout { visible_rect, .. } = layout;
        let lens_geometry = loaded_lens_geometry(app).cloned();
        let pointer = response
            .interact_pointer_pos()
            .filter(|position| visible_rect.contains(*position));
        let (pressed, down, released) = ui.input(|input| {
            (
                input.pointer.primary_pressed(),
                input.pointer.primary_down(),
                input.pointer.primary_released(),
            )
        });
        let pointer_uv = pointer.and_then(|position| {
            editable_source_uv(
                layout
                    .projection(app.develop.geometry, lens_geometry.as_deref())
                    .to_source(position),
            )
        });

        if pressed {
            if let Some(uv) = pointer_uv {
                app.develop_ui.white_balance_picker_drag = Some([uv, uv]);
            }
        } else if down {
            if let (Some(area), Some(uv)) = (
                app.develop_ui.white_balance_picker_drag.as_mut(),
                pointer_uv,
            ) {
                area[1] = uv;
                ui.ctx().request_repaint();
            }
        }

        if released {
            if let Some(mut area) = app.develop_ui.white_balance_picker_drag.take() {
                if let Some(uv) = pointer_uv {
                    area[1] = uv;
                }
                app.apply_white_balance_area(area);
            }
        }
    }

    pub(in crate::ui::preview) fn paint_white_balance_picker(
        ui: &Ui,
        app: &CalibRawApp,
        layout: PreviewLayout,
    ) {
        let PreviewLayout { visible_rect, .. } = layout;
        let painter = ui.painter_at(visible_rect);
        painter.text(
            visible_rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            "Drag over a neutral gray or white area",
            egui::FontId::proportional(13.0),
            Color32::WHITE,
        );
        let Some(area) = app.develop_ui.white_balance_picker_drag else {
            return;
        };
        let projection = layout.projection(
            app.develop.geometry,
            loaded_lens_geometry(app).map(AsRef::as_ref),
        );
        let start = projection.to_screen(area[0]);
        let current = projection.to_screen(area[1]);
        let rect = Rect::from_two_pos(start, current).intersect(visible_rect);
        if rect.width() > 0.0 && rect.height() > 0.0 {
            painter.rect_filled(rect, 0.0, Color32::from_white_alpha(24));
            painter.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.5, Color32::WHITE),
                egui::StrokeKind::Inside,
            );
        } else {
            painter.circle_stroke(start, 6.0, Stroke::new(1.5, Color32::WHITE));
        }
    }
}
