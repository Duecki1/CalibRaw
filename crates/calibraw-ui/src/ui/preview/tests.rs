use super::*;

#[test]
fn zoom_keeps_the_pointer_on_the_same_image_pixel() {
    let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
    let base = viewport.size();
    let pointer = egui::pos2(300.0, 200.0);
    let mut zoom = 1.0;
    let mut center = [0.5, 0.5];
    let before = screen_to_normalized_unclamped(viewport, pointer);
    assert!(transform_preview_about_screen_points(
        viewport,
        viewport,
        base,
        &mut zoom,
        &mut center,
        pointer,
        pointer,
        4.0
    ));
    let after =
        screen_to_normalized_unclamped(zoomed_image_rect(viewport, base, zoom, center), pointer);
    for axis in 0..2 {
        assert!((before[axis] - after[axis]).abs() < 1e-6);
    }
}

#[test]
fn pinch_translation_and_zoom_preserve_the_gesture_anchor() {
    let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
    let base = viewport.size();
    let mut zoom = 2.0;
    let mut center = [0.5, 0.5];
    let image = zoomed_image_rect(viewport, base, zoom, center);
    let from = egui::pos2(350.0, 250.0);
    let to = egui::pos2(380.0, 290.0);
    let before = screen_to_normalized_unclamped(image, from);
    transform_preview_about_screen_points(
        viewport,
        image,
        base,
        &mut zoom,
        &mut center,
        from,
        to,
        1.3,
    );
    let after = screen_to_normalized_unclamped(zoomed_image_rect(viewport, base, zoom, center), to);
    for axis in 0..2 {
        assert!((before[axis] - after[axis]).abs() < 1e-6);
    }
}

#[test]
fn zoom_out_preserves_mask_workspace_and_invalid_gestures_do_not_corrupt_the_view() {
    let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
    for factor in [f32::NAN, f32::INFINITY, -1.0, 0.0] {
        let mut zoom = 1.0;
        let mut center = [0.5, 0.5];
        assert!(!transform_preview_about_screen_points(
            viewport,
            viewport,
            viewport.size(),
            &mut zoom,
            &mut center,
            viewport.center(),
            viewport.center(),
            factor
        ));
        assert_eq!((zoom, center), (1.0, [0.5, 0.5]));
    }
    let mut zoom = 1.0;
    let mut center = [0.5, 0.5];
    transform_preview_about_screen_points(
        viewport,
        viewport,
        viewport.size(),
        &mut zoom,
        &mut center,
        viewport.center(),
        viewport.center(),
        0.1,
    );
    assert_eq!((zoom, center), (MIN_PREVIEW_ZOOM, [0.5, 0.5]));
    let image = zoomed_image_rect(viewport, viewport.size(), zoom, center);
    assert!(image.left() > viewport.left() && image.right() < viewport.right());
    assert!(image.top() > viewport.top() && image.bottom() < viewport.bottom());
}

#[test]
fn tiny_source_movements_still_request_new_detail() {
    assert!(preview_uv_changed(
        crate::app::PreviewUvRect {
            min: [0.4, 0.4],
            max: [0.5, 0.5]
        },
        crate::app::PreviewUvRect {
            min: [0.4001, 0.4],
            max: [0.5001, 0.5]
        },
    ));
}

#[cfg(test)]
mod preview_overlay_tests {
    use super::*;

    #[test]
    fn armed_picker_owns_the_adjustments_canvas_without_mobile_section_state() {
        assert!(white_balance_picker_owns_canvas(
            SidebarTab::Adjustments,
            true
        ));
        assert!(!white_balance_picker_owns_canvas(
            SidebarTab::Adjustments,
            false
        ));
        assert!(!white_balance_picker_owns_canvas(SidebarTab::Crop, true));
    }

