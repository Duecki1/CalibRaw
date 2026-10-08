//! Scene lights: effects that scatter light (Fog, Smoke) receive the lights
//! other effects place (Relight, Light Rays), whatever their order.

use super::fog_tests::{assert_close, mean_difference, FogScene, MASK_EDGE, RGB_TOLERANCE};
use super::{GpuParams, PipelineOptions, ProcessingQuality, RawGpuPipeline, RemoveSceneContext};
use crate::pipeline::{
    extract_padded_tile, EffectComponent, FogEffectSettings, LoadedRaw, LocalMask, MaskEffect,
    MaskImage, MaskKind, MaskStack, RelightEffectSettings, RemoveEditState, SmokeEffectSettings,
    TilePlan, TileSpec,
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

/// A dark street with one small red lamp centred at (`x`, `y`).
fn dark_with_red_lamp(width: u32, height: u32, x: u32, y: u32) -> anyhow::Result<LoadedRaw> {
    let pixels = (0..width * height)
        .flat_map(|i| {
            let (px, py) = (i % width, i / width);
            if px.abs_diff(x) < 3 && py.abs_diff(y) < 3 {
                [40.0, 3.0, 2.0]
            } else {
                [0.02; 3]
            }
        })
        .collect();
    LoadedRaw::from_scene_linear_rec2020(width, height, pixels)
}

fn with_image_lights(mut component: EffectComponent, image_lights: bool) -> EffectComponent {
    component.settings.fog.image_lights = image_lights;
    component.settings.smoke.image_lights = image_lights;
    component
}

fn fog_with_image_lights(image_lights: bool) -> MaskStack {
    global(vec![with_image_lights(fog(100.0), image_lights)])
}

#[test]
fn media_glow_in_the_colour_of_lights_in_the_photo() -> anyhow::Result<()> {
    let lamp = [WIDTH / 4, HEIGHT / 2];
    let Some(scene) = FogScene::with_source(
        dark_with_red_lamp(WIDTH, HEIGHT, lamp[0], lamp[1])?,
        ProcessingQuality::High,
    )?
    else {
        return Ok(());
    };
    for (name, medium) in [("Fog", fog as fn(f32) -> EffectComponent), ("Smoke", smoke)] {
        let render = |image_lights| {
            scene.render(&global(vec![with_image_lights(
                medium(100.0),
                image_lights,
            )]))
        };
        let unlit = render(false)?;
        let lit = render(true)?;
        // Mean gain of one channel in a square around `x`, leaving out the lamp.
        let gain = |channel: usize, x: u32| {
            let mut sum = 0.0;
            let mut count = 0.0;
            for py in lamp[1] - 6..lamp[1] + 6 {
                for px in x - 6..x + 6 {
                    if px.abs_diff(lamp[0]) < 4 && py.abs_diff(lamp[1]) < 4 {
                        continue;
                    }
                    let i = ((py * WIDTH + px) * 3) as usize + channel;
                    sum += lit[i] - unlit[i];
                    count += 1.0;
                }
            }
            sum / count
        };
        let (red, green) = (gain(0, lamp[0]), gain(1, lamp[0]));
        assert!(red > 0.002, "{name} does not glow beside the lamp: {red}");
        assert!(
            red > 2.0 * green,
            "{name} glow is not red: {red} vs {green}"
        );
        assert!(
            red > 2.0 * gain(0, WIDTH - 8),
            "{name} glow does not fall off away from the lamp"
        );
    }
    Ok(())
}

#[test]
fn image_lights_switched_on_by_an_output_edit_match_a_full_render() -> anyhow::Result<()> {
    let Some(scene) = FogScene::with_source(
        dark_with_red_lamp(WIDTH, HEIGHT, WIDTH / 4, HEIGHT / 2)?,
        ProcessingQuality::High,
    )?
    else {
        return Ok(());
    };
    let Some(fresh) = FogScene::with_source(scene.source.clone(), ProcessingQuality::High)? else {
        return Ok(());
    };
    // The tone stage skips the map while no effect reads it; an edit that
    // only reruns the output stage must still build it.
    scene.render(&fog_with_image_lights(false))?;
    let lit = fog_with_image_lights(true);
    let params = GpuParams::new(&scene.exposure, &lit, &scene.source);
    scene.pipeline.dispatch_stage(
        &scene.queue,
        &scene.device,
        &params,
        crate::pipeline::ProcessingStage::Output,
    );
    let output_only = scene.pipeline.read_display_linear_region_blocking(
        &scene.device,
        &scene.queue,
        0,
        0,
        WIDTH,
        HEIGHT,
    )?;
    let full = fresh.render(&lit)?;
    assert!(
        mean_difference(&full, &fresh.render(&fog_with_image_lights(false))?) > 1e-4,
        "the fixture's lamp does not light the fog"
    );
    assert_close(
        &output_only,
        &full,
        RGB_TOLERANCE,
        "output-only Image lights",
    );
    Ok(())
}

#[test]
fn fog_image_lights_from_export_tiles_match_the_full_frame() -> anyhow::Result<()> {
    const SIZE: [u32; 2] = [320, 240];
    let source = dark_with_red_lamp(SIZE[0], SIZE[1], 150, 110)?;
    let Some(scene) = FogScene::with_source(source.clone(), ProcessingQuality::High)? else {
        return Ok(());
    };
    let masks = fog_with_image_lights(true);
    let full = scene.render(&masks)?;
    assert!(
        mean_difference(&full, &scene.render(&fog_with_image_lights(false))?) > 1e-4,
        "the fixture's lamp does not light the fog"
    );

    // The export prepass: every tile adds its core to the shared light map.
    let plan = TilePlan::new(
        SIZE[0],
        SIZE[1],
        TileSpec {
            core_edge: 96,
            halo: 64,
        },
    );
    let first = extract_padded_tile(&source, plan.tiles[0]);
    let tile_params = |raw: &LoadedRaw, tile: crate::pipeline::ExportTile| {
        GpuParams::new_for_tile(
            &scene.exposure,
            &masks,
            raw,
            tile.global_origin_x,
            tile.global_origin_y,
            SIZE[0],
            SIZE[1],
        )
    };
    let pipeline = RawGpuPipeline::new(
        &scene.device,
        &scene.queue,
        &first,
        &tile_params(&first, plan.tiles[0]),
        PipelineOptions::new(ProcessingQuality::High)
            .mask_atlas_edge(MASK_EDGE)
            .programs(&scene.pipeline.program_template()),
    )?;
    let remove = RemoveEditState::default();
    let context = |tile: crate::pipeline::ExportTile| {
        RemoveSceneContext::new(
            &remove,
            &source,
            &scene.exposure,
            [tile.global_origin_x as f32, tile.global_origin_y as f32],
            [tile.padded_width as f32, tile.padded_height as f32],
        )
    };
    pipeline.begin_export_tone_analysis(&scene.queue, &scene.device);
    for &tile in &plan.tiles {
        let raw = extract_padded_tile(&source, tile);
        pipeline.upload_raw_tile(&scene.queue, &raw)?;
        let params = tile_params(&raw, tile).with_global_tone_histogram_bounds(
            tile.core_x,
            tile.core_y,
            tile.core_width,
            tile.core_height,
        );
        pipeline.accumulate_export_tone_tile_with_remove(
            &scene.queue,
            &scene.device,
            &params,
            context(tile),
        )?;
    }
    pipeline.finish_export_tone_analysis(&scene.queue, &scene.device);

    // Every tile renders as in the full frame.
    for &tile in &plan.tiles {
        let raw = extract_padded_tile(&source, tile);
        pipeline.upload_raw_tile(&scene.queue, &raw)?;
        pipeline.dispatch_export_tile_with_remove(
            &scene.queue,
            &scene.device,
            &tile_params(&raw, tile),
            context(tile),
        )?;
        let actual = pipeline.read_display_linear_region_blocking(
            &scene.device,
            &scene.queue,
            tile.local_core_x,
            tile.local_core_y,
            tile.core_width,
            tile.core_height,
        )?;
        let expected: Vec<_> = (tile.core_y..tile.core_y + tile.core_height)
            .flat_map(|row| {
                let start = ((row * SIZE[0] + tile.core_x) * 3) as usize;
                full[start..start + (tile.core_width * 3) as usize]
                    .iter()
                    .copied()
            })
            .collect();
        assert_close(
            &actual,
            &expected,
            1e-3,
            &format!("tile at {},{}", tile.core_x, tile.core_y),
        );
    }
    Ok(())
}
