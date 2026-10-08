use super::GpuParams;
use crate::pipeline::{EffectComponent, ExposureParams, LoadedRaw, MaskEffect, MaskStack};

fn global(component: EffectComponent) -> MaskStack {
    MaskStack {
        global_effects: vec![component],
        ..Default::default()
    }
}

#[test]
fn film_effects_pass_selection_follows_each_component() -> anyhow::Result<()> {
    let source = LoadedRaw::from_scene_linear_rec2020(1, 1, vec![0.18; 3])?;
    let neutral = ExposureParams {
        sharpen_amount: 0.0,
        ..Default::default()
    };
    let passes = |masks: &MaskStack| {
        let params = GpuParams::new(&neutral, masks, &source);
        (
            params.needs_intermediate_adjustment_passes(),
            params.needs_glow_passes(),
        )
    };

    assert_eq!(passes(&MaskStack::default()), (false, false));
    // Grain and Vignette follow the display transform and need no scene passes.
    for effect in [MaskEffect::Grain, MaskEffect::Vignette] {
        assert_eq!(
            passes(&global(EffectComponent::new(effect))),
            (false, false)
        );
    }
    // Halation and highlight Glow sample the scene with their own radius.
    for effect in [MaskEffect::Halation, MaskEffect::Glow] {
        let mut component = EffectComponent::new(effect);
        assert_eq!(
            passes(&global(component.clone())),
            (true, false),
            "{effect:?}"
        );
        component.enabled = false;
        assert_eq!(passes(&global(component)), (false, false), "{effect:?}");
    }
    // Only self-illuminating Glow emits into the shared Glow diffusion.
    let mut glow = EffectComponent::new(MaskEffect::Glow);
    glow.settings.glow.self_illuminating = true;
    assert_eq!(passes(&global(glow)), (true, true));
    Ok(())
}
