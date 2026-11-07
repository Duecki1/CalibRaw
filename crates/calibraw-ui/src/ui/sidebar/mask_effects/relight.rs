use super::instances::{instance_row, InstanceSelection, LIGHTS};
use super::{
    effect_card, effect_color, effect_details, effect_position, effect_toggle, float_param_slider,
    EffectFrame, PositionSpace,
};
use crate::i18n::tr;
use crate::pipeline::{effect_params::relight, MaskEffect, RelightEffectSettings, RelightLight};
use eframe::egui::Ui;

/// The card: the lights, the selected light's controls, then the Ambient
/// and Relief all lights share.
pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut RelightEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
    frame: &EffectFrame,
    mut selection: InstanceSelection<'_>,
) -> bool {
    effect_card(
        ui,
        MaskEffect::Relight,
        settings,
        enabled,
        remove,
        |ui, settings| {
            let mut changed = instance_row(
                ui,
                &mut settings.lights,
                &mut selection,
                &LIGHTS,
                |light| light.color,
                RelightLight::mirrored,
            );
            if let Some(light) = settings.lights.get_mut(*selection.selected) {
                // Each light's controls keep their own state (pad focus,
                // open details, colour picker).
                changed |= ui
                    .push_id(*selection.selected, |ui| light_controls(ui, light, frame))
                    .inner;
            }
            changed |= float_param_slider(ui, &mut settings.ambient, relight::AMBIENT);
            changed |= effect_details(ui, tr("Scene details"), |ui| {
                float_param_slider(ui, &mut settings.relief_scale, relight::RELIEF_SCALE)
            });
            changed
        },
    )
}

fn light_controls(ui: &mut Ui, light: &mut RelightLight, frame: &EffectFrame) -> bool {
    let mut changed = float_param_slider(ui, &mut light.amount, relight::AMOUNT);
    changed |= effect_position(
        ui,
        tr("Light position"),
        &mut light.source,
        [relight::SOURCE_X, relight::SOURCE_Y],
        frame,
        PositionSpace::Source,
    );
    changed |= float_param_slider(ui, &mut light.light_depth, relight::LIGHT_DEPTH);
    changed |= effect_color(ui, "relight-color-picker", &mut light.color, relight::COLOR);
    changed |= effect_toggle(ui, &mut light.shadows_enabled, relight::CAST_SHADOWS);
    changed |= effect_details(ui, tr("Light details"), |ui| {
        let mut changed = float_param_slider(ui, &mut light.reach, relight::REACH);
        changed |= float_param_slider(ui, &mut light.size, relight::SIZE);
        if light.shadows_enabled {
            changed |= float_param_slider(ui, &mut light.shadows, relight::SHADOWS);
        }
        changed
    });
    changed
}
