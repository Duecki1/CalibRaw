use super::{effect_card_action, effect_slider};
use crate::pipeline::{effect_params::blur, BlurEffectSettings, MaskEffect};
use eframe::egui::Ui;

pub(crate) fn show(ui: &mut Ui, settings: &mut BlurEffectSettings, enabled: &mut bool) -> bool {
    let mut changed = false;
    let action = super::super::Sidebar::adjustment_card(
        ui,
        MaskEffect::Blur.label(),
        true,
        false,
        *enabled,
        |ui| {
            changed |= effect_slider(ui, &mut settings.amount, blur::AMOUNT);
            changed |= effect_slider(ui, &mut settings.radius, blur::RADIUS);
        },
    );
    changed | effect_card_action(action, settings, enabled)
}
