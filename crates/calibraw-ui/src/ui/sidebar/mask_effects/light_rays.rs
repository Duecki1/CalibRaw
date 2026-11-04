use super::instances::{instance_row, InstanceSelection, SOURCES};
use super::{
    effect_card, effect_color, effect_details, effect_position, float_param_slider, EffectFrame,
    PositionSpace,
};
use crate::i18n::tr;
use crate::pipeline::{
    effect_params::light_rays, LightRaySource, LightRaysEffectSettings, MaskEffect,
};
use eframe::egui::Ui;

/// The card: the sources, the selected source's controls, then the look of
/// the rays all sources share.
pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut LightRaysEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
    frame: &EffectFrame,
    mut selection: InstanceSelection<'_>,
) -> bool {
    effect_card(
        ui,
        MaskEffect::LightRays,
        settings,
        enabled,
        remove,
        |ui, settings| {
            let mut changed = instance_row(
                ui,
                &mut settings.sources,
                &mut selection,
                &SOURCES,
                |source| source.color,
                LightRaySource::mirrored,
            );
            if let Some(source) = settings.sources.get_mut(*selection.selected) {
                // Each source's controls keep their own state (pad focus,
                // colour picker).
                changed |= ui
                    .push_id(*selection.selected, |ui| source_controls(ui, source, frame))
                    .inner;
            }
            changed |= effect_details(ui, tr("Ray details"), |ui| {
                let mut changed = float_param_slider(ui, &mut settings.spread, light_rays::SPREAD);
                changed |= float_param_slider(ui, &mut settings.fade, light_rays::FADE);
                changed |= float_param_slider(ui, &mut settings.softness, light_rays::SOFTNESS);
                changed |= float_param_slider(ui, &mut settings.variation, light_rays::VARIATION);
                if settings.variation > 0.0 {
                    changed |=
                        float_param_slider(ui, &mut settings.ray_count, light_rays::RAY_COUNT);
                }
                changed
            });
            changed
        },
    )
}

fn source_controls(ui: &mut Ui, source: &mut LightRaySource, frame: &EffectFrame) -> bool {
    let mut changed = float_param_slider(ui, &mut source.amount, light_rays::AMOUNT);
    changed |= float_param_slider(ui, &mut source.length, light_rays::LENGTH);
    changed |= effect_position(
        ui,
        tr("Source position"),
        &mut source.source,
        [light_rays::SOURCE_X, light_rays::SOURCE_Y],
        frame,
        PositionSpace::Source,
    );
    changed |= effect_color(
        ui,
        "light-rays-color-picker",
        &mut source.color,
        light_rays::COLOR,
    );
    changed
}
