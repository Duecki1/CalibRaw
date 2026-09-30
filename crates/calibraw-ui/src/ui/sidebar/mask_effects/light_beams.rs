use super::{effect_card, effect_color, effect_slider};
use crate::pipeline::{
    effect_params::light_beams, LightBeamPreset, LightBeamsEffectSettings, MaskEffect,
};
use eframe::egui::{self, Ui};

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut LightBeamsEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
) -> bool {
    effect_card(
        ui,
        MaskEffect::LightBeams,
        settings,
        enabled,
        remove,
        |ui, settings| {
            let mut changed = preset_selector(ui, settings);
            changed |= effect_slider(ui, &mut settings.amount, light_beams::AMOUNT);
            changed |= effect_slider(ui, &mut settings.length, light_beams::LENGTH);
            changed |= effect_slider(ui, &mut settings.source[0], light_beams::SOURCE_X);
            changed |= effect_slider(ui, &mut settings.source[1], light_beams::SOURCE_Y);
            changed |= effect_slider(ui, &mut settings.direction, light_beams::DIRECTION);
            changed |= effect_slider(ui, &mut settings.spread, light_beams::SPREAD);
            changed |= effect_slider(ui, &mut settings.softness, light_beams::SOFTNESS);
            changed |= effect_slider(ui, &mut settings.source_depth, light_beams::SOURCE_DEPTH);
            changed |= effect_slider(ui, &mut settings.scattering, light_beams::SCATTERING);
            changed |= effect_color(
                ui,
                "light-beams-color-picker",
                &mut settings.color,
                light_beams::COLOR,
            );
            changed
        },
    )
}

fn preset_selector(ui: &mut Ui, settings: &mut LightBeamsEffectSettings) -> bool {
    let mut preset = settings.preset;
    let mut changed = false;
    crate::ui::theme::property_row(ui, "Preset", |ui| {
        egui::ComboBox::from_id_salt("light-beams-preset")
            .selected_text(preset.label())
            .show_ui(ui, |ui| {
                for candidate in LightBeamPreset::ALL {
                    changed |= ui
                        .selectable_value(&mut preset, candidate, candidate.label())
                        .changed();
                }
            });
    });
    if changed {
        apply_preset(settings, preset);
    }
    changed
}

fn apply_preset(settings: &mut LightBeamsEffectSettings, preset: LightBeamPreset) {
    let source = settings.source;
    let source_depth = settings.source_depth;
    *settings = LightBeamsEffectSettings::from_preset(preset);
    settings.source = source;
    settings.source_depth = source_depth;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_presets_preserves_placed_light_position_and_depth() {
        for preset in LightBeamPreset::ALL {
            let mut settings = LightBeamsEffectSettings {
                source: [-25.0, 75.0],
                source_depth: 63.0,
                ..Default::default()
            };
            let mut expected = LightBeamsEffectSettings::from_preset(preset);
            expected.source = settings.source;
            expected.source_depth = settings.source_depth;

            apply_preset(&mut settings, preset);

            assert_eq!(settings, expected);
        }
    }
}
