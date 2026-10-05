use super::{effect_card, effect_details, float_param_angle, float_param_slider};
use crate::pipeline::{effect_params::lens_blur, LensBlurEffectSettings, MaskEffect};
use eframe::egui::Ui;

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut LensBlurEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
) -> bool {
    effect_card(
        ui,
        MaskEffect::LensBlur,
        settings,
        enabled,
        remove,
        |ui, settings| {
            let mut changed = float_param_slider(ui, &mut settings.amount, lens_blur::AMOUNT);
            changed |= float_param_slider(ui, &mut settings.radius, lens_blur::RADIUS);
            changed |= float_param_slider(ui, &mut settings.highlight_boost, lens_blur::HIGHLIGHTS);
            changed |= effect_details(ui, "Aperture details", |ui| {
                float_param_slider(ui, &mut settings.blades, lens_blur::BLADES)
                    | float_param_angle(ui, &mut settings.rotation, lens_blur::ROTATION)
            });
            changed
        },
    )
}
