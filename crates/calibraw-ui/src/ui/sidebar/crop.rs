use super::*;
use crate::app::AutoGeometry;
use crate::pipeline::{CropAspectRatio, GeometryTransform, PerspectiveCorrection};

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
            let (output_width, output_height) = app
                .develop
                .geometry
                .crop_pixel_dimensions(source_dimensions.0, source_dimensions.1);
            Self::show_aspect_ratio_picker(
                ui,
                &mut app.develop.geometry.aspect_ratio,
                output_height > output_width,
            );
            if app.develop.geometry.aspect_ratio != previous_aspect {
                app.develop
                    .geometry
                    .conform_crop_to_aspect(source_dimensions.0, source_dimensions.1);
                app.develop_ui.crop_constraint_reference = Some(app.develop.geometry.crop);
            }
        });

        moduwu_design::card_gap(ui);
        moduwu_design::section_card(ui, "Rotate & flip", |ui| {
            let size = moduwu_design::toolbar_icon_size();
            ui.horizontal_wrapped(|ui| {
                if moduwu_design::ghost_icon_button(
                    ui,
                    egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                    size,
                    "Rotate 90° counter-clockwise",
                )
                .clicked()
                {
                    app.develop.geometry.rotate_quarter_turn(false);
                }
                if moduwu_design::ghost_icon_button(
                    ui,
                    egui_phosphor::regular::ARROW_CLOCKWISE,
                    size,
                    "Rotate 90° clockwise",
                )
                .clicked()
                {
                    app.develop.geometry.rotate_quarter_turn(true);
                }
                if moduwu_design::ghost_icon_toggle_button(
                    ui,
                    egui_phosphor::regular::FLIP_HORIZONTAL,
                    app.develop.geometry.flip_horizontal,
                    size,
                    "Flip horizontal",
                )
                .clicked()
                {
                    app.develop.geometry.flip_horizontal = !app.develop.geometry.flip_horizontal;
                }
                if moduwu_design::ghost_icon_toggle_button(
                    ui,
                    egui_phosphor::regular::FLIP_VERTICAL,
                    app.develop.geometry.flip_vertical,
                    size,
                    "Flip vertical",
                )
                .clicked()
                {
                    app.develop.geometry.flip_vertical = !app.develop.geometry.flip_vertical;
                }
                ui.separator();
                let straighten_tooltip = if app.develop_ui.straighten_tool_active {
                    "Cancel straighten line"
                } else {
                    "Draw straighten line: drag along a horizon or vertical edge in the preview to level it"
                };
                if moduwu_design::ghost_icon_toggle_button(
                    ui,
                    egui_phosphor::regular::RULER,
                    app.develop_ui.straighten_tool_active,
                    size,
                    straighten_tooltip,
                )
                .clicked()
                {
                    app.develop_ui.straighten_tool_active = !app.develop_ui.straighten_tool_active;
                    app.develop_ui.straighten_drag = None;
                    app.develop_ui.crop_drag = None;
                }
                if ui
                    .add_enabled_ui(app.auto_geometry_available(), |ui| {
                        moduwu_design::ghost_icon_button(
                            ui,
                            egui_phosphor::regular::MAGIC_WAND,
                            size,
                            "Auto straighten: level the image using its horizon and vertical edges",
                        )
                    })
                    .inner
                    .clicked()
                {
                    app.develop_ui.straighten_tool_active = false;
                    app.develop_ui.straighten_drag = None;
                    app.start_auto_geometry(AutoGeometry::Straighten);
                }
            });
            AdjustmentSlider::new(
                "Straighten",
                &mut app.develop.geometry.rotation_degrees,
                -45.0..=45.0,
            )
            .decimals(1)
            .step(0.1)
            .show(ui);
        });

        moduwu_design::card_gap(ui);
        moduwu_design::section_card(ui, "Transform", |ui| {
            let size = moduwu_design::toolbar_icon_size();
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled_ui(app.auto_geometry_available(), |ui| {
                        moduwu_design::ghost_icon_button(
                            ui,
                            egui_phosphor::regular::MAGIC_WAND,
                            size,
                            "Auto perspective: make vertical lines parallel and level the image",
                        )
                    })
                    .inner
                    .clicked()
                {
                    app.develop_ui.straighten_tool_active = false;
                    app.develop_ui.straighten_drag = None;
                    app.start_auto_geometry(AutoGeometry::Perspective);
                }
            });
            let range = -PerspectiveCorrection::MAX_DEGREES..=PerspectiveCorrection::MAX_DEGREES;
            let mut perspective = app.develop.geometry.displayed_perspective();
            let mut perspective_changed = false;
            for (label, value) in ["Vertical", "Horizontal"].into_iter().zip(&mut perspective) {
                perspective_changed |= AdjustmentSlider::new(label, value, range.clone())
                    .decimals(1)
                    .step(0.1)
                    .show(ui);
            }
            if perspective_changed {
                // A new correction is made with this photo's lens; an existing one keeps its own.
                if before.perspective.is_identity() {
                    app.develop.geometry.perspective.focal_length_35mm =
                        app.perspective_focal_length();
                }
                app.develop.geometry.set_displayed_perspective(perspective);
            }
            let mut aspect = app.develop.geometry.displayed_aspect_stretch();
            if AdjustmentSlider::new(
                "Aspect",
                &mut aspect,
                -GeometryTransform::MAX_ASPECT_STRETCH..=GeometryTransform::MAX_ASPECT_STRETCH,
            )
            .decimals(0)
            .step(1.0)
            .show(ui)
            {
                app.develop.geometry.set_displayed_aspect_stretch(aspect);
            }
            AdjustmentSlider::new(
                "Horizontal skew",
                &mut app.develop.geometry.horizontal_skew_degrees,
                -30.0..=30.0,
            )
            .decimals(1)
            .step(0.1)
            .show(ui);
            AdjustmentSlider::new(
                "Vertical skew",
                &mut app.develop.geometry.vertical_skew_degrees,
                -30.0..=30.0,
            )
            .decimals(1)
            .step(0.1)
            .show(ui);
        });

        app.develop.geometry = app.develop.geometry.sanitized();
        let geometry = app.develop.geometry;
        let containment_transform_changed =
            (geometry.rotation_degrees - before.rotation_degrees).abs() > 1e-6
                || (geometry.horizontal_skew_degrees - before.horizontal_skew_degrees).abs() > 1e-6
                || (geometry.vertical_skew_degrees - before.vertical_skew_degrees).abs() > 1e-6
                || geometry.perspective != before.perspective
                || geometry.aspect_stretch != before.aspect_stretch;
        if containment_transform_changed {
            app.refit_crop(source_dimensions.0, source_dimensions.1);
        } else {
            app.develop
                .geometry
                .fit_crop_inside_transformed_source(source_dimensions.0, source_dimensions.1);
        }
        if app.develop.geometry != before {
            app.note_geometry_changed();
        }
    }

    /// Ratio chips, the orientation of a non-square ratio and the terms of a
    /// custom one. `crop_is_portrait` orients a ratio picked while none is
    /// fixed.
    fn show_aspect_ratio_picker(ui: &mut Ui, ratio: &mut CropAspectRatio, crop_is_portrait: bool) {
        moduwu_design::action_row(ui, |ui| {
            for (choice, label) in AspectChoice::ALL {
                if moduwu_design::chip(ui, label, AspectChoice::of(*ratio) == choice).clicked() {
                    *ratio = choice.pick(*ratio, crop_is_portrait);
                }
            }
        });
        if let Some(mut portrait) = is_portrait(*ratio) {
            let width = ui.available_width().max(1.0);
            if moduwu_design::segmented_control(
                ui,
                &mut portrait,
                &[(false, "Landscape"), (true, "Portrait")],
                width,
            )
            .changed()
            {
                *ratio = ratio.transposed();
            }
        }
        if let CropAspectRatio::Custom {
            mut width,
            mut height,
        } = *ratio
        {
            let terms = 1..=CropAspectRatio::MAX_TERM;
            ui.horizontal(|ui| {
                ui.add(
                    moduwu_design::NumberField::new(&mut width, terms.clone())
                        .commit_on_finish(true)
                        .id_salt("crop-custom-aspect-width"),
                );
                ui.label("×");
                ui.add(
                    moduwu_design::NumberField::new(&mut height, terms)
                        .commit_on_finish(true)
                        .id_salt("crop-custom-aspect-height"),
                );
            });
            *ratio = CropAspectRatio::Custom { width, height };
        }
    }
}

