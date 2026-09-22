use super::{effect_card, effect_color, effect_slider};
use crate::pipeline::{effect_params::fog, FogEffectSettings, MaskEffect};
use eframe::egui::Ui;

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut FogEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
) -> bool {
    effect_card(
        ui,
        MaskEffect::Fog,
        settings,
        enabled,
        remove,
        |ui, settings| {
            let mut changed = false;
            changed |= effect_slider(ui, &mut settings.amount, fog::AMOUNT);
            changed |= effect_slider(ui, &mut settings.density, fog::DENSITY);
            changed |= effect_slider(ui, &mut settings.scale, fog::SCALE);
            changed |= effect_slider(ui, &mut settings.softness, fog::SOFTNESS);
            changed |= effect_slider(ui, &mut settings.variation, fog::VARIATION);
            changed |= effect_slider(ui, &mut settings.seed, fog::SEED);
            changed |= effect_color(ui, "fog-color-picker", &mut settings.color, fog::COLOR);
            changed
        },
    )
}
