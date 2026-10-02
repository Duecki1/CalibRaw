use super::{effect_card, effect_color, effect_details, effect_position, effect_slider};
use crate::pipeline::{effect_params::light_rays, LightRaysEffectSettings, MaskEffect};
use eframe::egui::Ui;

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut LightRaysEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
) -> bool {
    effect_card(
        ui,
        MaskEffect::LightRays,
        settings,
        enabled,
        remove,
        |ui, settings| {
            let mut changed = effect_slider(ui, &mut settings.amount, light_rays::AMOUNT);
            changed |= effect_slider(ui, &mut settings.length, light_rays::LENGTH);
            changed |= effect_position(
                ui,
                "Source position",
                &mut settings.source,
                [light_rays::SOURCE_X, light_rays::SOURCE_Y],
            );
            changed |= effect_color(
                ui,
                "light-rays-color-picker",
                &mut settings.color,
                light_rays::COLOR,
            );
            changed |= effect_details(ui, "Ray details", |ui| {
                let mut changed = effect_slider(ui, &mut settings.spread, light_rays::SPREAD);
                changed |= effect_slider(ui, &mut settings.fade, light_rays::FADE);
                changed |= effect_slider(ui, &mut settings.softness, light_rays::SOFTNESS);
                changed |= effect_slider(ui, &mut settings.variation, light_rays::VARIATION);
                if settings.variation > 0.0 {
                    changed |= effect_slider(ui, &mut settings.ray_count, light_rays::RAY_COUNT);
                }
                changed
            });
            changed
        },
    )
}
