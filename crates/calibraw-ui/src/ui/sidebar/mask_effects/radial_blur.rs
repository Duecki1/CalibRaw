use super::{effect_card, effect_position, float_param_slider, EffectFrame, PositionSpace};
use crate::pipeline::{
    effect_params::radial_blur, MaskEffect, RadialBlurEffectSettings, RadialBlurMode,
};
use eframe::egui::Ui;

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut RadialBlurEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
    frame: &EffectFrame,
) -> bool {
    effect_card(
        ui,
        MaskEffect::RadialBlur,
        settings,
        enabled,
        remove,
        |ui, settings| {
            let mut changed = mode_selector(ui, &mut settings.mode);
            changed |= float_param_slider(ui, &mut settings.amount, radial_blur::AMOUNT);
            changed |= float_param_slider(ui, &mut settings.strength, radial_blur::STRENGTH);
            changed |= effect_position(
                ui,
                "Blur center",
                &mut settings.center,
                [radial_blur::CENTER_X, radial_blur::CENTER_Y],
                frame,
                PositionSpace::Source,
            );
            changed
        },
    )
}

fn mode_selector(ui: &mut Ui, mode: &mut RadialBlurMode) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let width = ((ui.available_width() - ui.spacing().item_spacing.x) * 0.5).max(1.0);
        for candidate in RadialBlurMode::ALL {
            if moduwu_design::segmented_button(ui, candidate.label(), *mode == candidate, width)
                .on_hover_text(match candidate {
                    RadialBlurMode::Zoom => "Trails radiate toward the blur center.",
                    RadialBlurMode::Spin => "Trails rotate around the blur center.",
                })
                .clicked()
                && *mode != candidate
            {
                *mode = candidate;
                changed = true;
            }
        }
    });
    changed
}
