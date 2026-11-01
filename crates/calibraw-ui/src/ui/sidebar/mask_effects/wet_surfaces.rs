use super::{effect_card, effect_details, float_param_slider, pattern_seed};
use crate::i18n::tr;
use crate::pipeline::{effect_params::wet_surfaces, MaskEffect, WetSurfacesEffectSettings};
use eframe::egui::Ui;

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut WetSurfacesEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
) -> bool {
    effect_card(
        ui,
        MaskEffect::WetSurfaces,
        settings,
        enabled,
        remove,
        |ui, settings| {
            let mut changed = float_param_slider(ui, &mut settings.wetness, wet_surfaces::WETNESS);
            changed |= float_param_slider(ui, &mut settings.puddles, wet_surfaces::PUDDLES);
            changed |= float_param_slider(ui, &mut settings.reflections, wet_surfaces::REFLECTIONS);
            changed |= float_param_slider(ui, &mut settings.gloss, wet_surfaces::GLOSS);
            changed |= effect_details(ui, tr("Water details"), |ui| {
                float_param_slider(ui, &mut settings.puddle_size, wet_surfaces::PUDDLE_SIZE)
                    | float_param_slider(ui, &mut settings.ripples, wet_surfaces::RIPPLES)
                    | pattern_seed(ui, &mut settings.seed, wet_surfaces::SEED)
            });
            changed
        },
    )
}
