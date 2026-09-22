pub(super) mod blur;
pub(super) mod edge_glow;
pub(super) mod fog;
pub(super) mod glow;
pub(super) mod lens_blur;
pub(super) mod light_rays;
pub(super) mod motion_blur;
pub(super) mod neon;
pub(super) mod pixelate;
pub(super) mod radial_blur;
pub(super) mod smoke;
pub(super) mod tilt_shift;

use super::{MaskEffect, Ui};
use crate::pipeline::effect_params::ColorParamSpec;

pub(super) use crate::ui::components::adjustment_slider::float_param_slider as effect_slider;

/// The shared chrome around every mask-effect card: its title, enable state and
/// reset action. Each effect contributes only its own controls through `body`,
/// which reports whether any of them changed.
fn effect_card<Settings>(
    ui: &mut Ui,
    effect: MaskEffect,
    settings: &mut Settings,
    enabled: &mut bool,
    body: impl FnOnce(&mut Ui, &mut Settings) -> bool,
) -> bool
where
    Settings: Default,
{
    let mut changed = false;
    let action = super::Sidebar::adjustment_card(ui, effect.label(), true, false, *enabled, |ui| {
        changed = body(ui, settings)
    });
    // Reset always has to be applied, so the card action is combined with `|`
    // instead of short-circuiting.
    changed | apply_card_action(action, settings)
}

fn apply_card_action<Settings>(
    action: super::adjustment_cards::CardAction,
    settings: &mut Settings,
) -> bool
where
    Settings: Default,
{
    use super::adjustment_cards::CardAction;

    match action {
        CardAction::None | CardAction::Toggle => false,
        CardAction::Reset => {
            *settings = Settings::default();
            true
        }
    }
}

pub(super) fn effect_description(effect: MaskEffect) -> Option<&'static str> {
    match effect {
        MaskEffect::LensBlur => Some(
            "Uses an aperture-shaped scene-linear blur for natural bokeh.",
        ),
        MaskEffect::LightRays => Some(
            "The mask is the light source. Rays converge on the source point and travel beyond the mask.",
        ),
        MaskEffect::Fog => Some(
            "Fog is generated in full-image coordinates and blended through the editable mask.",
        ),
        MaskEffect::Smoke => Some(
            "Smoke is generated in full-image coordinates and blended through the editable mask.",
        ),
        _ => None,
    }
}

fn effect_color(
    ui: &mut Ui,
    id_salt: &'static str,
    color: &mut [f32; 3],
    spec: ColorParamSpec,
) -> bool {
    crate::ui::components::color_picker::sidebar_color_picker(
        ui,
        id_salt,
        color,
        spec.label,
        spec.tooltip,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{LocalMask, MaskKind};

    #[test]
    fn each_effect_reset_preserves_other_effects_adjustments_and_mask_state() {
        fn modify(value: &mut serde_json::Value) {
            match value {
                serde_json::Value::Number(number) if number.is_f64() => {
                    *value = serde_json::Value::from(0.25)
                }
                serde_json::Value::Array(values) => values.iter_mut().for_each(modify),
                serde_json::Value::Object(values) => values.values_mut().for_each(modify),
                _ => {}
            }
        }
        let mut mask = LocalMask::new(MaskKind::Brush, 1);
        mask.enabled = false;
        mask.adjustments.exposure = 1.75;
        let mut settings = serde_json::to_value(mask.effect_settings).unwrap();
        modify(&mut settings);
        mask.effect_settings = serde_json::from_value(settings).unwrap();
        let before = mask;
        macro_rules! check {
            ($field:ident) => {{
                let mut mask = before.clone();
                assert!(apply_card_action(
                    super::super::adjustment_cards::CardAction::Reset,
                    &mut mask.effect_settings.$field,
                ));
                let mut expected = before.clone();
                expected.effect_settings.$field = Default::default();
                assert_eq!(mask, expected, stringify!($field));
            }};
        }
        check!(blur);
        check!(lens_blur);
        check!(motion_blur);
        check!(radial_blur);
        check!(tilt_shift);
        check!(edge_glow);
        check!(glow);
        check!(light_rays);
        check!(neon);
        check!(pixelate);
        check!(fog);
        check!(smoke);
    }
}
