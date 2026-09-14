use super::{effect_card_action, effect_slider};
use crate::pipeline::{effect_params::tilt_shift, MaskEffect, TiltShiftEffectSettings};
use eframe::egui::Ui;

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut TiltShiftEffectSettings,
    enabled: &mut bool,
    is_fullscreen_mask: bool,
) -> bool {
    let mut changed = false;
    let action = super::super::Sidebar::adjustment_card(
        ui,
        MaskEffect::TiltShift.label(),
        true,
        false,
        *enabled,
        |ui| {
            if !is_fullscreen_mask {
                ui.colored_label(
                ui.visuals().warn_fg_color,
                "Best used with a Fullscreen mask. Other masks clip the built-in focus band and can create an abrupt blur boundary.",
            );
                ui.add_space(3.0);
            }
            changed |= effect_slider(ui, &mut settings.amount, tilt_shift::AMOUNT);
            changed |= effect_slider(ui, &mut settings.radius, tilt_shift::RADIUS);
            changed |= effect_slider(ui, &mut settings.center[0], tilt_shift::CENTER_X);
            changed |= effect_slider(ui, &mut settings.center[1], tilt_shift::CENTER_Y);
            changed |= effect_slider(ui, &mut settings.angle, tilt_shift::ANGLE);
            changed |= effect_slider(ui, &mut settings.focus_width, tilt_shift::FOCUS_WIDTH);
            changed |= effect_slider(ui, &mut settings.feather, tilt_shift::FEATHER);
        },
    );
    changed | effect_card_action(action, settings, enabled)
}
