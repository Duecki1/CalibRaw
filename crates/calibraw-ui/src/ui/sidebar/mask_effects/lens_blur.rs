use super::{effect_card_action, effect_slider};
use crate::pipeline::{effect_params::lens_blur, LensBlurEffectSettings, MaskEffect};
use eframe::egui::Ui;

pub(crate) fn show(ui: &mut Ui, settings: &mut LensBlurEffectSettings, enabled: &mut bool) -> bool {
    let mut changed = false;
    let action = super::super::Sidebar::adjustment_card(
        ui,
        MaskEffect::LensBlur.label(),
        true,
        false,
        *enabled,
        |ui| {
            changed |= effect_slider(ui, &mut settings.amount, lens_blur::AMOUNT);
            changed |= effect_slider(ui, &mut settings.radius, lens_blur::RADIUS);
            changed |= effect_slider(ui, &mut settings.blades, lens_blur::BLADES);
            changed |= effect_slider(ui, &mut settings.rotation, lens_blur::ROTATION);
            changed |= effect_slider(ui, &mut settings.highlight_boost, lens_blur::HIGHLIGHTS);
        },
    );
    changed | effect_card_action(action, settings, enabled)
}
