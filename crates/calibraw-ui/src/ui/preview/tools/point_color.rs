use super::super::*;

impl Preview {
    pub(in crate::ui::preview) fn handle_point_color_picker(
        ui: &Ui,
        app: &mut CalibRawApp,
        frame: &eframe::Frame,
        image_rect: Rect,
        preview_rect: Rect,
        source_width: u32,
        source_height: u32,
        response: &egui::Response,
    ) {
        let mask_index = match app.ui.sidebar_tab {
            SidebarTab::Adjustments if app.develop_ui.point_color.picker_active => None,
            SidebarTab::Masks if app.develop_ui.mask_point_color.picker_active => {
                let Some(index) = app.masks.stack.selected_mask else {
                    app.develop_ui.mask_point_color.picker_active = false;
                    return;
                };
                if !app
                    .masks
                    .stack
                    .masks
                    .get(index)
                    .is_some_and(|mask| mask.effect.uses_adjustments())
                {
                    app.develop_ui.mask_point_color.picker_active = false;
                    return;
                }
                Some(index)
            }
            _ => return,
        };
        let colors_len = if let Some(index) = mask_index {
            app.masks
                .stack
                .masks
                .get(index)
                .map_or(0, |mask| mask.adjustments.point_colors.len())
        } else {
            app.develop.exposure.point_colors.len()
        };
        if colors_len >= crate::pipeline::MAX_POINT_COLORS {
            if mask_index.is_some() {
                app.develop_ui.mask_point_color.picker_active = false;
            } else {
                app.develop_ui.point_color.picker_active = false;
            }
            return;
        }
        if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            if mask_index.is_some() {
                app.develop_ui.mask_point_color.picker_active = false;
            } else {
                app.develop_ui.point_color.picker_active = false;
            }
            return;
        }
        let Some(pointer) = response
            .interact_pointer_pos()
            .filter(|position| preview_rect.contains(*position))
        else {
            return;
        };
        let lens_geometry = loaded_lens_geometry(app).cloned();
        let Some(uv) = editable_source_uv(final_geometry_screen_to_native_source(
            image_rect,
            app.develop.geometry,
            lens_geometry.as_deref(),
            source_width,
            source_height,
            pointer,
        )) else {
            return;
        };
        if !ui.input(|input| input.pointer.primary_released()) {
            return;
        }

        if app.preview.pending_stage.is_some() {
            app.ui.notice =
                Some("Preview is updating. Sample the color again when it is ready.".into());
            return;
        }
        let Some(render_state) = frame.wgpu_render_state() else {
            return;
        };
        let Some(pipeline) = app.preview.gpu_pipeline.as_ref() else {
            return;
        };
        if pipeline.width == 0 || pipeline.height == 0 {
            return;
        }
        let x = ((uv[0] * pipeline.width as f32).floor() as u32).min(pipeline.width - 1);
        let y = ((uv[1] * pipeline.height as f32).floor() as u32).min(pipeline.height - 1);
        let Some(raw) = app.develop.preview_raw.as_ref() else {
            return;
        };
        let params = crate::pipeline::GpuParams::new(
            &app.develop.target_exposure,
            &app.preview_mask_stack(),
            raw,
        )
        .with_vignette_geometry(app.develop.geometry);
        let sample = if mask_index.is_some() {
            crate::pipeline::RawGpuPipeline::read_local_point_color_sample_blocking
        } else {
            crate::pipeline::RawGpuPipeline::read_point_color_sample_blocking
        };
        let rgb = match sample(
            pipeline,
            &render_state.device,
            &render_state.queue,
            &params,
            x,
            y,
        ) {
            Ok(rgb) => rgb,
            Err(error) => {
                app.ui.notice = Some(format!("Could not sample point color: {error:#}"));
                return;
            }
        };
        let srgb_linear = rec2020_to_srgb(rgb);
        let srgb = srgb_linear.map(linear_to_srgb);
        let point = crate::pipeline::PointColor::from_srgb(srgb);
        if let Some(index) = mask_index {
            if let Some(mask) = app.masks.stack.masks.get_mut(index) {
                if mask.adjustments.point_colors.push(point) {
                    app.develop_ui.mask_point_color.selected =
                        mask.adjustments.point_colors.len() - 1;
                    app.develop_ui.mask_point_color.picker_active = false;
                    crate::app::preview_visibility::PreviewVisibility::invalidate_mask_cache(
                        ui.ctx(),
                    );
                    app.mark_mask_adjustments_dirty();
                    ui.ctx().request_repaint();
                }
            }
        } else if app.develop.exposure.point_colors.push(point) {
            app.develop_ui.point_color.selected = app.develop.exposure.point_colors.len() - 1;
            app.develop_ui.point_color.picker_active = false;
            app.mark_pipeline_dirty();
            ui.ctx().request_repaint();
        }
    }

    pub(in crate::ui::preview) fn paint_point_color_picker(
        ui: &Ui,
        app: &CalibRawApp,
        preview_rect: Rect,
        response: &egui::Response,
    ) {
        let active = match app.ui.sidebar_tab {
            SidebarTab::Adjustments => app.develop_ui.point_color.picker_active,
            SidebarTab::Masks => app.develop_ui.mask_point_color.picker_active,
            _ => false,
        };
        if !active {
            return;
        }
        let painter = ui.painter_at(preview_rect);
        painter.text(
            preview_rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            "Click a color in the image",
            egui::FontId::proportional(13.0),
            Color32::WHITE,
        );
        if let Some(pointer) = response.hover_pos().filter(|p| preview_rect.contains(*p)) {
            painter.circle_stroke(pointer, 8.0, Stroke::new(1.5, Color32::WHITE));
            painter.line_segment(
                [
                    pointer - egui::vec2(12.0, 0.0),
                    pointer + egui::vec2(12.0, 0.0),
                ],
                Stroke::new(1.0, Color32::WHITE),
            );
            painter.line_segment(
                [
                    pointer - egui::vec2(0.0, 12.0),
                    pointer + egui::vec2(0.0, 12.0),
                ],
                Stroke::new(1.0, Color32::WHITE),
            );
        }
    }
}

fn linear_to_srgb(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

fn rec2020_to_srgb(rgb: [f32; 3]) -> [f32; 3] {
    // The preview's display-linear attachment is linear Rec. 2020. PointColor
    // stores encoded sRGB samples, so convert primaries before applying the OETF.
    [
        1.660_491 * rgb[0] - 0.5876411 * rgb[1] - 0.0728499 * rgb[2],
        -0.1245505 * rgb[0] + 1.1328999 * rgb[1] - 0.0083494 * rgb[2],
        -0.0181508 * rgb[0] - 0.1005789 * rgb[1] + 1.1187297 * rgb[2],
    ]
    .map(|value| value.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::linear_to_srgb;

    #[test]
    fn converts_linear_black_white_and_mid_gray() {
        assert_eq!(linear_to_srgb(0.0), 0.0);
        assert!((linear_to_srgb(1.0) - 1.0).abs() < 1e-6);
        assert!((linear_to_srgb(0.18) - 0.461).abs() < 0.002);
    }
}
