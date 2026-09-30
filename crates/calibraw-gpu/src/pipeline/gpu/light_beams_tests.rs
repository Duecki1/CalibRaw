use super::{fog_tests::FogScene, pack_effect_mask, GpuParams, ProcessingQuality, RawGpuPipeline};
use crate::pipeline::{
    extract_padded_tile, EffectComponent, ExportTile, FogEffectSettings, LightBeamPreset,
    LightBeamsEffectSettings, LocalMask, MaskEffect, MaskImage, MaskKind, MaskStack,
    ProcessingStage,
};

const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;
const MASK_EDGE: u32 = 64;
const RGB_TOLERANCE: f32 = 2.0e-4;
const MIN_LIFT: f32 = 5.0e-4;
const RIGHT: [f32; 4] = [0.60, 0.40, 0.85, 0.60];
const LEFT: [f32; 4] = [0.15, 0.40, 0.40, 0.60];

fn beam() -> LightBeamsEffectSettings {
    LightBeamsEffectSettings {
        amount: 80.0,
        length: 150.0,
        source: [50.0, 50.0],
        direction: 0.0,
        spread: 60.0,
        softness: 35.0,
        source_depth: 35.0,
        scattering: 0.0,
        color: [1.0; 3],
        ..Default::default()
    }
}

fn fog() -> FogEffectSettings {
    FogEffectSettings {
        amount: 65.0,
        density: 45.0,
        variation: 0.0,
        ..Default::default()
    }
}

fn fog_component(settings: FogEffectSettings) -> EffectComponent {
    let mut component = EffectComponent::new(MaskEffect::Fog);
    component.settings.fog = settings;
    component
}

fn beam_component(settings: LightBeamsEffectSettings) -> EffectComponent {
    let mut component = EffectComponent::new(MaskEffect::LightBeams);
    component.settings.light_beams = settings;
    component
}

fn depth(value: u8) -> MaskImage {
    MaskImage::new(8, 8, vec![value; 64]).unwrap()
}

fn stack(components: Vec<EffectComponent>) -> MaskStack {
    MaskStack {
        global_effects: components,
        scene_depth: Some(depth(255)),
        ..Default::default()
    }
}

fn fog_only() -> MaskStack {
    stack(vec![fog_component(fog())])
}

fn fog_and_beam(settings: LightBeamsEffectSettings) -> MaskStack {
    stack(vec![fog_component(fog()), beam_component(settings)])
}

fn assert_close(actual: &[f32], expected: &[f32], context: &str) {
    assert_eq!(actual.len(), expected.len(), "{context}");
    assert!(!actual.is_empty(), "{context}: empty render");
    let mut maximum = 0.0_f32;
    let mut worst = 0;
    for (index, (&a, &b)) in actual.iter().zip(expected).enumerate() {
        assert!(a.is_finite() && b.is_finite(), "{context}: non-finite RGB");
        if (a - b).abs() > maximum {
            maximum = (a - b).abs();
            worst = index;
        }
    }
    assert!(
        maximum <= RGB_TOLERANCE,
        "{context}: max error {maximum:e} at channel {worst}: {} vs {}",
        actual[worst],
        expected[worst],
    );
}

// Compare identical checkerboard pixels against the fog-only render. Averaging
// a region avoids depending on a ray sample or a checker edge at one pixel.
fn region_lift(actual: &[f32], baseline: &[f32], width: u32, rect: [f32; 4]) -> f32 {
    assert_eq!(actual.len(), baseline.len());
    let height = actual.len() as u32 / (width * 3);
    let bounds = [
        (rect[0] * width as f32).ceil() as u32,
        (rect[1] * height as f32).ceil() as u32,
        (rect[2] * width as f32).floor() as u32,
        (rect[3] * height as f32).floor() as u32,
    ];
    let mut sum = 0.0;
    let mut count = 0;
    for y in bounds[1]..bounds[3] {
        for x in bounds[0]..bounds[2] {
            let offset = ((y * width + x) * 3) as usize;
            for channel in offset..offset + 3 {
                assert!(actual[channel].is_finite() && baseline[channel].is_finite());
                sum += actual[channel] - baseline[channel];
                count += 1;
            }
        }
    }
    assert!(count > 0, "empty measurement region {rect:?}");
    sum / count as f32
}

