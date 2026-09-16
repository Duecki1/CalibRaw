use super::{effect_card, effect_slider};
use crate::pipeline::{
    effect_params::radial_blur, MaskEffect, RadialBlurEffectSettings, RadialBlurMode,
};
use eframe::egui::{self, Ui};

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut RadialBlurEffectSettings,
    enabled: &mut bool,
) -> bool {
    effect_card(
        ui,
        MaskEffect::RadialBlur,
        settings,
        enabled,
        |ui, settings| {
            let mut changed = false;
            changed |= mode_selector(ui, &mut settings.mode);
            changed |= effect_slider(ui, &mut settings.amount, radial_blur::AMOUNT);
            changed |= effect_slider(ui, &mut settings.strength, radial_blur::STRENGTH);
            changed |= effect_slider(ui, &mut settings.center[0], radial_blur::CENTER_X);
            changed |= effect_slider(ui, &mut settings.center[1], radial_blur::CENTER_Y);
            changed
        },
    )
}

/// Radial blur is the only effect with an enumerated mode in addition to its sliders.
fn mode_selector(ui: &mut Ui, mode: &mut RadialBlurMode) -> bool {
    let mut changed = false;
    crate::ui::theme::property_row(ui, "Mode", |ui| {
        egui::ComboBox::from_id_salt("radial-blur-mode")
            .selected_text(mode.label())
            .show_ui(ui, |ui| {
                for candidate in RadialBlurMode::ALL {
                    changed |= ui
                        .selectable_value(mode, candidate, candidate.label())
                        .changed();
                }
            });
    });
    changed
}
