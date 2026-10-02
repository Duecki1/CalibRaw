use super::{effect_card, effect_details, float_param_slider};
use crate::pipeline::{effect_params::halation, HalationEffectSettings, MaskEffect};
use eframe::egui::Ui;

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut HalationEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
) -> bool {
    effect_card(
        ui,
        MaskEffect::Halation,
        settings,
        enabled,
        remove,
        |ui, settings| {
            let mut changed = float_param_slider(ui, &mut settings.amount, halation::AMOUNT);
            changed |= float_param_slider(ui, &mut settings.radius, halation::RADIUS);
            changed |= float_param_slider(ui, &mut settings.threshold, halation::THRESHOLD);
            changed |= effect_details(ui, "Color details", |ui| {
                float_param_slider(ui, &mut settings.warmth, halation::WARMTH)
            });
            changed
        },
    )
}