#[test]
fn light_beams_params_preserve_shader_id_fields_and_bounds() {
    let settings = LightBeamsEffectSettings {
        amount: 37.0,
        length: 123.0,
        source: [23.0, 67.0],
        direction: -42.0,
        spread: 51.0,
        softness: 29.0,
        source_depth: 61.0,
        scattering: 73.0,
        color: [0.2, 0.4, 0.6],
        ..beam()
    };
    let component = beam_component(settings);
    let packed = pack_effect_mask(component.effect, &component.settings, true).unwrap();
    assert_eq!(MaskEffect::LightBeams.shader_id(), 13);
    assert_eq!(packed.metadata[0..2], [1, 1]);
    assert_eq!(packed.metadata[3] >> super::MASK_EFFECT_ID_SHIFT, 13);
    assert_eq!(packed.adjust_0, [37.0, 123.0, 23.0, 67.0]);
    assert_eq!(packed.adjust_1, [0.2, 0.4, 0.6, -42.0]);
    assert_eq!(packed.adjust_2, [51.0, 29.0, 61.0, 73.0]);

    for (settings, primary, secondary, tertiary) in [
        (
            LightBeamsEffectSettings {
                amount: 150.0,
                length: 250.0,
                source: [-100.0, 200.0],
                direction: 240.0,
                spread: 200.0,
                softness: 150.0,
                source_depth: 150.0,
                scattering: 150.0,
                color: [-1.0, 2.0, 0.5],
                ..beam()
            },
            [100.0, 200.0, -50.0, 150.0],
            [0.0, 1.0, 0.5, 180.0],
            [170.0, 100.0, 100.0, 100.0],
        ),
        (
            LightBeamsEffectSettings {
                amount: -10.0,
                length: -10.0,
                source: [200.0, -100.0],
                direction: -240.0,
                spread: -10.0,
                softness: -10.0,
                source_depth: -10.0,
                scattering: -10.0,
                color: [2.0, -1.0, 0.5],
                ..beam()
            },
            [0.0, 0.0, 150.0, -50.0],
            [1.0, 0.0, 0.5, -180.0],
            [1.0, 0.0, 0.0, 0.0],
        ),
    ] {
        let component = beam_component(settings);
        let packed = pack_effect_mask(component.effect, &component.settings, true).unwrap();
        assert_eq!(packed.adjust_0, primary);
        assert_eq!(packed.adjust_1, secondary);
        assert_eq!(packed.adjust_2, tertiary);
    }
    for (amount, length, enabled) in [(0.0, 150.0, true), (80.0, 0.0, true), (80.0, 150.0, false)] {
        let component = beam_component(LightBeamsEffectSettings {
            amount,
            length,
            ..beam()
        });
        let packed = pack_effect_mask(component.effect, &component.settings, enabled).unwrap();
        assert_eq!(packed.metadata[0..2], [0, 0]);
        assert_eq!(packed.metadata[3] >> super::MASK_EFFECT_ID_SHIFT, 13);
    }
}

#[test]
fn light_beams_gpu_uniform_fog_lights_the_directed_region_in_both_qualities() -> anyhow::Result<()>
{
    for quality in [ProcessingQuality::Preview, ProcessingQuality::High] {
        let Some(scene) = FogScene::new(WIDTH, HEIGHT, quality)? else {
            return Ok(());
        };
        let baseline = scene.render(&fog_only())?;
        let lit = scene.render(&fog_and_beam(beam()))?;
        let right = region_lift(&lit, &baseline, WIDTH, RIGHT);
        let left = region_lift(&lit, &baseline, WIDTH, LEFT);
        assert!(right > MIN_LIFT, "{quality:?}: cone lift {right}");
        assert!(
            right > left + MIN_LIFT,
            "{quality:?}: right={right}, left={left}"
        );

        let down = scene.render(&fog_and_beam(LightBeamsEffectSettings {
            direction: 90.0,
            ..beam()
        }))?;
        let below = region_lift(&down, &baseline, WIDTH, [0.42, 0.65, 0.58, 0.85]);
        let above = region_lift(&down, &baseline, WIDTH, [0.42, 0.15, 0.58, 0.35]);
        assert!(
            below > above + MIN_LIFT,
            "{quality:?}: below={below}, above={above}"
        );
    }
    Ok(())
}

