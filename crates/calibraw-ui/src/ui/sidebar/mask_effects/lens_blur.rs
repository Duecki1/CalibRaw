use super::{effect_card, effect_details, effect_slider};
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
            let mut changed = effect_slider(ui, &mut settings.amount, lens_blur::AMOUNT);
            changed |= effect_slider(ui, &mut settings.radius, lens_blur::RADIUS);
            changed |= effect_slider(ui, &mut settings.highlight_boost, lens_blur::HIGHLIGHTS);
            changed |= effect_details(ui, "Aperture details", |ui| {
                effect_slider(ui, &mut settings.blades, lens_blur::BLADES)
                    | effect_slider(ui, &mut settings.rotation, lens_blur::ROTATION)
            });
            changed
        },
    )
}
