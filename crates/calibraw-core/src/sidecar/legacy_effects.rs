//! Glow, Halation, Grain and Vignette sliders of edits saved before these
//! effects became effect components.
//!
//! They were sliders on the Effects card, and Halation was also a slider in
//! every mask's adjustments. Edits now hold them only as effect components.
//! Sidecars and presets that still carry the old fields are read through
//! [`LegacyEffectSliders`], and each edited slider becomes the matching
//! component: a global effect for the Effects card, a component of the mask
//! for a mask's Halation.
//!
//! Components render with their own algorithms, so a migrated edit resembles
//! the saved one rather than matching it. Glow keeps its amount and radius as
//! a neutral highlight Glow; its threshold has no component equivalent.

use super::EditState;
use crate::pipeline::{
    effect_params, EffectComponent, GlowEffectSettings, GrainEffectSettings,
    HalationEffectSettings, InitialEffectSettings, MaskEffect, MaskEffectSettings,
    VignetteEffectSettings, MAX_EFFECT_COMPONENTS,
};
use serde::Deserialize;
use std::sync::Arc;

/// Below this magnitude a slider was at rest, as the renderer treated it.
const ACTIVE_EPSILON: f32 = 1e-6;

/// The old slider fields of a serialized `edits` object. Every other field
/// is ignored, so this reads any edit, old or current.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct LegacyEffectSliders {
    #[serde(default)]
    exposure: GlobalSliders,
    #[serde(default)]
    masks: MaskStackSliders,
}

/// Effects-card sliders. Missing fields take the sliders' former defaults.
#[derive(Debug, Deserialize)]
#[serde(default)]
struct GlobalSliders {
    halation_amount: f32,
    grain_amount: f32,
    glow_amount: f32,
    glow_radius: f32,
    vignette_amount: f32,
    vignette_midpoint: f32,
    vignette_roundness: f32,
    vignette_feather: f32,
    vignette_highlights: f32,
}