#[test]
fn light_beams_gpu_reversing_direction_changes_the_illuminated_side() -> anyhow::Result<()> {
    let Some(scene) = FogScene::new(WIDTH, HEIGHT, ProcessingQuality::High)? else {
        return Ok(());
    };
    let baseline = scene.render(&fog_only())?;
    let forward = scene.render(&fog_and_beam(beam()))?;
    let reverse = scene.render(&fog_and_beam(LightBeamsEffectSettings {
        direction: 180.0,
        ..beam()
    }))?;
    let forward_right = region_lift(&forward, &baseline, WIDTH, RIGHT);
    let forward_left = region_lift(&forward, &baseline, WIDTH, LEFT);
    let reverse_right = region_lift(&reverse, &baseline, WIDTH, RIGHT);
    let reverse_left = region_lift(&reverse, &baseline, WIDTH, LEFT);
    assert!(
        forward_right > forward_left + MIN_LIFT,
        "forward: {forward_right} vs {forward_left}"
    );
    assert!(
        reverse_left > reverse_right + MIN_LIFT,
        "reverse: {reverse_left} vs {reverse_right}"
    );
    assert!(reverse_left > forward_left + MIN_LIFT);
    assert!(forward_right > reverse_right + MIN_LIFT);
    Ok(())
}

#[test]
fn light_beams_gpu_fog_and_beam_component_order_is_independent() -> anyhow::Result<()> {
    let Some(scene) = FogScene::new(WIDTH, HEIGHT, ProcessingQuality::High)? else {
        return Ok(());
    };
    for variation in [0.0, 70.0] {
        let fog = fog_component(FogEffectSettings {
            variation,
            seed: 73.0,
            ..fog()
        });
        let light = beam_component(beam());
        let baseline = scene.render(&stack(vec![fog.clone()]))?;
        let fog_first = scene.render(&stack(vec![fog.clone(), light.clone()]))?;
        let beam_first = scene.render(&stack(vec![light, fog]))?;
        assert!(region_lift(&fog_first, &baseline, WIDTH, RIGHT) > MIN_LIFT);
        assert_close(
            &beam_first,
            &fog_first,
            &format!("component order, variation={variation}"),
        );
    }
    Ok(())
}

#[test]
fn light_beams_gpu_streetlamp_scattering_lights_above_the_cone() -> anyhow::Result<()> {
    let Some(scene) = FogScene::new(WIDTH, HEIGHT, ProcessingQuality::High)? else {
        return Ok(());
    };
    let lamp = LightBeamsEffectSettings {
        source: [50.0, 50.0],
        color: [1.0; 3],
        ..LightBeamsEffectSettings::from_preset(LightBeamPreset::Streetlamp)
    };
    assert!(lamp.scattering > 0.0);
    let baseline = scene.render(&fog_only())?;
    let narrow = scene.render(&fog_and_beam(LightBeamsEffectSettings {
        scattering: 0.0,
        ..lamp
    }))?;
    let broad = scene.render(&fog_and_beam(lamp))?;
    // Above a downward source lies outside even Streetlamp's wide cone.
    let region = [0.43, 0.30, 0.57, 0.40];
    let without_pool = region_lift(&narrow, &baseline, WIDTH, region);
    let with_pool = region_lift(&broad, &baseline, WIDTH, region);
    assert!(with_pool > MIN_LIFT, "streetlamp pool lift={with_pool}");
    assert!(
        with_pool > without_pool + MIN_LIFT,
        "pool={with_pool}, cone only={without_pool}"
    );
    Ok(())
}

#[test]
fn light_beams_gpu_zero_amount_zero_length_and_disabled_restore_baselines() -> anyhow::Result<()> {
    let Some(scene) = FogScene::new(WIDTH, HEIGHT, ProcessingQuality::High)? else {
        return Ok(());
    };
    for with_fog in [false, true] {
        let base = if with_fog { fog_only() } else { stack(vec![]) };
        let baseline = scene.render(&base)?;
        let mut active = base.clone();
        active.global_effects.push(beam_component(beam()));
        for (amount, length, enabled) in
            [(0.0, 150.0, true), (80.0, 0.0, true), (80.0, 150.0, false)]
        {
            let lit = scene.render(&active)?;
            assert!(
                region_lift(&lit, &baseline, WIDTH, RIGHT) > MIN_LIFT,
                "inactive fixture, fog={with_fog}"
            );
            let mut disabled = active.clone();
            let light = disabled.global_effects.last_mut().unwrap();
            light.settings.light_beams.amount = amount;
            light.settings.light_beams.length = length;
            light.enabled = enabled;
            assert_close(
                &scene.render(&disabled)?,
                &baseline,
                &format!("fog={with_fog}, amount={amount}, length={length}, enabled={enabled}"),
            );
        }
        scene.render(&active)?;
        assert_close(
            &scene.render(&base)?,
            &baseline,
            "removing a light clears cached state",
        );
    }
    Ok(())
}

