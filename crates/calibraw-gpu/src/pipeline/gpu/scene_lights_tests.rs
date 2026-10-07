//! Scene lights: effects that scatter light (Fog, Smoke) receive the lights
//! other effects place (Relight, Light Rays), whatever their order.

use super::fog_tests::{assert_close, FogScene, MASK_EDGE, RGB_TOLERANCE};
use super::ProcessingQuality;
use crate::pipeline::{
    EffectComponent, FogEffectSettings, LoadedRaw, LocalMask, MaskEffect, MaskImage, MaskKind,
    MaskStack, RelightEffectSettings, SmokeEffectSettings,
};

const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;

fn grey() -> anyhow::Result<LoadedRaw> {
    LoadedRaw::from_scene_linear_rec2020(WIDTH, HEIGHT, vec![0.18; (WIDTH * HEIGHT * 3) as usize])
}

/// A flat scene halfway into the depth range, so fog lies in front of it.
fn mid_depth() -> MaskImage {
    MaskImage::new(8, 8, vec![128; 64]).unwrap()
}

fn relight_at(x: f32, y: f32) -> EffectComponent {
    let mut component = EffectComponent::new(MaskEffect::Relight);
    component.settings.relight = RelightEffectSettings {
        amount: 100.0,
        source: [x, y],
        depth: 20.0,
        // A short reach, so the light falls off within the small image.
        reach: 30.0,
        ..Default::default()
    };
    component
}

fn fog(light_glow: f32) -> EffectComponent {
    let mut component = EffectComponent::new(MaskEffect::Fog);
    component.settings.fog = FogEffectSettings {
        amount: 75.0,
        density: 65.0,
        variation: 0.0,
        light_glow,
        ..Default::default()
    };
    component
}

fn smoke(light_glow: f32) -> EffectComponent {
    let mut component = EffectComponent::new(MaskEffect::Smoke);
    component.settings.smoke = SmokeEffectSettings {
        amount: 100.0,
        density: 100.0,
        // Pale smoke: its colour is its albedo.
        color: [0.9; 3],
        light_glow,
        ..Default::default()
    };
    component
}

fn global(effects: Vec<EffectComponent>) -> MaskStack {
    MaskStack {
        global_effects: effects,
        scene_depth: Some(mid_depth()),
        ..Default::default()
    }
}

/// Mean brightness gain of `lit` over `unlit` in a square of `radius` pixels.
fn gain_around(lit: &[f32], unlit: &[f32], x: u32, y: u32, radius: u32) -> f32 {
    let mut sum = 0.0;
    let mut count = 0.0;
    for py in y.saturating_sub(radius)..(y + radius).min(HEIGHT) {
        for px in x.saturating_sub(radius)..(x + radius).min(WIDTH) {
            let i = ((py * WIDTH + px) * 3) as usize;
            sum += (lit[i] + lit[i + 1] + lit[i + 2]) - (unlit[i] + unlit[i + 1] + unlit[i + 2]);
            count += 3.0;
        }
    }
    sum / count
}

#[test]
fn media_glow_around_a_relight_light_whichever_comes_first() -> anyhow::Result<()> {
    let Some(scene) = FogScene::with_source(grey()?, ProcessingQuality::High)? else {
        return Ok(());
    };
    // How much Light glow brightens the left quarter of the image with the
    // light over it or over the far right. Comparing one region keeps the
    // smoke's own pattern out of the comparison.
    let glow_on_left = |medium: fn(f32) -> EffectComponent, light_x: f32, light_first: bool| {
        let stack = |glow| {
            let light = relight_at(light_x, 50.0);
            global(if light_first {
                vec![light, medium(glow)]
            } else {
                vec![medium(glow), light]
            })
        };
        let unlit = scene.render(&stack(0.0))?;
        let lit = scene.render(&stack(100.0))?;
        anyhow::Ok(gain_around(&lit, &unlit, WIDTH / 4, HEIGHT / 2, 6))
    };
    for (name, medium) in [("Fog", fog as fn(f32) -> EffectComponent), ("Smoke", smoke)] {
        for light_first in [true, false] {
            let near = glow_on_left(medium, 25.0, light_first)?;
            let far = glow_on_left(medium, 95.0, light_first)?;
            assert!(
                near > 0.005,
                "{name} does not glow around the light (light first: {light_first}): {near}"
            );
            assert!(
                near > far * 1.5,
                "{name} glow does not fall off away from the light (light first: {light_first}): near {near}, far {far}"
            );
        }
    }
    Ok(())
}

#[test]
fn media_ignore_lights_that_do_not_reach_them() -> anyhow::Result<()> {
    let Some(scene) = FogScene::with_source(grey()?, ProcessingQuality::High)? else {
        return Ok(());
    };
    let unlit = scene.render(&global(vec![fog(100.0)]))?;

    let mut disabled = relight_at(25.0, 50.0);
    disabled.enabled = false;
    assert_close(
        &scene.render(&global(vec![disabled, fog(100.0)]))?,
        &unlit,
        RGB_TOLERANCE,
        "a disabled Relight still lights the fog",
    );

    // A Relight confined to an empty mask lights nothing, not even fog.
    scene.pipeline.update_mask_layer(
        &scene.queue,
        0,
        &vec![0; (MASK_EDGE * MASK_EDGE) as usize],
    )?;
    let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
    mask.effect_components.push(relight_at(25.0, 50.0));
    let confined = MaskStack {
        masks: vec![mask],
        ..global(vec![fog(100.0)])
    };
    assert_close(
        &scene.render(&confined)?,
        &unlit,
        RGB_TOLERANCE,
        "a Relight outside its mask still lights the fog",
    );
    Ok(())
}

#[test]
fn fog_glows_around_the_light_rays_source() -> anyhow::Result<()> {
    let Some(scene) = FogScene::with_source(grey()?, ProcessingQuality::High)? else {
        return Ok(());
    };
    let mut rays = EffectComponent::new(MaskEffect::LightRays);
    rays.settings.light_rays.amount = 100.0;
    rays.settings.light_rays.source = [25.0, 50.0];
    let unlit = scene.render(&global(vec![rays.clone(), fog(0.0)]))?;
    let lit = scene.render(&global(vec![rays, fog(100.0)]))?;
    let near = gain_around(&lit, &unlit, WIDTH / 4, HEIGHT / 2, 6);
    let far = gain_around(&lit, &unlit, WIDTH - 6, HEIGHT / 2, 6);
    assert!(near > 0.005, "fog does not glow around the source: {near}");
    assert!(near > far * 1.5, "near {near}, far {far}");
    Ok(())
}
