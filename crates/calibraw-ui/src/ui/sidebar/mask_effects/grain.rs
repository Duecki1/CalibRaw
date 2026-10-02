use super::{effect_card, effect_details, float_param_slider};
use crate::pipeline::{effect_params::grain, GrainEffectSettings, MaskEffect};
use eframe::egui::Ui;

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut GrainEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
) -> bool {
    effect_card(
        ui,
        MaskEffect::Grain,
        settings,
        enabled,
        remove,
        |ui, settings| {
            let mut changed = float_param_slider(ui, &mut settings.amount, grain::AMOUNT);
            changed |= float_param_slider(ui, &mut settings.size, grain::SIZE);
            changed |= effect_details(ui, "Texture details", |ui| {
                float_param_slider(ui, &mut settings.roughness, grain::ROUGHNESS)
                    | float_param_slider(ui, &mut settings.color, grain::COLOR)
                    | float_param_slider(ui, &mut settings.seed, grain::SEED)
            });
            changed
        },
    )
}
