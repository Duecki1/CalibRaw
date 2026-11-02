use super::{
    effect_card, effect_details, float_param_angle, float_param_slider, image_lights_toggle,
    pattern_seed,
};
use crate::i18n::tr;
use crate::pipeline::{effect_params::rain, MaskEffect, RainEffectSettings};
use eframe::egui::Ui;

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut RainEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
) -> bool {
    effect_card(
        ui,
        MaskEffect::Rain,
        settings,
        enabled,
        remove,
        |ui, settings| {
            let mut changed = float_param_slider(ui, &mut settings.amount, rain::AMOUNT);
            changed |= float_param_slider(ui, &mut settings.length, rain::LENGTH);
            changed |= float_param_angle(ui, &mut settings.angle, rain::ANGLE);
            changed |= float_param_slider(ui, &mut settings.mist, rain::MIST);
            changed |= moduwu_design::toggle_with_help(
                ui,
                &mut settings.depth_enabled,
                tr("Scene depth"),
                tr("Generate and use shared scene depth so rain falls behind nearer objects and mist builds with distance. Turn off for rain in front of the whole photo."),
            )
            .changed();
            changed |= float_param_slider(ui, &mut settings.light_glow, rain::LIGHT_GLOW);
            changed |= image_lights_toggle(ui, &mut settings.image_lights);
            changed |= effect_details(ui, tr("Rain details"), |ui| {
                let mut changed = float_param_slider(ui, &mut settings.drop_size, rain::DROP_SIZE);
                // Splashes need the ground in scene depth, and with scene depth the
                // photo itself says how much the lens blurs each distance.
                changed |= if settings.depth_enabled {
                    float_param_slider(ui, &mut settings.splashes, rain::SPLASHES)
                } else {
                    float_param_slider(ui, &mut settings.focus, rain::FOCUS)
                };
                changed |= float_param_slider(ui, &mut settings.gusts, rain::GUSTS);
                changed |= float_param_slider(ui, &mut settings.perspective, rain::PERSPECTIVE);
                changed | pattern_seed(ui, &mut settings.seed, rain::SEED)
            });
            changed
        },
    )
}
