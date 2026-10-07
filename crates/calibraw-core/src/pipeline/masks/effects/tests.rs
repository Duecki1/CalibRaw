use crate::pipeline::{
    EffectComponent, GrainEffectSettings, HalationEffectSettings, MaskEffect, MaskEffectCategory,
    MaskEffectSettings, MaskKind, MaskStack, VignetteEffectSettings,
};
use serde_json::json;

#[test]
fn effect_names_and_shader_ids_remain_compatible() {
    let effects = [
        (MaskEffect::Adjustment, "Adjustment", 0),
        (MaskEffect::Neon, "Neon", 1),
        (MaskEffect::Glow, "Glow", 2),
        (MaskEffect::LightRays, "LightRays", 3),
        (MaskEffect::Blur, "Blur", 4),
        (MaskEffect::EdgeGlow, "EdgeGlow", 5),
        (MaskEffect::Pixelate, "Pixelate", 6),
        (MaskEffect::LensBlur, "LensBlur", 7),
        (MaskEffect::MotionBlur, "MotionBlur", 8),
        (MaskEffect::RadialBlur, "RadialBlur", 9),
        (MaskEffect::TiltShift, "TiltShift", 10),
        (MaskEffect::Fog, "Fog", 11),
        (MaskEffect::Smoke, "Smoke", 12),
        (MaskEffect::Grain, "Grain", 13),
        (MaskEffect::Halation, "Halation", 14),
        (MaskEffect::Vignette, "Vignette", 15),
        (MaskEffect::Relight, "Relight", 16),
    ];
    assert_eq!(MaskEffect::ALL.len(), effects.len());
    for (effect, serialized, shader_id) in effects {
        assert_eq!(effect.shader_id(), shader_id);
        assert_eq!(serde_json::to_value(effect).unwrap(), json!(serialized));
        assert_eq!(
            serde_json::from_value::<MaskEffect>(json!(serialized)).unwrap(),
            effect,
        );
        assert_eq!(MaskEffect::ALL.iter().filter(|&&e| e == effect).count(), 1);
    }
}

#[test]
fn photographic_defaults_and_serialized_field_names_are_stable() {
    assert_eq!(
        serde_json::to_value(GrainEffectSettings::default()).unwrap(),
        json!({"amount":25.0,"size":1.0,"roughness":50.0,"color":0.0,"seed":0.0}),
    );
    assert_eq!(
        serde_json::to_value(HalationEffectSettings::default()).unwrap(),
        json!({"amount":25.0,"radius":8.0,"threshold":60.0,"warmth":75.0}),
    );
    assert_eq!(
        serde_json::to_value(VignetteEffectSettings::default()).unwrap(),
        json!({"amount":-25.0,"midpoint":50.0,"roundness":0.0,"feather":70.0,
            "highlights":30.0,"center":[50.0,50.0]}),
    );
    assert_eq!(
        serde_json::to_value(MaskEffectSettings::default()).unwrap(),
        json!({})
    );
}

#[test]
fn old_and_partial_effect_settings_receive_photographic_defaults() {
    let legacy: MaskEffectSettings = serde_json::from_value(json!({
        "neon":{"amount":73.0}, "fog":{"seed":211.0},
    }))
    .unwrap();
    assert_eq!(legacy.neon.amount, 73.0);
    assert_eq!(legacy.fog.seed, 211.0);
    assert_eq!(legacy.grain, GrainEffectSettings::default());
    assert_eq!(legacy.halation, HalationEffectSettings::default());
    assert_eq!(legacy.vignette, VignetteEffectSettings::default());
    let serialized = serde_json::to_value(legacy).unwrap();
    for field in ["grain", "halation", "vignette"] {
        assert!(serialized.get(field).is_none());
    }

    let partial: MaskEffectSettings = serde_json::from_value(json!({
        "grain":{"seed":321.0}, "halation":{"warmth":90.0},
        "vignette":{"center":[40.0,65.0]},
    }))
    .unwrap();
    assert_eq!(
        partial.grain,
        GrainEffectSettings {
            seed: 321.0,
            ..Default::default()
        }
    );
    assert_eq!(
        partial.halation,
        HalationEffectSettings {
            warmth: 90.0,
            ..Default::default()
        }
    );
    assert_eq!(
        partial.vignette,
        VignetteEffectSettings {
            center: [40.0, 65.0],
            ..Default::default()
        }
    );
}

#[test]
fn photographic_effects_are_active_only_when_enabled_and_non_neutral() {
    let mut zero_radius = EffectComponent::new(MaskEffect::Halation);
    zero_radius.settings.halation.radius = 0.0;
    assert!(!zero_radius.is_active());
    type AmountCase = (MaskEffect, fn(&mut MaskEffectSettings) -> &mut f32);
    let cases: [AmountCase; 3] = [
        (MaskEffect::Grain, |s| &mut s.grain.amount),
        (MaskEffect::Halation, |s| &mut s.halation.amount),
        (MaskEffect::Vignette, |s| &mut s.vignette.amount),
    ];
    for (effect, field) in cases {
        let mut component = EffectComponent::new(effect);
        assert!(component.is_active());
        assert_eq!(effect.category(), Some(MaskEffectCategory::FilmAndFinish));
        component.enabled = false;
        assert!(!component.is_active());
        component.enabled = true;
        for (amount, active) in [
            (0.0, false),
            (0.000_000_1, false),
            (25.0, true),
            (-25.0, effect == MaskEffect::Vignette),
        ] {
            *field(&mut component.settings) = amount;
            assert_eq!(component.is_active(), active, "{effect:?}: {amount}");
        }
    }
}

#[test]
fn cropping_preserves_global_and_masked_photographic_coordinates_and_seed() {
    let mut stack = MaskStack::default();
    stack.add_mask(MaskKind::Fullscreen).unwrap();
    for effect in [
        MaskEffect::Grain,
        MaskEffect::Halation,
        MaskEffect::Vignette,
    ] {
        let mut component = EffectComponent::new(effect);
        component.settings.grain.seed = 613.0;
        component.settings.vignette.center = [35.0, 72.0];
        stack.global_effects.push(component.clone());
        stack.masks[0].effect_components.push(component);
    }
    let cropped = stack.cropped_for_region(900, 700, 1200, 800, 6000, 4000);
    assert_eq!(cropped.global_effects, stack.global_effects);
    assert_eq!(
        cropped.masks[0].effect_components,
        stack.masks[0].effect_components
    );
    assert_eq!(
        cropped.rasterize_layer(0, 9, 7, 1200, 800),
        vec![255; 9 * 7]
    );
}

#[test]
fn relight_settings_saved_before_the_shadow_switch_cast_shadows() {
    let settings: MaskEffectSettings =
        serde_json::from_value(json!({"relight":{"amount":40.0,"shadows":25.0}})).unwrap();
    assert!(settings.relight.shadows_enabled);
    assert_eq!(settings.relight.shadows, 25.0);
}

#[test]
fn fog_and_smoke_saved_before_light_glow_ignore_scene_lights() {
    use crate::pipeline::effect_params::{fog, smoke};
    let settings: MaskEffectSettings =
        serde_json::from_value(json!({"fog":{"amount":40.0},"smoke":{"amount":30.0}})).unwrap();
    assert_eq!(settings.fog.light_glow, 0.0);
    assert_eq!(settings.smoke.light_glow, 0.0);
    assert!(!settings.fog.image_lights);
    let fresh = MaskEffectSettings::default();
    assert_eq!(fresh.fog.light_glow, fog::LIGHT_GLOW.default);
    assert_eq!(fresh.smoke.light_glow, smoke::LIGHT_GLOW.default);
}