    #[test]
    fn zoom_overlay_uses_a_native_density_source_crop() {
        let region = overlay_raster_region(
            crate::app::PreviewUvRect {
                min: [0.45, 0.40],
                max: [0.55, 0.60],
            },
            6000,
            4000,
            Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0)),
            1.0,
            2,
        );

        assert_eq!(region.source_x, 2698);
        assert_eq!(region.source_y, 1598);
        assert_eq!(region.source_width, 604);
        assert_eq!(region.source_height, 804);
        assert_eq!(region.texture_width, 604);
        assert_eq!(region.texture_height, 804);
    }

    #[test]
    fn mask_drag_overlay_caps_raster_without_changing_aspect_or_source_coordinates() {
        let edge = if cfg!(target_os = "android") {
            384
        } else {
            512
        };
        for (width, height, preview_size, source_region, texture_size) in [
            (
                6000,
                4000,
                egui::vec2(1200.0, 800.0),
                (1400, 900, 3200, 2200),
                (edge, edge * 11 / 16),
            ),
            (
                4000,
                6000,
                egui::vec2(800.0, 1200.0),
                (900, 1400, 2200, 3200),
                (edge * 11 / 16, edge),
            ),
        ] {
            let full = overlay_raster_region(
                crate::app::PreviewUvRect {
                    min: [0.25, 0.25],
                    max: [0.75, 0.75],
                },
                width,
                height,
                Rect::from_min_size(Pos2::ZERO, preview_size),
                1.0,
                100,
            );
            let drag = mask_overlay_raster_region(full, Some(0), true);

            assert_eq!((drag.texture_width, drag.texture_height), texture_size);
            assert_eq!(
                (
                    drag.source_x,
                    drag.source_y,
                    drag.source_width,
                    drag.source_height,
                ),
                source_region,
            );
            assert_eq!(
                drag.texture_width * full.texture_height,
                drag.texture_height * full.texture_width,
            );
            assert_eq!(
                overlay_source_uv(drag, width, height),
                overlay_source_uv(full, width, height),
            );
        }
    }

    #[test]
    fn mask_drag_overlay_restores_uncapped_key_on_release_and_keeps_small_rasters() {
        let full = OverlayRasterKey {
            source_x: 30,
            source_y: 40,
            source_width: 3200,
            source_height: 2400,
            texture_width: 1600,
            texture_height: 1200,
        };
        let drag = mask_overlay_raster_region(full, Some(0), true);
        // Dimensions alone invalidate the texture even if the revision is unchanged.
        assert_ne!((0, None::<usize>, 7, drag), (0, None::<usize>, 7, full));
        assert_eq!(mask_overlay_raster_region(full, Some(0), false), full);
        assert_eq!(mask_overlay_raster_region(full, None, true), full);

        let small = OverlayRasterKey {
            texture_width: 200,
            texture_height: 100,
            ..full
        };
        assert_eq!(mask_overlay_raster_region(small, Some(0), true), small);
        let thin = mask_overlay_raster_region(
            OverlayRasterKey {
                texture_width: 1,
                texture_height: 4096,
                ..full
            },
            Some(0),
            true,
        );
        assert_eq!(thin.texture_width, 1);
        assert_eq!(
            thin.texture_height,
            if cfg!(target_os = "android") {
                384
            } else {
                512
            },
        );
    }

    #[test]
    fn screen_relative_brush_compensates_for_zoom() {
        assert!((zoom_scaled_brush_size(0.08, 4.0, false) - 0.02).abs() < 1e-6);
    }

    #[test]
    fn image_relative_brush_ignores_zoom() {
        assert!((zoom_scaled_brush_size(0.08, 4.0, true) - 0.08).abs() < 1e-6);
    }
}

#[test]
fn ctrl_plus_and_minus_zoom_the_preview_not_the_interface() {
    let ctx = egui::Context::default();
    crate::ui::theme::install(&ctx);
    let mut factors = Vec::new();
    for (key, modifiers) in [
        (egui::Key::Plus, egui::Modifiers::COMMAND),
        (
            egui::Key::Plus,
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        ),
        (egui::Key::Equals, egui::Modifiers::COMMAND),
        (egui::Key::Minus, egui::Modifiers::COMMAND),
        (egui::Key::Minus, egui::Modifiers::NONE),
    ] {
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            modifiers,
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            factors.push(ui.input_mut(viewport::keyboard_zoom_factor));
        });
    }
    assert_eq!(
        factors,
        [Some(1.25), Some(1.25), Some(1.25), Some(0.8), None]
    );
    assert_eq!(ctx.zoom_factor(), 1.0);
}
