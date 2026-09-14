use super::{effect_card_action, effect_slider};
use crate::pipeline::{
    effect_params::radial_blur, MaskEffect, RadialBlurEffectSettings, RadialBlurMode,
};
use eframe::egui::{self, Ui};

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut RadialBlurEffectSettings,
    enabled: &mut bool,
) -> bool {
    let mut changed = false;
    let action = super::super::Sidebar::adjustment_card(
        ui,
        MaskEffect::RadialBlur.label(),
        true,
        false,
        *enabled,
        |ui| {
            ui.horizontal(|ui| {
                ui.label("Mode");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::ComboBox::from_id_salt("radial-blur-mode")
                        .selected_text(settings.mode.label())
                        .show_ui(ui, |ui| {
                            for mode in RadialBlurMode::ALL {
                                changed |= ui
                                    .selectable_value(&mut settings.mode, mode, mode.label())
                                    .changed();
                            }
                        });
                });
            });
            changed |= effect_slider(ui, &mut settings.amount, radial_blur::AMOUNT);
            changed |= effect_slider(ui, &mut settings.strength, radial_blur::STRENGTH);
            changed |= effect_slider(ui, &mut settings.center[0], radial_blur::CENTER_X);
            changed |= effect_slider(ui, &mut settings.center[1], radial_blur::CENTER_Y);
        },
    );
    changed | effect_card_action(action, settings, enabled)
}