impl Default for GlobalSliders {
    fn default() -> Self {
        Self {
            halation_amount: 0.0,
            grain_amount: 0.0,
            glow_amount: 0.0,
            glow_radius: 50.0,
            vignette_amount: 0.0,
            vignette_midpoint: 50.0,
            vignette_roundness: 0.0,
            vignette_feather: 50.0,
            vignette_highlights: 0.0,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct MaskStackSliders {
    /// In the same order as the decoded edit's masks.
    #[serde(default)]
    masks: Vec<MaskSliders>,
}

#[derive(Debug, Default, Deserialize)]
struct MaskSliders {
    #[serde(default)]
    adjustments: LocalSliders,
}

#[derive(Debug, Default, Deserialize)]
struct LocalSliders {
    #[serde(default)]
    halation_amount: f32,
}

impl LegacyEffectSliders {
    /// Drops the Effects-card sliders and keeps those of the masks.
    pub(crate) fn without_global(self) -> Self {
        Self {
            exposure: GlobalSliders::default(),
            ..self
        }
    }

    /// Adds a component to `edits` for every edited slider and returns whether
    /// any slider was edited.
    ///
    /// `edits` must be decoded from the same `edits` object as `self`, before
    /// any mask is removed or reordered. An effect already present as a
    /// component keeps that component, and a full component list keeps its
    /// components, so their slider is dropped.
    pub(crate) fn migrate(self, edits: &mut EditState) -> bool {
        let global = self.exposure.components();
        let masked = self
            .masks
            .masks
            .iter()
            .any(|mask| mask.adjustments.halation().is_some());
        if global.is_empty() && !masked {
            return false;
        }

        let masks = Arc::make_mut(&mut edits.masks);
        for component in global {
            add_component(&mut masks.global_effects, component);
        }
        for (mask, sliders) in masks.masks.iter_mut().zip(&self.masks.masks) {
            // Masks with a single legacy effect never applied their adjustments.
            if !mask.effect.uses_adjustments() {
                continue;
            }
            if let Some(mut halation) = sliders.adjustments.halation() {
                halation.enabled = mask.adjustments_enabled;
                add_component(&mut mask.effect_components, halation);
            }
        }
        true
    }
}

impl GlobalSliders {
    fn components(&self) -> Vec<EffectComponent> {
        let mut components = Vec::new();
        if self.glow_amount.abs() > ACTIVE_EPSILON {
            use effect_params::glow;
            components.push(component(
                MaskEffect::Glow,
                MaskEffectSettings {
                    glow: GlowEffectSettings {
                        amount: glow::AMOUNT.clamp(self.glow_amount),
                        radius: glow::RADIUS.clamp(self.glow_radius),
                        // The Effects-card Glow kept the colour of its highlights.
                        color: [1.0; 3],
                        ..GlowEffectSettings::initial()
                    },
                    ..MaskEffectSettings::default()
                },
            ));
        }
        if self.halation_amount > ACTIVE_EPSILON {
            components.push(halation(self.halation_amount));
        }
        if self.grain_amount > ACTIVE_EPSILON {
            components.push(component(
                MaskEffect::Grain,
                MaskEffectSettings {
                    grain: GrainEffectSettings {
                        amount: effect_params::grain::AMOUNT.clamp(self.grain_amount),
                        ..GrainEffectSettings::initial()
                    },
                    ..MaskEffectSettings::default()
                },
            ));
        }
        if self.vignette_amount.abs() > ACTIVE_EPSILON {
            use effect_params::vignette;
            components.push(component(
                MaskEffect::Vignette,
                MaskEffectSettings {
                    vignette: VignetteEffectSettings {
                        amount: vignette::AMOUNT.clamp(self.vignette_amount),
                        midpoint: vignette::MIDPOINT.clamp(self.vignette_midpoint),
                        roundness: vignette::ROUNDNESS.clamp(self.vignette_roundness),
                        feather: vignette::FEATHER.clamp(self.vignette_feather),
                        highlights: vignette::HIGHLIGHTS.clamp(self.vignette_highlights),
                        ..VignetteEffectSettings::initial()
                    },
                    ..MaskEffectSettings::default()
                },
            ));
        }
        components
    }
}

impl LocalSliders {
    fn halation(&self) -> Option<EffectComponent> {
        (self.halation_amount > ACTIVE_EPSILON).then(|| halation(self.halation_amount))
    }
}

fn halation(amount: f32) -> EffectComponent {
    component(
        MaskEffect::Halation,
        MaskEffectSettings {
            halation: HalationEffectSettings {
                amount: effect_params::halation::AMOUNT.clamp(amount),
                ..HalationEffectSettings::initial()
            },
            ..MaskEffectSettings::default()
        },
    )
}

fn component(effect: MaskEffect, settings: MaskEffectSettings) -> EffectComponent {
    EffectComponent {
        effect,
        enabled: true,
        settings,
    }
}

fn add_component(components: &mut Vec<EffectComponent>, component: EffectComponent) {
    let present = components
        .iter()
        .any(|existing| existing.effect == component.effect);
    if !present && components.len() < MAX_EFFECT_COMPONENTS {
        components.push(component);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::MaskKind;
    use crate::sidecar::default_edit_state;
    use serde_json::json;

    fn sliders(edits: serde_json::Value) -> LegacyEffectSliders {
        serde_json::from_value(edits).unwrap()
    }

    fn global_effect(edits: &EditState, effect: MaskEffect) -> &EffectComponent {
        edits
            .masks
            .global_effects
            .iter()
            .find(|component| component.effect == effect)
            .unwrap_or_else(|| panic!("no global {effect:?}"))
    }

    #[test]
    fn edited_effects_card_sliders_become_global_components() {
        let mut edits = default_edit_state();
        let migrated = sliders(json!({"exposure": {
            "halation_amount": 40.0,
            "grain_amount": 30.0,
            "glow_amount": 20.0,
            "glow_radius": 70.0,
            "glow_threshold": 10.0,
            "vignette_amount": -35.0,
            "vignette_midpoint": 40.0,
            "vignette_roundness": 25.0,
            "vignette_feather": 60.0,
            "vignette_highlights": 15.0,
        }}))
        .migrate(&mut edits);

        assert!(migrated);
        let effects: Vec<_> = edits
            .masks
            .global_effects
            .iter()
            .map(|component| component.effect)
            .collect();
        assert_eq!(
            effects,
            [
                MaskEffect::Glow,
                MaskEffect::Halation,
                MaskEffect::Grain,
                MaskEffect::Vignette
            ]
        );
        assert!(edits
            .masks
            .global_effects
            .iter()
            .all(|component| component.enabled && component.is_active()));

        let glow = global_effect(&edits, MaskEffect::Glow).settings.glow;
        assert!(!glow.self_illuminating);
        assert_eq!(
            (glow.amount, glow.radius, glow.color),
            (20.0, 70.0, [1.0; 3])
        );
        assert_eq!(
            global_effect(&edits, MaskEffect::Halation)
                .settings
                .halation
                .amount,
            40.0
        );
        assert_eq!(
            global_effect(&edits, MaskEffect::Grain).settings.grain,
            GrainEffectSettings {
                amount: 30.0,
                ..GrainEffectSettings::default()
            }
        );
        assert_eq!(
            global_effect(&edits, MaskEffect::Vignette)
                .settings
                .vignette,
            VignetteEffectSettings {
                amount: -35.0,
                midpoint: 40.0,
                roundness: 25.0,
                feather: 60.0,
                highlights: 15.0,
                ..VignetteEffectSettings::default()
            }
        );
    }

    #[test]
    fn resting_or_missing_sliders_leave_the_edit_untouched() {
        let mut edits = default_edit_state();
        let untouched = edits.clone();
        let resting = json!({
            "exposure": {
                "halation_amount": 0.0,
                "grain_amount": 0.0,
                "glow_amount": 0.0,
                "glow_radius": 80.0,
                "vignette_amount": 0.0,
                "vignette_midpoint": 10.0,
            },
            "masks": {"masks": [{"adjustments": {"halation_amount": 0.0}}]},
        });
        for value in [resting, json!({}), json!({"exposure": {}, "masks": {}})] {
            assert!(!sliders(value).migrate(&mut edits));
            assert_eq!(edits, untouched);
        }
    }

    #[test]
    fn mask_halation_becomes_a_component_of_its_mask() {
        let mut edits = default_edit_state();
        let masks = Arc::make_mut(&mut edits.masks);
        for _ in 0..3 {
            masks.add_mask(MaskKind::Fullscreen).unwrap();
        }
        masks.masks[1].adjustments_enabled = false;
        // A mask with a single legacy effect never applied its adjustments.
        masks.masks[2].effect = MaskEffect::Blur;

        let migrated = sliders(json!({"masks": {"masks": [
            {"adjustments": {"halation_amount": 45.0}},
            {"adjustments": {"halation_amount": 55.0}},
            {"adjustments": {"halation_amount": 65.0}},
        ]}}))
        .migrate(&mut edits);

        assert!(migrated);
        assert!(edits.masks.global_effects.is_empty());
        let halation = |index: usize| -> Vec<(bool, f32)> {
            edits.masks.masks[index]
                .effect_components
                .iter()
                .map(|component| {
                    assert_eq!(component.effect, MaskEffect::Halation);
                    (component.enabled, component.settings.halation.amount)
                })
                .collect()
        };
        assert_eq!(halation(0), [(true, 45.0)]);
        // Hidden adjustments stay hidden.
        assert_eq!(halation(1), [(false, 55.0)]);
        assert!(halation(2).is_empty());
    }

    #[test]
    fn existing_components_and_full_lists_take_precedence() {
        let mut edits = default_edit_state();
        let mut grain = EffectComponent::new(MaskEffect::Grain);
        grain.settings.grain.amount = 80.0;
        Arc::make_mut(&mut edits.masks).global_effects.push(grain);

        assert!(sliders(json!({"exposure": {"grain_amount": 10.0}})).migrate(&mut edits));
        assert_eq!(edits.masks.global_effects.len(), 1);
        assert_eq!(edits.masks.global_effects[0].settings.grain.amount, 80.0);

        let mut full = default_edit_state();
        let effects = MaskEffect::ALL
            .into_iter()
            .filter(|effect| !matches!(effect, MaskEffect::Adjustment | MaskEffect::Vignette))
            .take(MAX_EFFECT_COMPONENTS);
        Arc::make_mut(&mut full.masks).global_effects = effects.map(EffectComponent::new).collect();
        let before = full.masks.global_effects.clone();
        assert!(sliders(json!({"exposure": {"vignette_amount": -20.0}})).migrate(&mut full));
        assert_eq!(full.masks.global_effects, before);
    }

    #[test]
    fn out_of_range_sliders_are_clamped_to_component_ranges() {
        let mut edits = default_edit_state();
        sliders(json!({"exposure": {"grain_amount": 500.0, "vignette_amount": -500.0}}))
            .migrate(&mut edits);
        assert_eq!(
            global_effect(&edits, MaskEffect::Grain)
                .settings
                .grain
                .amount,
            effect_params::grain::AMOUNT.max
        );
        assert_eq!(
            global_effect(&edits, MaskEffect::Vignette)
                .settings
                .vignette
                .amount,
            effect_params::vignette::AMOUNT.min
        );
    }

    #[test]
    fn without_global_keeps_only_mask_sliders() {
        let mut edits = default_edit_state();
        Arc::make_mut(&mut edits.masks)
            .add_mask(MaskKind::Fullscreen)
            .unwrap();
        sliders(json!({
            "exposure": {"grain_amount": 30.0},
            "masks": {"masks": [{"adjustments": {"halation_amount": 45.0}}]},
        }))
        .without_global()
        .migrate(&mut edits);
        assert!(edits.masks.global_effects.is_empty());
        assert_eq!(edits.masks.masks[0].effect_components.len(), 1);
    }
}
