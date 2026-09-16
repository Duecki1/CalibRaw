use super::{effect_card, effect_slider};
use crate::pipeline::{effect_params::pixelate, MaskEffect, PixelateEffectSettings};
use eframe::egui::Ui;

pub(crate) fn show(ui: &mut Ui, settings: &mut PixelateEffectSettings, enabled: &mut bool) -> bool {
    effect_card(
        ui,
        MaskEffect::Pixelate,
        settings,
        enabled,
        |ui, settings| {
            let mut changed = false;
            changed |= effect_slider(ui, &mut settings.amount, pixelate::AMOUNT);
            changed |= effect_slider(ui, &mut settings.block_size, pixelate::BLOCK_SIZE);
            changed
        },
    )
}
