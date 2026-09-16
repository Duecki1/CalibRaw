use super::{effect_card, effect_color, effect_slider};
use crate::pipeline::{effect_params::glow, GlowEffectSettings, MaskEffect};
use eframe::egui::Ui;

pub(crate) fn show(ui: &mut Ui, settings: &mut GlowEffectSettings, enabled: &mut bool) -> bool {
    effect_card(ui, MaskEffect::Glow, settings, enabled, |ui, settings| {
        let mut changed = false;
        changed |= effect_slider(ui, &mut settings.amount, glow::AMOUNT);
        changed |= effect_slider(ui, &mut settings.radius, glow::RADIUS);
        changed |= effect_slider(ui, &mut settings.core, glow::CORE);
        changed |= effect_color(ui, "glow-color-picker", &mut settings.color, glow::COLOR);
        changed
    })
}
