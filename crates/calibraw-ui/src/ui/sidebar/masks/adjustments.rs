use super::*;
use crate::app::DevelopUiState;
use crate::ui::components::point_color::PointColorUiState;

/// Sidebar tab selections used by the local mask adjustment sections.
pub(super) struct LocalAdjustmentTabs<'a> {
    pub(super) tone_curve: &'a mut ToneCurveTab,
    pub(super) color_grade: &'a mut ColorGradeTab,
    pub(super) hsl_mixer_color: &'a mut HslMixerColor,
    pub(super) point_color: &'a mut PointColorUiState,
    pub(super) point_color_tab: &'a mut bool,
}

impl<'a> LocalAdjustmentTabs<'a> {
    pub(super) fn for_masks(develop_ui: &'a mut DevelopUiState) -> Self {
        Self {
            tone_curve: &mut develop_ui.tone_curve_tab,
            color_grade: &mut develop_ui.color_grade_tab,
            hsl_mixer_color: &mut develop_ui.hsl_mixer_color,
            point_color: &mut develop_ui.mask_point_color,
            point_color_tab: &mut develop_ui.mask_point_color_tab,
        }
    }
}

impl Sidebar {
    pub(super) fn show_local_adjustment_card(
        ui: &mut Ui,
        adjustment: &mut crate::pipeline::LocalAdjustments,
        section: MaskSection,
        title: &'static str,
        default_open: bool,
        foldable: bool,
        tabs: LocalAdjustmentTabs<'_>,
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
            changed |= Self::show_local_mask_adjustment_section(ui, adjustment, section, tabs);
        });
        changed | action.apply_local(adjustment, group)
    }

    pub(super) fn show_local_mask_adjustment_section(
        ui: &mut Ui,
        adjustment: &mut crate::pipeline::LocalAdjustments,
        section: MaskSection,
        tabs: LocalAdjustmentTabs<'_>,
    ) -> bool {
        match section {
            MaskSection::Properties => false,
            MaskSection::Light => Self::show_local_mask_light(ui, adjustment),
            MaskSection::ToneCurve => {
                Self::show_local_mask_tone_curve(ui, adjustment, tabs.tone_curve)
            }
            MaskSection::Color => Self::show_local_mask_color(ui, adjustment),
            MaskSection::ColorGrading => {
                Self::show_local_mask_color_grading(ui, adjustment, tabs.color_grade)
            }
            MaskSection::Effects => Self::show_local_mask_effects(ui, adjustment),
            MaskSection::ColorMixer => Self::show_local_mask_color_mixer(
                ui,
                adjustment,
                tabs.hsl_mixer_color,
                tabs.point_color,
                tabs.point_color_tab,
            ),
        }
    }

    fn show_local_mask_light(
        ui: &mut Ui,
        adjustment: &mut crate::pipeline::LocalAdjustments,
    ) -> bool {
        use crate::pipeline::effect_params::adjustment as params;

        let mut changed = false;
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
        changed
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
        point_color: &mut PointColorUiState,
        point_color_tab: &mut bool,
    ) -> bool {
        ui.horizontal(|ui| {
            ui.selectable_value(point_color_tab, false, "Mixer");
            ui.selectable_value(point_color_tab, true, "Point Color");
        });
        ui.add_space(moduwu_design::SPACE_XS);
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
