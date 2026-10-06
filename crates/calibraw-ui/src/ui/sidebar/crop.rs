use super::*;
use crate::pipeline::{CropAspectRatio, GeometryTransform};

impl Sidebar {
    pub(super) fn reset_crop(app: &mut CalibRawApp) {
        app.develop.geometry = GeometryTransform::default();
        app.develop_ui.crop_constraint_reference = Some(app.develop.geometry.crop);
        app.develop_ui.crop_drag = None;
        app.develop_ui.straighten_tool_active = false;
        app.develop_ui.straighten_drag = None;
        app.note_geometry_changed();
    }

    pub(super) fn show_crop(ui: &mut Ui, app: &mut CalibRawApp, _layout: ScreenLayout) {
        let source_dimensions = app
            .develop
            .loaded_raw
            .as_ref()
            .map(|raw| (raw.width, raw.height))
            .unwrap_or((1, 1));

        let before = app.develop.geometry;
        if app.develop_ui.crop_constraint_reference.is_none() {
            app.develop_ui.crop_constraint_reference = Some(app.develop.geometry.crop);
        }
        moduwu_design::section_card(ui, "Aspect ratio", |ui| {
            let previous_aspect = app.develop.geometry.aspect_ratio;
            moduwu_design::combo_box(
                "crop-aspect-ratio",
                app.develop.geometry.aspect_ratio.label(),
                ui.available_width().max(1.0),
            )
            .show_ui(ui, |ui| {
                for aspect in [
                    CropAspectRatio::Free,
                    CropAspectRatio::Original,
                    CropAspectRatio::Square,
                    CropAspectRatio::FourThree,
                    CropAspectRatio::ThreeFour,
                    CropAspectRatio::ThreeTwo,
                    CropAspectRatio::TwoThree,
                    CropAspectRatio::SixteenNine,
                    CropAspectRatio::NineSixteen,
                ] {
                    ui.selectable_value(
                        &mut app.develop.geometry.aspect_ratio,
                        aspect,
                        aspect.label(),
                    );
                }
            });
            if app.develop.geometry.aspect_ratio != previous_aspect {
                Self::apply_crop_aspect(app, source_dimensions.0, source_dimensions.1);
                app.develop_ui.crop_constraint_reference = Some(app.develop.geometry.crop);
            }
        });

        moduwu_design::card_gap(ui);
        moduwu_design::section_card(ui, "Rotation", |ui| {
            ui.horizontal(|ui| {
                if crate::ui::icons::icon_button(
                    ui,
                    crate::ui::icons::UiIcon::RotateLeft,
                    moduwu_design::toolbar_icon_size(),
                    "Rotate 90° counter-clockwise",
                )
                .clicked()
                {
                    app.develop.geometry.rotate_quarter_turn(false);
                }
                if crate::ui::icons::icon_button(
                    ui,
                    crate::ui::icons::UiIcon::RotateRight,
                    moduwu_design::toolbar_icon_size(),
                    "Rotate 90° clockwise",
                )
                .clicked()
                {
                    app.develop.geometry.rotate_quarter_turn(true);
                }
            });
            AdjustmentSlider::new(
                "Straighten",
                &mut app.develop.geometry.rotation_degrees,
                -45.0..=45.0,
            )
            .decimals(1)
            .step(0.1)
            .hover_text("Fine rotation for leveling the image.")
            .show(ui);
            let straighten_label = if app.develop_ui.straighten_tool_active {
                "Cancel straighten line"
            } else {
                "Draw straighten line"
            };
            if moduwu_design::toggle_button(
                ui,
                straighten_label,
                app.develop_ui.straighten_tool_active,
            )
                .on_hover_text("Drag along a horizon or vertical edge in the Crop preview. CalibRaw rotates the image so that line becomes level.")
                .clicked()
            {
                app.develop_ui.straighten_tool_active = !app.develop_ui.straighten_tool_active;
                app.develop_ui.straighten_drag = None;
                app.develop_ui.crop_drag = None;
            }
            if app.develop_ui.straighten_tool_active {
                ui.small(
                    "Drag along a horizon or vertical edge in the preview to level the image.",
                );
            }
            if moduwu_design::secondary_button_enabled(
                ui,
                app.auto_straighten_available(),
                "Auto straighten",
            )
            .on_hover_text("Level the image using its horizon and vertical edges.")
            .clicked()
            {
                app.develop_ui.straighten_tool_active = false;
                app.develop_ui.straighten_drag = None;
                app.start_auto_straighten();
            }
        });

        moduwu_design::card_gap(ui);
        moduwu_design::section_card(ui, "Transform", |ui| {
            ui.horizontal_wrapped(|ui| {
                if moduwu_design::toggle_button(
                    ui,
                    "Flip horizontal",
                    app.develop.geometry.flip_horizontal,
                )
                .clicked()
                {
                    app.develop.geometry.flip_horizontal = !app.develop.geometry.flip_horizontal;
                }
                if moduwu_design::toggle_button(
                    ui,
                    "Flip vertical",
                    app.develop.geometry.flip_vertical,
                )
                .clicked()
                {
                    app.develop.geometry.flip_vertical = !app.develop.geometry.flip_vertical;
                }
            });
            AdjustmentSlider::new(
                "Horizontal",
                &mut app.develop.geometry.horizontal_transform,
                -30.0..=30.0,
            )
            .decimals(1)
            .step(0.1)
            .hover_text("Correct horizontal perspective.")
            .show(ui);
            AdjustmentSlider::new(
                "Vertical",
                &mut app.develop.geometry.vertical_transform,
                -30.0..=30.0,
            )
            .decimals(1)
            .step(0.1)
            .hover_text("Correct vertical perspective.")
            .show(ui);
        });

        app.develop.geometry = app.develop.geometry.sanitized();
        let containment_transform_changed =
            (app.develop.geometry.rotation_degrees - before.rotation_degrees).abs() > 1e-6
                || (app.develop.geometry.horizontal_transform - before.horizontal_transform).abs()
                    > 1e-6
                || (app.develop.geometry.vertical_transform - before.vertical_transform).abs()
                    > 1e-6;
        if containment_transform_changed {
            if let Some(reference) = app.develop_ui.crop_constraint_reference {
                app.develop.geometry.crop = reference;
            }
        }
        app.develop
            .geometry
            .fit_crop_inside_transformed_source(source_dimensions.0, source_dimensions.1);
        if app.develop.geometry != before {
            app.note_geometry_changed();
        }
    }

    fn apply_crop_aspect(app: &mut CalibRawApp, source_width: u32, source_height: u32) {
        let Some(target_normalized_ratio) = app
            .develop
            .geometry
            .normalized_crop_aspect(source_width, source_height)
        else {
            return;
        };
        let crop = app.develop.geometry.crop;
        let center_x = (crop[0] + crop[2]) * 0.5;
        let center_y = (crop[1] + crop[3]) * 0.5;
        let mut width = crop[2] - crop[0];
        let mut height = crop[3] - crop[1];
        if width / height.max(f32::EPSILON) > target_normalized_ratio {
            width = height * target_normalized_ratio;
        } else {
            height = width / target_normalized_ratio;
        }
        width = width.clamp(GeometryTransform::MIN_CROP_EXTENT, 1.0);
        height = height.clamp(GeometryTransform::MIN_CROP_EXTENT, 1.0);
        let left = (center_x - width * 0.5).clamp(0.0, 1.0 - width);
        let top = (center_y - height * 0.5).clamp(0.0, 1.0 - height);
        app.develop.geometry.crop = [left, top, left + width, top + height];
    }
}
