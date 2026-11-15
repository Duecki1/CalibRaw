use super::instances::{instance_row, InstanceSelection, SOURCES};
use super::{
    effect_details, effect_position, float_param_slider, styled_effect_card, EffectFrame,
    PositionSpace, StyleChoice,
};
use crate::i18n::tr;
use crate::pipeline::{
    effect_params::light_shafts, LightShaftSource, LightShaftsEffectSettings, LightShaftsMode,
    MaskEffect,
};
use eframe::egui::Ui;

/// The card: its Scene and 2D styles (`style`), the sources, the selected
/// source's controls, then what all sources share: Penetration and, in 2D,
/// the look of the rays.
pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut LightShaftsEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
    frame: &EffectFrame,
    mut selection: InstanceSelection<'_>,
    style: Option<&mut StyleChoice>,
) -> bool {
    styled_effect_card(
        ui,
        MaskEffect::LightShafts,
        settings,
        enabled,
        remove,
        style,
        |ui, settings| {
            let mut changed = instance_row(
                ui,
                &mut settings.sources,
                &mut selection,
                &SOURCES,
                LightShaftSource::swatch,
                LightShaftSource::mirrored,
            );
            if let Some(source) = settings.sources.get_mut(*selection.selected) {
                // Each source's controls keep their own state (pad focus).
                changed |= ui
                    .push_id(*selection.selected, |ui| source_controls(ui, source, frame))
                    .inner;
            }
            changed |= float_param_slider(ui, &mut settings.penetration, light_shafts::PENETRATION);
            if settings.mode == LightShaftsMode::Flat {
                changed |= ray_details(ui, settings);
            }
            changed
        },
    )
}

fn source_controls(ui: &mut Ui, source: &mut LightShaftSource, frame: &EffectFrame) -> bool {
    let mut changed = float_param_slider(ui, &mut source.amount, light_shafts::AMOUNT);
    changed |= float_param_slider(ui, &mut source.length, light_shafts::LENGTH);
    changed |= effect_position(
        ui,
        tr("Source position"),
        &mut source.source,
        [light_shafts::SOURCE_X, light_shafts::SOURCE_Y],
        frame,
        PositionSpace::Source,
    );
    changed |= float_param_slider(ui, &mut source.warmth, light_shafts::WARMTH);
    changed
}

/// The 2D style's ray look; Ray count only while the rays vary.
fn ray_details(ui: &mut Ui, settings: &mut LightShaftsEffectSettings) -> bool {
    effect_details(ui, tr("Ray details"), |ui| {
        let mut changed = float_param_slider(ui, &mut settings.spread, light_shafts::SPREAD);
        changed |= float_param_slider(ui, &mut settings.softness, light_shafts::SOFTNESS);
        changed |= float_param_slider(ui, &mut settings.variation, light_shafts::VARIATION);
        if settings.variation > 0.0 {
            changed |= float_param_slider(ui, &mut settings.ray_count, light_shafts::RAY_COUNT);
        }
        changed
    })
}
