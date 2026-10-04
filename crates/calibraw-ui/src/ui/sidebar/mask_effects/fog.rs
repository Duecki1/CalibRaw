use super::{effect_card, effect_color, effect_details, float_param_slider};
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
            let mut changed = float_param_slider(ui, &mut settings.amount, fog::AMOUNT);
            changed |= float_param_slider(ui, &mut settings.density, fog::DENSITY);
            if moduwu_design::toggle_button(ui, "Scene depth", settings.depth_enabled)
                .on_hover_text("Generate and use shared scene depth so distant areas collect more fog. Turn off to use a flat veil; depth settings are preserved.")
                .clicked()
            {
                settings.depth_enabled = !settings.depth_enabled;
                changed = true;
            }
            if settings.depth_enabled {
                changed |= float_param_slider(ui, &mut settings.start, fog::START);
            }
            changed |= effect_color(ui, "fog-color-picker", &mut settings.color, fog::COLOR);
            changed |= effect_details(ui, "Atmosphere details", |ui| {
                let mut changed = false;
                if settings.depth_enabled {
                    changed |=
                        float_param_slider(ui, &mut settings.depth_influence, fog::DEPTH_INFLUENCE);
                }
                changed |= float_param_slider(ui, &mut settings.softness, fog::SOFTNESS);
                changed |= float_param_slider(ui, &mut settings.variation, fog::VARIATION);
                if settings.variation > 0.0 {
                    changed |= float_param_slider(ui, &mut settings.scale, fog::SCALE);
                    changed |= float_param_slider(ui, &mut settings.seed, fog::SEED);
                }
                changed
            });
            changed
        },
    )
}
