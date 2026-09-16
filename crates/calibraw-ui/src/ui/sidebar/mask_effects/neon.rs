use super::{effect_card, effect_color, effect_slider};
use crate::pipeline::{effect_params::neon, MaskEffect, NeonEffectSettings};
use eframe::egui::Ui;

pub(crate) fn show(ui: &mut Ui, settings: &mut NeonEffectSettings, enabled: &mut bool) -> bool {
    effect_card(ui, MaskEffect::Neon, settings, enabled, |ui, settings| {
        let mut changed = false;
        changed |= effect_slider(ui, &mut settings.amount, neon::AMOUNT);
        changed |= effect_slider(ui, &mut settings.edge_width, neon::EDGE_WIDTH);
        changed |= effect_slider(ui, &mut settings.detail, neon::DETAIL);
        changed |= effect_slider(ui, &mut settings.glow, neon::GLOW);
        changed |= effect_slider(ui, &mut settings.background, neon::BACKGROUND);
        changed |= effect_color(ui, "neon-color-picker", &mut settings.color, neon::COLOR);
        changed
    })
}
