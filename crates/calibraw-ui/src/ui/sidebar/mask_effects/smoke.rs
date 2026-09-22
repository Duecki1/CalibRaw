use super::{effect_card, effect_color, effect_slider};
use crate::pipeline::{effect_params::smoke, MaskEffect, SmokeEffectSettings};
use eframe::egui::Ui;

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut SmokeEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
) -> bool {
    effect_card(
        ui,
        MaskEffect::Smoke,
        settings,
        enabled,
        remove,
        |ui, settings| {
            let mut changed = false;
            changed |= effect_slider(ui, &mut settings.amount, smoke::AMOUNT);
            changed |= effect_slider(ui, &mut settings.density, smoke::DENSITY);
            changed |= effect_slider(ui, &mut settings.scale, smoke::SCALE);
            changed |= effect_slider(ui, &mut settings.turbulence, smoke::TURBULENCE);
            changed |= effect_slider(ui, &mut settings.softness, smoke::SOFTNESS);
            changed |= effect_slider(ui, &mut settings.angle, smoke::ANGLE);
            changed |= effect_slider(ui, &mut settings.seed, smoke::SEED);
            changed |= effect_color(ui, "smoke-color-picker", &mut settings.color, smoke::COLOR);
            changed
        },
    )
}
