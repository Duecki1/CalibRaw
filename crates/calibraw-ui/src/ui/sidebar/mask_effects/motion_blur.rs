use super::{effect_card_action, effect_slider};
use crate::pipeline::{effect_params::motion_blur, MaskEffect, MotionBlurEffectSettings};
use eframe::egui::Ui;

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut MotionBlurEffectSettings,
    enabled: &mut bool,
) -> bool {
    let mut changed = false;
    let action = super::super::Sidebar::adjustment_card(
        ui,
        MaskEffect::MotionBlur.label(),
        true,
        false,
        *enabled,
        |ui| {
            changed |= effect_slider(ui, &mut settings.amount, motion_blur::AMOUNT);
            changed |= effect_slider(ui, &mut settings.distance, motion_blur::DISTANCE);
            changed |= effect_slider(ui, &mut settings.angle, motion_blur::ANGLE);
        },
    );
    changed | effect_card_action(action, settings, enabled)
}
