use super::{
    effect_card, effect_details, effect_position, float_param_slider, EffectFrame, PositionSpace,
};
use crate::pipeline::{effect_params::vignette, MaskEffect, VignetteEffectSettings};
use eframe::egui::Ui;

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut VignetteEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
    frame: &EffectFrame,
) -> bool {
    effect_card(
        ui,
        MaskEffect::Vignette,
        settings,
        enabled,
        remove,
        |ui, settings| {
            let mut changed = float_param_slider(ui, &mut settings.amount, vignette::AMOUNT);
            changed |= float_param_slider(ui, &mut settings.midpoint, vignette::MIDPOINT);
            changed |= float_param_slider(ui, &mut settings.feather, vignette::FEATHER);
            changed |= effect_details(ui, "Shape details", |ui| {
                let mut changed =
                    float_param_slider(ui, &mut settings.roundness, vignette::ROUNDNESS);
                if settings.amount < 0.0 {
                    changed |=
                        float_param_slider(ui, &mut settings.highlights, vignette::HIGHLIGHTS);
                }
                changed |= effect_position(
                    ui,
                    "Vignette center",
                    &mut settings.center,
                    [vignette::CENTER_X, vignette::CENTER_Y],
                    frame,
                    PositionSpace::Output,
                );
                changed
            });
            changed
        },
    )
}