/// One chip of the aspect-ratio picker. A chip stands for its ratio in both
/// orientations; the orientation is chosen separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AspectChoice {
    Free,
    Original,
    Square,
    FourThree,
    ThreeTwo,
    SixteenNine,
    Custom,
}

impl AspectChoice {
    const ALL: [(Self, &'static str); 7] = [
        (Self::Free, "Free"),
        (Self::Original, "Original"),
        (Self::Square, "1:1"),
        (Self::FourThree, "4:3"),
        (Self::ThreeTwo, "3:2"),
        (Self::SixteenNine, "16:9"),
        (Self::Custom, "Custom"),
    ];

    fn of(ratio: CropAspectRatio) -> Self {
        match ratio {
            CropAspectRatio::Free => Self::Free,
            CropAspectRatio::Original => Self::Original,
            CropAspectRatio::Square => Self::Square,
            CropAspectRatio::FourThree | CropAspectRatio::ThreeFour => Self::FourThree,
            CropAspectRatio::ThreeTwo | CropAspectRatio::TwoThree => Self::ThreeTwo,
            CropAspectRatio::SixteenNine | CropAspectRatio::NineSixteen => Self::SixteenNine,
            CropAspectRatio::Custom { .. } => Self::Custom,
        }
    }

    /// The ratio to use when this chip is picked while `current` is set:
    /// the current choice stays as it is, and a new fixed ratio keeps the
    /// current orientation, or the crop's when no ratio is fixed. Custom
    /// starts from the current ratio's terms, or 5 × 4.
    fn pick(self, current: CropAspectRatio, crop_is_portrait: bool) -> CropAspectRatio {
        if Self::of(current) == self {
            return current;
        }
        let landscape = match self {
            Self::Free => return CropAspectRatio::Free,
            Self::Original => return CropAspectRatio::Original,
            Self::Square => return CropAspectRatio::Square,
            Self::FourThree => CropAspectRatio::FourThree,
            Self::ThreeTwo => CropAspectRatio::ThreeTwo,
            Self::SixteenNine => CropAspectRatio::SixteenNine,
            Self::Custom => {
                let (width, height) = current
                    .terms()
                    .filter(|(width, height)| width != height)
                    .map_or((5, 4), |(width, height)| {
                        (width.max(height), width.min(height))
                    });
                CropAspectRatio::Custom { width, height }
            }
        };
        if is_portrait(current).unwrap_or(crop_is_portrait) {
            landscape.transposed()
        } else {
            landscape
        }
    }
}

/// Whether a fixed, non-square ratio is taller than wide; `None` for ratios
/// without an orientation.
fn is_portrait(ratio: CropAspectRatio) -> Option<bool> {
    ratio
        .terms()
        .filter(|(width, height)| width != height)
        .map(|(width, height)| height > width)
}