#[test]
fn light_beams_gpu_near_depth_truncates_fog_and_fallback_illumination() -> anyhow::Result<()> {
    let Some(scene) = FogScene::new(WIDTH, HEIGHT, ProcessingQuality::High)? else {
        return Ok(());
    };
    for with_fog in [false, true] {
        let mut baseline_stack = if with_fog { fog_only() } else { stack(vec![]) };
        let baseline_far = scene.render(&baseline_stack)?;
        let mut lights = baseline_stack.clone();
        lights.global_effects.push(beam_component(beam()));
        let far = scene.render(&lights)?;
        assert!(region_lift(&far, &baseline_far, WIDTH, RIGHT) > MIN_LIFT);
        // A surface before the source cannot contain the cone's scattering.
        for value in [0, 25] {
            lights.scene_depth = Some(depth(value));
            baseline_stack.scene_depth = Some(depth(value));
            let baseline_near = scene.render(&baseline_stack)?;
            assert_close(
                &scene.render(&lights)?,
                &baseline_near,
                &format!("near depth={value}, fog={with_fog}"),
            );
        }
    }
    Ok(())
}

#[test]
fn light_beams_gpu_missing_depth_uses_visible_restrained_fallback_haze() -> anyhow::Result<()> {
    let Some(scene) = FogScene::new(WIDTH, HEIGHT, ProcessingQuality::High)? else {
        return Ok(());
    };
    let baseline = scene.render(&MaskStack::default())?;
    let mut fallback = stack(vec![beam_component(beam())]);
    fallback.scene_depth = None;
    let haze = scene.render(&fallback)?;
    let without_fog = region_lift(&haze, &baseline, WIDTH, RIGHT);
    let fog_baseline = scene.render(&fog_only())?;
    let fog_lit = scene.render(&fog_and_beam(beam()))?;
    let in_fog = region_lift(&fog_lit, &fog_baseline, WIDTH, RIGHT);
    assert!(without_fog > MIN_LIFT, "fallback lift={without_fog}");
    assert!(without_fog < in_fog, "fallback={without_fog}, fog={in_fog}");
    Ok(())
}

#[test]
fn light_beams_gpu_local_coverage_clips_illumination_against_global_light() -> anyhow::Result<()> {
    let Some(scene) = FogScene::new(WIDTH, HEIGHT, ProcessingQuality::High)? else {
        return Ok(());
    };
    let light = beam_component(LightBeamsEffectSettings {
        source: [25.0, 50.0],
        ..beam()
    });
    let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
    mask.effect_components.push(light.clone());
    let coverage: Vec<_> = (0..MASK_EDGE * MASK_EDGE)
        .map(|i| {
            if i % MASK_EDGE < MASK_EDGE / 2 {
                half::f16::ONE.to_bits()
            } else {
                half::f16::ZERO.to_bits()
            }
        })
        .collect();
    scene
        .pipeline
        .update_mask_layer(&scene.queue, 0, &coverage)?;
    // Test both the Fog integrator and the separate fallback's mask blend.
    for with_fog in [false, true] {
        let base = if with_fog { fog_only() } else { stack(vec![]) };
        let baseline = scene.render(&base)?;
        let mut global = base.clone();
        global.global_effects.push(light.clone());
        let global_rgb = scene.render(&global)?;
        let mut local = base;
        local.masks.push(mask.clone());
        let local_rgb = scene.render(&local)?;
        for (region, inside) in [
            ([0.32, 0.43, 0.44, 0.57], true),
            ([0.58, 0.43, 0.72, 0.57], false),
        ] {
            let global_lift = region_lift(&global_rgb, &baseline, WIDTH, region);
            let local_lift = region_lift(&local_rgb, &baseline, WIDTH, region);
            assert!(
                global_lift > MIN_LIFT,
                "fog={with_fog}, global lift={global_lift}, region={region:?}"
            );
            let expected = if inside { &global_rgb } else { &baseline };
            let error = region_lift(&local_rgb, expected, WIDTH, region);
            assert!(error.abs() <= RGB_TOLERANCE, "fog={with_fog}, inside={inside}, local={local_lift}, global={global_lift}, error={error}");
            // Pin individual channels as well, so cancelling errors cannot pass.
            let x0 = (region[0] * WIDTH as f32).ceil() as u32;
            let x1 = (region[2] * WIDTH as f32).floor() as u32;
            for y in (region[1] * HEIGHT as f32).ceil() as u32
                ..(region[3] * HEIGHT as f32).floor() as u32
            {
                let begin = ((y * WIDTH + x0) * 3) as usize;
                let end = ((y * WIDTH + x1) * 3) as usize;
                assert_close(
                    &local_rgb[begin..end],
                    &expected[begin..end],
                    "local coverage interior",
                );
            }
        }
    }
    Ok(())
}

