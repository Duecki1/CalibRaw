use super::{effect_card, effect_color, effect_details, effect_slider};
use crate::pipeline::{effect_params::glow, GlowEffectSettings, MaskEffect};
use eframe::egui::Ui;

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut GlowEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
) -> bool {
    effect_card(
        ui,
        MaskEffect::Glow,
        settings,
        enabled,
        remove,
        |ui, settings| {
            let mut changed = effect_slider(ui, &mut settings.amount, glow::AMOUNT);
            changed |= effect_slider(ui, &mut settings.radius, glow::RADIUS);
            changed |= effect_color(ui, "glow-color-picker", &mut settings.color, glow::COLOR);
            changed |= effect_details(ui, "Light details", |ui| {
                effect_slider(ui, &mut settings.core, glow::CORE)
            });
            changed
        },
    )
}
