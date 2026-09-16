use super::{effect_card, effect_slider};
use crate::pipeline::{effect_params::lens_blur, LensBlurEffectSettings, MaskEffect};
use eframe::egui::Ui;

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut LensBlurEffectSettings,
    enabled: &mut bool,
) -> bool {
    effect_card(
        ui,
        MaskEffect::LensBlur,
        settings,
        enabled,
        |ui, settings| {
            let mut changed = false;
            changed |= effect_slider(ui, &mut settings.amount, lens_blur::AMOUNT);
            changed |= effect_slider(ui, &mut settings.radius, lens_blur::RADIUS);
            changed |= effect_slider(ui, &mut settings.blades, lens_blur::BLADES);
            changed |= effect_slider(ui, &mut settings.rotation, lens_blur::ROTATION);
            changed |= effect_slider(ui, &mut settings.highlight_boost, lens_blur::HIGHLIGHTS);
            changed
        },
    )
}
