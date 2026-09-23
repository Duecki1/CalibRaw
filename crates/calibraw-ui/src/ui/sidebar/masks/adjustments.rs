use super::*;

impl Sidebar {
    pub(super) fn prepare_content_mask(
        app: &mut CalibRawApp,
        frame: &eframe::Frame,
        kind: MaskKind,
    ) {
        match kind {
            MaskKind::Subject | MaskKind::Background => app.request_subject_mask(frame),
            MaskKind::Object => {
                if let Err(error) = app.capture_mask_source(frame) {
                    app.report_ai_mask_error(error);
                }
            }
            MaskKind::LuminanceRange | MaskKind::ColorRange => {
                if let Err(error) = app.capture_mask_source(frame) {
                    app.ui.status = error;
                    return;
                }
                let source = app.masks.source_cache.clone();
                if let Some(component) = app.masks.stack.selected_component_mut() {
                    match &mut component.geometry {
                        MaskGeometry::LuminanceRange { source: target, .. }
                        | MaskGeometry::ColorRange { source: target, .. } => *target = source,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn show_local_adjustment_card(
        ui: &mut Ui,
        adjustment: &mut crate::pipeline::LocalAdjustments,
        section: MaskSection,
        title: &'static str,
        default_open: bool,
        foldable: bool,
        tabs: (
            &mut ToneCurveTab,
            &mut ColorGradeTab,
            &mut HslMixerColor,
            &mut crate::ui::components::point_color::PointColorUiState,
            &mut bool,
        ),
    ) -> bool {
        let group = match section {
            MaskSection::Light => AdjustmentGroup::Light,
            MaskSection::ToneCurve => AdjustmentGroup::ToneCurve,
            MaskSection::Color => AdjustmentGroup::Color,
            MaskSection::ColorGrading => AdjustmentGroup::ColorGrading,
            MaskSection::Effects => AdjustmentGroup::Effects,
            MaskSection::ColorMixer => AdjustmentGroup::ColorMixer,
            MaskSection::Properties => return false,
        };
        let mut changed = false;
        let action = Self::adjustment_card(ui, title, default_open, foldable, true, |ui| {
            changed |= Self::show_local_mask_adjustment_section(
                ui, adjustment, section, tabs.0, tabs.1, tabs.2, tabs.3, tabs.4,
            )
            .0;
        });
        changed | action.apply_local(adjustment, group)
    }

    pub(super) fn show_local_mask_adjustment_section(
        ui: &mut Ui,
        adjustment: &mut crate::pipeline::LocalAdjustments,
        section: MaskSection,
        selected_tab: &mut ToneCurveTab,
        selected_grade_tab: &mut ColorGradeTab,
        selected_hsl_color: &mut HslMixerColor,
        point_color: &mut crate::ui::components::point_color::PointColorUiState,
        point_color_tab: &mut bool,
    ) -> (bool, bool) {
        match section {
            MaskSection::Properties => (false, false),
            MaskSection::Light => Self::show_local_mask_light(ui, adjustment),
            MaskSection::ToneCurve => (
                Self::show_local_mask_tone_curve(ui, adjustment, selected_tab),
                false,
            ),
            MaskSection::Color => (Self::show_local_mask_color(ui, adjustment), false),
            MaskSection::ColorGrading => (
                Self::show_local_mask_color_grading(ui, adjustment, selected_grade_tab),
                false,
            ),
            MaskSection::Effects => (Self::show_local_mask_effects(ui, adjustment), false),
            MaskSection::ColorMixer => (
                Self::show_local_mask_color_mixer(
                    ui,
                    adjustment,
                    selected_hsl_color,
                    point_color,
                    point_color_tab,
                ),
                false,
            ),
        }
    }

    fn show_local_mask_light(
        ui: &mut Ui,
        adjustment: &mut crate::pipeline::LocalAdjustments,
    ) -> (bool, bool) {
        use crate::pipeline::effect_params::adjustment as params;

        let mut changed = false;
        let shadows_before = adjustment.shadows;
        let blacks_before = adjustment.blacks;
        changed |= gradient_float_param_slider(
            ui,
            &mut adjustment.exposure,
            params::EXPOSURE,
            SliderGradient::Brightness,
        );
        changed |= gradient_float_param_slider(
            ui,
            &mut adjustment.contrast,
            params::CONTRAST,
            SliderGradient::Brightness,
        );
        changed |= gradient_float_param_slider(
            ui,
            &mut adjustment.highlights,
            params::HIGHLIGHTS,
            SliderGradient::Brightness,
        );
        changed |= gradient_float_param_slider(
            ui,
            &mut adjustment.shadows,
            params::SHADOWS,
            SliderGradient::Brightness,
        );
        changed |= gradient_float_param_slider(
            ui,
            &mut adjustment.whites,
            params::WHITES,
            SliderGradient::Brightness,
        );
        changed |= gradient_float_param_slider(
            ui,
            &mut adjustment.blacks,
            params::BLACKS,
            SliderGradient::Brightness,
        );
        (
            changed,
            adjustment.shadows != shadows_before || adjustment.blacks != blacks_before,
        )
    }

    fn show_local_mask_color(
        ui: &mut Ui,
        adjustment: &mut crate::pipeline::LocalAdjustments,
    ) -> bool {
        use crate::pipeline::effect_params::adjustment as params;

        let mut changed = false;
        changed |= gradient_float_param_slider(
            ui,
            &mut adjustment.temperature,
            params::TEMPERATURE,
            SliderGradient::Temperature,
        );
        changed |= gradient_float_param_slider(
            ui,
            &mut adjustment.tint,
            params::TINT,
            SliderGradient::Tint,
        );
        changed |= hue_adjustment_slider(ui, &mut adjustment.hue, params::HUE.tooltip);
        changed |= gradient_float_param_slider(
            ui,
            &mut adjustment.saturation,
            params::SATURATION,
            SliderGradient::Colorfulness,
        );
        changed
    }

    fn show_local_mask_effects(
        ui: &mut Ui,
        adjustment: &mut crate::pipeline::LocalAdjustments,
    ) -> bool {
        use crate::pipeline::effect_params::adjustment as params;

        let mut changed = false;
        changed |= float_param_slider(ui, &mut adjustment.texture, params::TEXTURE);
        changed |= float_param_slider(ui, &mut adjustment.clarity, params::CLARITY);
        changed |= float_param_slider(ui, &mut adjustment.dehaze, params::DEHAZE);
        changed |= float_param_slider(ui, &mut adjustment.halation_amount, params::HALATION);
        changed
    }

    fn show_local_mask_color_grading(
        ui: &mut Ui,
        adjustment: &mut crate::pipeline::LocalAdjustments,
        selected_grade_tab: &mut ColorGradeTab,
    ) -> bool {
        color_grading_editor(ui, &mut adjustment.color_grading, selected_grade_tab)
    }

    fn show_local_mask_tone_curve(
        ui: &mut Ui,
        adjustment: &mut crate::pipeline::LocalAdjustments,
        selected_tab: &mut ToneCurveTab,
    ) -> bool {
        let changed = tone_curve_channel_editor(
            ui,
            ToneCurveChannels {
                rgb: &mut adjustment.tone_curve,
                red: &mut adjustment.tone_curve_red,
                green: &mut adjustment.tone_curve_green,
                blue: &mut adjustment.tone_curve_blue,
            },
            selected_tab,
            32.0,
        );
        if changed {
            adjustment.sanitize_tone_curves();
        }
        changed
    }

    fn show_local_mask_color_mixer(
        ui: &mut Ui,
        adjustment: &mut crate::pipeline::LocalAdjustments,
        selected_color: &mut HslMixerColor,
        point_color: &mut crate::ui::components::point_color::PointColorUiState,
        point_color_tab: &mut bool,
    ) -> bool {
        ui.horizontal(|ui| {
            ui.selectable_value(point_color_tab, false, "Mixer");
            ui.selectable_value(point_color_tab, true, "Point Color");
        });
        ui.add_space(crate::ui::theme::SPACE_XS);
        if *point_color_tab {
            crate::ui::components::point_color::point_color(
                ui,
                &mut adjustment.point_colors,
                point_color,
            )
        } else {
            point_color.picker_active = false;
            point_color.visualize_range = false;
            hsl_mixer(
                ui,
                selected_color,
                &mut adjustment.hsl_hue,
                &mut adjustment.hsl_saturation,
                &mut adjustment.hsl_luminance,
            )
        }
    }
}
