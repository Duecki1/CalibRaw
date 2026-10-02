use super::{effect_card, effect_details, effect_slider};
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
            let mut changed = effect_slider(ui, &mut settings.amount, grain::AMOUNT);
            changed |= effect_slider(ui, &mut settings.size, grain::SIZE);
            changed |= effect_details(ui, "Texture details", |ui| {
                effect_slider(ui, &mut settings.roughness, grain::ROUGHNESS)
                    | effect_slider(ui, &mut settings.color, grain::COLOR)
                    | effect_slider(ui, &mut settings.seed, grain::SEED)
            });
            changed
        },
    )
}
