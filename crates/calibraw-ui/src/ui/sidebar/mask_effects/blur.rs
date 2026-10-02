use super::{effect_card, float_param_slider};
use crate::pipeline::{effect_params::blur, BlurEffectSettings, MaskEffect};
use eframe::egui::Ui;

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut BlurEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
) -> bool {
    effect_card(
        ui,
        MaskEffect::Blur,
        settings,
        enabled,
        remove,
        |ui, settings| {
            let mut changed = false;
            changed |= float_param_slider(ui, &mut settings.amount, blur::AMOUNT);
            changed |= float_param_slider(ui, &mut settings.radius, blur::RADIUS);
            changed
        },
    )
}
