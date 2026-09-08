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
fn zoom_out_stops_at_fit_and_invalid_gestures_do_not_corrupt_the_view() {
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
    assert_eq!((zoom, center), (1.0, [0.5, 0.5]));
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
    fn screen_relative_brush_compensates_for_zoom() {
        assert!((zoom_scaled_brush_size(0.08, 4.0, false) - 0.02).abs() < 1e-6);
    }

    #[test]
    fn image_relative_brush_ignores_zoom() {
        assert!((zoom_scaled_brush_size(0.08, 4.0, true) - 0.08).abs() < 1e-6);
    }
}
