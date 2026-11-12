//! The card of an effect that draws none of its own: its controls in the order
//! its settings list them (`EffectSettings::controls`), the details collapsed.

use super::{
    effect_color, effect_details, effect_toggle, float_param_angle, float_param_slider,
    pattern_seed, styled_effect_card, MaskEffect, StyleChoice, Ui,
};
use crate::i18n::tr;
use crate::pipeline::{EffectControl, EffectSettings, EffectSettingsVisitor, MaskEffectSettings};

/// Draws `effect`'s card from its controls, led by its Style control when
/// its effect has kin. Returns whether its settings or visibility changed.
pub(in crate::ui::sidebar) fn generated_card(
    ui: &mut Ui,
    effect: MaskEffect,
    settings: &mut MaskEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
    style: Option<&mut StyleChoice>,
) -> bool {
    settings
        .visit_mut(
            effect,
            GeneratedCard {
                ui,
                enabled,
                remove,
                style,
            },
        )
        .unwrap_or(false)
}

struct GeneratedCard<'a> {
    ui: &'a mut Ui,
    enabled: &'a mut bool,
    remove: &'a mut bool,
    style: Option<&'a mut StyleChoice>,
}

impl EffectSettingsVisitor for GeneratedCard<'_> {
    type Output = bool;

    fn visit<S: EffectSettings>(self, effect: MaskEffect, settings: &mut S) -> bool {
        styled_effect_card(
            self.ui,
            effect,
            settings,
            self.enabled,
            self.remove,
            self.style,
            |ui, settings| {
                let controls = settings.controls();
                let mut changed = show_controls(ui, effect, controls.shown);
                if let Some(details) = controls.details {
                    changed |= effect_details(ui, tr(details.title), |ui| {
                        show_controls(ui, effect, details.controls)
                    });
                }
                changed
            },
        )
    }
}

fn show_controls(ui: &mut Ui, effect: MaskEffect, controls: Vec<EffectControl<'_>>) -> bool {
    let mut changed = false;
    for control in controls {
        changed |= match control {
            EffectControl::Slider(value, spec) => float_param_slider(ui, value, spec),
            EffectControl::Angle(value, spec) => float_param_angle(ui, value, spec),
            EffectControl::Seed(value, spec) => pattern_seed(ui, value, spec),
            EffectControl::Color(color, spec) => {
                let id_salt = format!("{}-color-picker", effect.shader_name().replace('_', "-"));
                effect_color(ui, id_salt, color, spec)
            }
            EffectControl::Toggle(value, spec) => effect_toggle(ui, value, spec),
            // Without the effect's frame a position shows as its coordinates.
            EffectControl::Position(position, [x, y]) => {
                float_param_slider(ui, &mut position[0], x)
                    | float_param_slider(ui, &mut position[1], y)
            }
        };
    }
    changed
}
