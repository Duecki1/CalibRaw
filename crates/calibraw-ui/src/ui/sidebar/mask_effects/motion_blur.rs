use super::{effect_card, effect_slider};
use crate::pipeline::{effect_params::motion_blur, MaskEffect, MotionBlurEffectSettings};
use eframe::egui::Ui;

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut MotionBlurEffectSettings,
    enabled: &mut bool,
) -> bool {
    effect_card(
        ui,
        MaskEffect::MotionBlur,
        settings,
        enabled,
        |ui, settings| {
            let mut changed = false;
            changed |= effect_slider(ui, &mut settings.amount, motion_blur::AMOUNT);
            changed |= effect_slider(ui, &mut settings.distance, motion_blur::DISTANCE);
            changed |= effect_slider(ui, &mut settings.angle, motion_blur::ANGLE);
            changed
        },
    )
}
