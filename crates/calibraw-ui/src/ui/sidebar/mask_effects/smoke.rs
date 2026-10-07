use super::{
    effect_card, effect_color, effect_details, float_param_angle, float_param_slider,
    image_lights_toggle, pattern_seed,
};
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
            let mut changed = float_param_slider(ui, &mut settings.amount, smoke::AMOUNT);
            changed |= float_param_slider(ui, &mut settings.density, smoke::DENSITY);
            changed |= float_param_slider(ui, &mut settings.scale, smoke::SCALE);
            changed |= effect_color(ui, "smoke-color-picker", &mut settings.color, smoke::COLOR);
            changed |= float_param_slider(ui, &mut settings.light_glow, smoke::LIGHT_GLOW);
            changed |= image_lights_toggle(ui, &mut settings.image_lights);
            changed |= effect_details(ui, "Texture details", |ui| {
                float_param_slider(ui, &mut settings.turbulence, smoke::TURBULENCE)
                    | float_param_slider(ui, &mut settings.softness, smoke::SOFTNESS)
                    | float_param_angle(ui, &mut settings.angle, smoke::ANGLE)
                    | pattern_seed(ui, &mut settings.seed, smoke::SEED)
            });
            changed
        },
    )
}
