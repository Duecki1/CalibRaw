use super::{effect_card, effect_details, effect_position, float_param_slider};
use crate::pipeline::{effect_params::tilt_shift, MaskEffect, TiltShiftEffectSettings};
use eframe::egui::Ui;

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut TiltShiftEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
    is_fullscreen_mask: bool,
) -> bool {
    effect_card(
        ui,
        MaskEffect::TiltShift,
        settings,
        enabled,
        remove,
        |ui, settings| {
            if !is_fullscreen_mask {
                ui.add(eframe::egui::Label::new(eframe::egui::RichText::new(
                    "This mask also limits the blur. Feather its edge for a smooth transition."
                ).small()).wrap());
            }
            let mut changed = float_param_slider(ui, &mut settings.amount, tilt_shift::AMOUNT);
            changed |= float_param_slider(ui, &mut settings.radius, tilt_shift::RADIUS);
            changed |= float_param_slider(ui, &mut settings.focus_width, tilt_shift::FOCUS_WIDTH);
            changed |= effect_details(ui, "Focus band details", |ui| {
                let mut changed = effect_position(
                    ui,
                    "Focus position",
                    &mut settings.center,
                    [tilt_shift::CENTER_X, tilt_shift::CENTER_Y],
                );
                changed |= float_param_slider(ui, &mut settings.angle, tilt_shift::ANGLE);
                changed |= float_param_slider(ui, &mut settings.feather, tilt_shift::FEATHER);
                changed
            });
            changed
        },
    )
}
