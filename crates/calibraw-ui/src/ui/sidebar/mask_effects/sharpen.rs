use super::{effect_card, float_param_slider};
use crate::pipeline::{effect_params::sharpen, MaskEffect, SharpenEffectSettings};
use eframe::egui::Ui;

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut SharpenEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
) -> bool {
    effect_card(
        ui,
        MaskEffect::Sharpen,
        settings,
        enabled,
        remove,
        |ui, settings| {
            let mut changed = float_param_slider(ui, &mut settings.amount, sharpen::AMOUNT);
            changed |= float_param_slider(ui, &mut settings.radius, sharpen::RADIUS);
            changed |= float_param_slider(ui, &mut settings.detail, sharpen::DETAIL);
            changed |= float_param_slider(ui, &mut settings.masking, sharpen::MASKING);
            changed
        },
    )
}
