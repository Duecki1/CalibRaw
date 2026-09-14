use super::{effect_card_action, effect_slider};
use crate::pipeline::{effect_params::pixelate, MaskEffect, PixelateEffectSettings};
use eframe::egui::Ui;

pub(crate) fn show(ui: &mut Ui, settings: &mut PixelateEffectSettings, enabled: &mut bool) -> bool {
    let mut changed = false;
    let action = super::super::Sidebar::adjustment_card(
        ui,
        MaskEffect::Pixelate.label(),
        true,
        false,
        *enabled,
        |ui| {
            changed |= effect_slider(ui, &mut settings.amount, pixelate::AMOUNT);
            changed |= effect_slider(ui, &mut settings.block_size, pixelate::BLOCK_SIZE);
        },
    );
    changed | effect_card_action(action, settings, enabled)
}