#[test]
fn light_beams_gpu_multiple_lights_add_illumination() -> anyhow::Result<()> {
    let Some(scene) = FogScene::new(WIDTH, HEIGHT, ProcessingQuality::High)? else {
        return Ok(());
    };
    let baseline = scene.render(&fog_only())?;
    let light = beam_component(LightBeamsEffectSettings {
        amount: 35.0,
        ..beam()
    });
    let single = scene.render(&stack(vec![fog_component(fog()), light.clone()]))?;
    let double = scene.render(&stack(vec![fog_component(fog()), light.clone(), light]))?;
    let one = region_lift(&single, &baseline, WIDTH, RIGHT);
    let two = region_lift(&double, &baseline, WIDTH, RIGHT);
    assert!(one > MIN_LIFT, "single light lift={one}");
    assert!(two > one + MIN_LIFT, "single={one}, double={two}");
    Ok(())
}

#[test]
fn light_beams_gpu_depth_noise_and_geometry_match_full_frame_in_tiles() -> anyhow::Result<()> {
    let Some(scene) = FogScene::new(320, 240, ProcessingQuality::High)? else {
        return Ok(());
    };
    let depth_pixels = (0..37 * 29)
        .map(|i| {
            let x = (i % 37) as f32 / 36.0;
            let y = (i / 37) as f32 / 28.0;
            (255.0 * (0.15 + 0.55 * x + 0.30 * y)).round() as u8
        })
        .collect();
    let mut masks = stack(vec![fog_component(FogEffectSettings {
        variation: 70.0,
        seed: 73.0,
        scale: 35.0,
        ..fog()
    })]);
    masks.scene_depth = Some(MaskImage::new(37, 29, depth_pixels).unwrap());
    let fog_baseline = scene.render(&masks)?;
    masks
        .global_effects
        .push(beam_component(LightBeamsEffectSettings {
            source: [28.0, 38.0],
            source_depth: 30.0,
            direction: 25.0,
            scattering: 40.0,
            ..beam()
        }));
    let full = scene.render(&masks)?;
    assert!(
        region_lift(&full, &fog_baseline, 320, [0.36, 0.42, 0.60, 0.62]) > MIN_LIFT,
        "tile fixture light is inactive"
    );
    for (x, y) in [(104, 88), (111, 93)] {
        const HALO: u32 = 64;
        let tile = ExportTile {
            core_x: x,
            core_y: y,
            core_width: 96,
            core_height: 64,
            local_core_x: HALO,
            local_core_y: HALO,
            padded_width: 96 + 2 * HALO,
            padded_height: 64 + 2 * HALO,
            global_origin_x: x as i32 - HALO as i32,
            global_origin_y: y as i32 - HALO as i32,
        };
        let raw = extract_padded_tile(&scene.source, tile);
        let params = GpuParams::new_for_tile(
            &scene.exposure,
            &masks,
            &raw,
            tile.global_origin_x,
            tile.global_origin_y,
            scene.source.width,
            scene.source.height,
        );
        let pipeline = RawGpuPipeline::new_headless_reusing_programs_with_mask_edge(
            &scene.device,
            &scene.queue,
            &raw,
            &params,
            ProcessingQuality::High,
            &scene.pipeline,
            MASK_EDGE,
        )?;
        pipeline.dispatch_stage(&scene.queue, &scene.device, &params, ProcessingStage::Raw);
        // Fog's ambient radiance and output tone must use full-image statistics.
        pipeline.dispatch_tone_guide_with_inherited_statistics(
            &scene.queue,
            &scene.device,
            &params,
            &scene.pipeline,
        );
        pipeline.dispatch_stage(
            &scene.queue,
            &scene.device,
            &params,
            ProcessingStage::Output,
        );
        let actual = pipeline.read_display_linear_region_blocking(
            &scene.device,
            &scene.queue,
            tile.local_core_x,
            tile.local_core_y,
            tile.core_width,
            tile.core_height,
        )?;
        let expected: Vec<_> = (y..y + tile.core_height)
            .flat_map(|row| {
                let start = ((row * scene.source.width + x) * 3) as usize;
                full[start..start + (tile.core_width * 3) as usize]
                    .iter()
                    .copied()
            })
            .collect();
        assert_close(&actual, &expected, &format!("light/fog tile at {x},{y}"));
    }
    Ok(())
}
