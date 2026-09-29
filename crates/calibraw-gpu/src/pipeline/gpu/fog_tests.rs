use super::{GpuParams, ProcessingQuality, RawGpuPipeline, tests::request_test_device};
use crate::pipeline::{
    EffectComponent, ExportTile, ExposureParams, FogEffectSettings, LoadedRaw, LocalMask,
    MaskEffect, MaskImage, MaskKind, MaskStack, ProcessingStage, extract_padded_tile,
};

const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;
const MASK_EDGE: u32 = 64;
const RGB_TOLERANCE: f32 = 2.0e-4;

fn is_dark(x: u32, y: u32) -> bool {
    (x / 8 + y / 8).is_multiple_of(2)
}

fn checkerboard(width: u32, height: u32) -> anyhow::Result<LoadedRaw> {
    let pixels = (0..width * height)
        .flat_map(|i| {
            [if is_dark(i % width, i / width) {
                0.01
            } else {
                0.65
            }; 3]
        })
        .collect();
    LoadedRaw::from_scene_linear_rec2020(width, height, pixels)
}

fn neutral_exposure() -> ExposureParams {
    ExposureParams {
        sharpen_amount: 0.0,
        texture: 0.0,
        clarity: 0.0,
        dehaze: 0.0,
        ..Default::default()
    }
}

fn uniform_fog() -> FogEffectSettings {
    FogEffectSettings {
        amount: 75.0,
        density: 65.0,
        variation: 0.0,
        ..Default::default()
    }
}

fn constant_depth(value: u8) -> MaskImage {
    MaskImage::new(8, 8, vec![value; 64]).unwrap()
}

fn depth_ramp(width: u32, height: u32) -> MaskImage {
    // Both axes matter, so a tile-local UV or a flipped map cannot pass by accident.
    let pixels = (0..width * height)
        .map(|i| {
            let x = (i % width) as f32 / (width - 1) as f32;
            let y = (i / width) as f32 / (height - 1) as f32;
            (255.0 * (0.15 + 0.55 * x + 0.30 * y)).round() as u8
        })
        .collect();
    MaskImage::new(width, height, pixels).unwrap()
}

fn fog_component(settings: FogEffectSettings) -> EffectComponent {
    let mut component = EffectComponent::new(MaskEffect::Fog);
    component.settings.fog = settings;
    component
}

fn global_fog(settings: FogEffectSettings, scene_depth: Option<MaskImage>) -> MaskStack {
    MaskStack {
        global_effects: vec![fog_component(settings)],
        scene_depth,
        ..Default::default()
    }
}

struct FogScene {
    device: wgpu::Device,
    queue: wgpu::Queue,
    source: LoadedRaw,
    exposure: ExposureParams,
    pipeline: RawGpuPipeline,
}

impl FogScene {
    fn new(width: u32, height: u32, quality: ProcessingQuality) -> anyhow::Result<Option<Self>> {
        let Some((device, queue)) = request_test_device() else {
            eprintln!("fog GPU regression skipped: no headless wgpu adapter");
            return Ok(None);
        };
        let source = checkerboard(width, height)?;
        let exposure = neutral_exposure();
        // Reserve a real atlas layer for tests that switch from global to local fog.
        let initial = MaskStack {
            masks: vec![LocalMask::new(MaskKind::Fullscreen, 1)],
            ..Default::default()
        };
        let pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
            &device,
            &queue,
            &source,
            &GpuParams::new(&exposure, &initial, &source),
            quality,
            MASK_EDGE,
        )?;
        Ok(Some(Self {
            device,
            queue,
            source,
            exposure,
            pipeline,
        }))
    }

    fn render(&self, masks: &MaskStack) -> anyhow::Result<Vec<f32>> {
        // Exercise the public upload path, including Arc-backed depth cache invalidation.
        self.pipeline.recompute(
            &self.queue,
            &self.device,
            &GpuParams::new(&self.exposure, masks, &self.source),
        );
        let rgb = self.pipeline.read_display_linear_region_blocking(
            &self.device,
            &self.queue,
            0,
            0,
            self.source.width,
            self.source.height,
        )?;
        assert_eq!(
            rgb.len(),
            (self.source.width * self.source.height * 3) as usize
        );
        assert!(
            rgb.iter().all(|v| v.is_finite()),
            "fog produced non-finite RGB"
        );
        Ok(rgb)
    }
}

fn assert_close(actual: &[f32], expected: &[f32], tolerance: f32, context: &str) {
    assert_eq!(actual.len(), expected.len(), "{context}");
    assert!(!actual.is_empty(), "{context}: empty render");
    let mut max_error = 0.0_f32;
    let mut worst = 0;
    for (i, (&a, &b)) in actual.iter().zip(expected).enumerate() {
        assert!(
            a.is_finite() && b.is_finite(),
            "{context}: non-finite RGB at {i}"
        );
        if (a - b).abs() > max_error {
            max_error = (a - b).abs();
            worst = i;
        }
    }
    assert!(
        max_error <= tolerance,
        "{context}: max error {max_error:e} at channel {worst}: {} vs {}",
        actual[worst],
        expected[worst],
    );
}

fn mean_difference(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(a, b)| (a - b).abs()).sum::<f32>() / a.len() as f32
}

fn checker_levels(rgb: &[f32], width: u32) -> (f32, f32) {
    let mut sums = [0.0; 2];
    let mut counts = [0; 2];
    for (i, pixel) in rgb.chunks_exact(3).enumerate() {
        let x = i as u32 % width;
        let y = i as u32 / width;
        // Measure cell interiors, independent of filtering at checker edges.
        if !(2..6).contains(&(x % 8)) || !(2..6).contains(&(y % 8)) {
            continue;
        }
        let group = usize::from(!is_dark(x, y));
        sums[group] += pixel.iter().sum::<f32>() / 3.0;
        counts[group] += 1;
    }
    assert!(counts.iter().all(|&count| count > 0));
    (sums[0] / counts[0] as f32, sums[1] / counts[1] as f32)
}

fn contrast(rgb: &[f32], width: u32) -> f32 {
    let (dark, light) = checker_levels(rgb, width);
    light - dark
}

#[test]
fn fog_gpu_near_surface_is_clear_and_far_surface_loses_contrast() -> anyhow::Result<()> {
    let Some(scene) = FogScene::new(WIDTH, HEIGHT, ProcessingQuality::High)? else {
        return Ok(());
    };
    let baseline = scene.render(&MaskStack::default())?;
    let fog = uniform_fog(); // Keep the default start=8 and depth influence=100.
    let near = scene.render(&global_fog(fog, Some(constant_depth(0))))?;
    assert_close(
        &near,
        &baseline,
        RGB_TOLERANCE,
        "depth zero must stop fog at the surface",
    );

    let far = scene.render(&global_fog(fog, Some(constant_depth(255))))?;
    let (clear_black, clear_white) = checker_levels(&baseline, WIDTH);
    let (fog_black, fog_white) = checker_levels(&far, WIDTH);
    let clear_contrast = clear_white - clear_black;
    assert!(
        clear_contrast > 0.1,
        "fixture has no usable checker contrast"
    );
    assert!(
        fog_white - fog_black < clear_contrast * 0.8,
        "far fog failed to extinguish contrast: clear={clear_contrast}, fog={}",
        fog_white - fog_black,
    );
    assert!(
        fog_black > clear_black + clear_contrast * 0.02,
        "far fog failed to lift blacks: clear={clear_black}, fog={fog_black}",
    );
    Ok(())
}

#[test]
fn fog_gpu_density_increases_extinction_and_start_truncates_the_volume() -> anyhow::Result<()> {
    let Some(scene) = FogScene::new(WIDTH, HEIGHT, ProcessingQuality::High)? else {
        return Ok(());
    };
    let baseline = scene.render(&MaskStack::default())?;
    let far_depth = constant_depth(255);
    let mut previous_contrast = contrast(&baseline, WIDTH);
    let mut previous_black = checker_levels(&baseline, WIDTH).0;
    for density in [20.0, 50.0, 80.0] {
        let fog = FogEffectSettings {
            density,
            ..uniform_fog()
        };
        let rgb = scene.render(&global_fog(fog, Some(far_depth.clone())))?;
        let (black, white) = checker_levels(&rgb, WIDTH);
        assert!(
            white - black < previous_contrast * 0.98,
            "density={density}: contrast {} did not decrease from {previous_contrast}",
            white - black,
        );
        assert!(
            black > previous_black,
            "density={density}: black level did not rise"
        );
        previous_contrast = white - black;
        previous_black = black;
    }

    let depth = constant_depth(128);
    let in_front = scene.render(&global_fog(
        FogEffectSettings {
            start: 0.0,
            ..uniform_fog()
        },
        Some(depth.clone()),
    ))?;
    let behind = scene.render(&global_fog(
        FogEffectSettings {
            start: 75.0,
            ..uniform_fog()
        },
        Some(depth),
    ))?;
    assert!(contrast(&in_front, WIDTH) < contrast(&baseline, WIDTH) * 0.9);
    assert_close(
        &behind,
        &baseline,
        RGB_TOLERANCE,
        "fog starting behind the surface must be invisible",
    );
    Ok(())
}

#[test]
fn fog_gpu_zero_amount_or_density_bypasses_previously_active_fog() -> anyhow::Result<()> {
    let Some(scene) = FogScene::new(WIDTH, HEIGHT, ProcessingQuality::High)? else {
        return Ok(());
    };
    let baseline = scene.render(&MaskStack::default())?;
    let active = global_fog(uniform_fog(), Some(constant_depth(255)));
    for (amount, density) in [(0.0, 65.0), (75.0, 0.0)] {
        let fogged = scene.render(&active)?;
        assert!(
            mean_difference(&fogged, &baseline) > 0.01,
            "fixture fog is inactive"
        );
        let mut disabled = active.clone();
        disabled.global_effects[0].settings.fog.amount = amount;
        disabled.global_effects[0].settings.fog.density = density;
        assert_close(
            &scene.render(&disabled)?,
            &baseline,
            RGB_TOLERANCE,
            &format!("amount={amount}, density={density} must bypass stale fog"),
        );
    }
    scene.render(&active)?;
    assert_close(
        &scene.render(&MaskStack::default())?,
        &baseline,
        RGB_TOLERANCE,
        "removing fog must restore the original image",
    );
    Ok(())
}

#[test]
fn fog_gpu_zero_depth_influence_ignores_present_and_missing_depth() -> anyhow::Result<()> {
    let Some(scene) = FogScene::new(WIDTH, HEIGHT, ProcessingQuality::High)? else {
        return Ok(());
    };
    let baseline = scene.render(&MaskStack::default())?;
    let settings = FogEffectSettings {
        depth_influence: 0.0,
        variation: 70.0,
        seed: 29.0,
        ..uniform_fog()
    };
    let reference = scene.render(&global_fog(settings, None))?;
    assert!(
        mean_difference(&reference, &baseline) > 0.01,
        "ignoring depth must still render fog"
    );
    for depth in [constant_depth(0), constant_depth(255), depth_ramp(19, 11)] {
        let actual = scene.render(&global_fog(settings, Some(depth)))?;
        assert_close(
            &actual,
            &reference,
            RGB_TOLERANCE,
            "zero depth influence must ignore the depth texture",
        );
    }
    Ok(())
}

#[test]
fn fog_gpu_replacing_and_clearing_depth_does_not_reuse_stale_samples() -> anyhow::Result<()> {
    let Some(scene) = FogScene::new(WIDTH, HEIGHT, ProcessingQuality::High)? else {
        return Ok(());
    };
    let baseline = scene.render(&MaskStack::default())?;
    let without_depth = global_fog(uniform_fog(), None);
    let fallback = scene.render(&without_depth)?;
    assert!(
        mean_difference(&fallback, &baseline) > 0.001,
        "missing depth must provide visible fallback fog"
    );
    let near = global_fog(uniform_fog(), Some(constant_depth(0)));
    let far = global_fog(uniform_fog(), Some(constant_depth(255)));
    // Identical dimensions but different backing Arcs must replace the upload.
    let far_rgb = scene.render(&far)?;
    assert!(contrast(&far_rgb, WIDTH) < contrast(&fallback, WIDTH));
    assert_close(
        &scene.render(&near)?,
        &baseline,
        RGB_TOLERANCE,
        "near depth must replace a cached far upload",
    );
    assert_close(
        &scene.render(&far)?,
        &far_rgb,
        RGB_TOLERANCE,
        "restoring the same far Arc must re-upload it",
    );
    assert_close(
        &scene.render(&far.clone())?,
        &far_rgb,
        RGB_TOLERANCE,
        "cloned depth must render identically",
    );
    assert_close(
        &scene.render(&without_depth)?,
        &fallback,
        RGB_TOLERANCE,
        "clearing depth must restore the fresh fallback",
    );

    // Public dimensions can become invalid even though MaskImage::new validates length.
    for invalid_width in [0, 9] {
        scene.render(&far)?;
        let mut invalid = constant_depth(255);
        invalid.width = invalid_width;
        let rgb = scene.render(&global_fog(uniform_fog(), Some(invalid)))?;
        assert_close(
            &rgb,
            &fallback,
            RGB_TOLERANCE,
            "invalid depth must safely disable stale texture sampling",
        );
    }
    assert_close(
        &scene.render(&far)?,
        &far_rgb,
        RGB_TOLERANCE,
        "valid depth must recover after invalid data",
    );
    Ok(())
}

#[test]
fn fog_gpu_full_mask_matches_global_and_empty_or_disabled_mask_bypasses() -> anyhow::Result<()> {
    for quality in [ProcessingQuality::Preview, ProcessingQuality::High] {
        let Some(scene) = FogScene::new(WIDTH, HEIGHT, quality)? else {
            return Ok(());
        };
        let baseline = scene.render(&MaskStack::default())?;
        let settings = FogEffectSettings {
            variation: 65.0,
            seed: 17.0,
            ..uniform_fog()
        };
        let global = global_fog(settings, Some(depth_ramp(23, 15)));
        let global_rgb = scene.render(&global)?;
        assert!(
            mean_difference(&global_rgb, &baseline) > 0.01,
            "fixture fog is inactive"
        );
        let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
        mask.effect_components.push(fog_component(settings));
        let mut local = MaskStack {
            masks: vec![mask],
            scene_depth: global.scene_depth.clone(),
            ..Default::default()
        };
        let full_coverage = vec![half::f16::ONE.to_bits(); (MASK_EDGE * MASK_EDGE) as usize];
        scene
            .pipeline
            .update_mask_layer(&scene.queue, 0, &full_coverage)?;
        assert_close(
            &scene.render(&local)?,
            &global_rgb,
            RGB_TOLERANCE,
            "full mask must match global volumetric fog",
        );
        scene
            .pipeline
            .update_mask_layer(&scene.queue, 0, &vec![0; full_coverage.len()])?;
        assert_close(
            &scene.render(&local)?,
            &baseline,
            RGB_TOLERANCE,
            "empty coverage must preserve the image",
        );

        // Coverage clips the completed volume; it must not become scene distance.
        let split: Vec<_> = (0..MASK_EDGE * MASK_EDGE)
            .map(|i| {
                if i % MASK_EDGE < MASK_EDGE / 2 {
                    0
                } else {
                    half::f16::ONE.to_bits()
                }
            })
            .collect();
        scene.pipeline.update_mask_layer(&scene.queue, 0, &split)?;
        let clipped = scene.render(&local)?;
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                if x.abs_diff(WIDTH / 2) < 4 {
                    continue;
                }
                let i = ((y * WIDTH + x) * 3) as usize;
                let expected = if x < WIDTH / 2 {
                    &baseline
                } else {
                    &global_rgb
                };
                assert_close(
                    &clipped[i..i + 3],
                    &expected[i..i + 3],
                    RGB_TOLERANCE,
                    "mask coverage must only clip fog contribution",
                );
            }
        }
        scene
            .pipeline
            .update_mask_layer(&scene.queue, 0, &full_coverage)?;
        local.masks[0].enabled = false;
        assert_close(
            &scene.render(&local)?,
            &baseline,
            RGB_TOLERANCE,
            "disabled local fog must preserve the image",
        );
    }
    Ok(())
}

#[test]
fn fog_gpu_variation_and_seed_are_deterministic() -> anyhow::Result<()> {
    let Some(scene) = FogScene::new(WIDTH, HEIGHT, ProcessingQuality::High)? else {
        return Ok(());
    };
    let mut masks = global_fog(uniform_fog(), Some(constant_depth(255)));
    let uniform = scene.render(&masks)?;
    masks.global_effects[0].settings.fog.seed = 137.0;
    assert_close(
        &scene.render(&masks)?,
        &uniform,
        RGB_TOLERANCE,
        "seed must not affect uniform haze",
    );
    masks.global_effects[0].settings.fog.variation = 85.0;
    let banks = scene.render(&masks)?;
    assert!(
        mean_difference(&banks, &uniform) > 0.001,
        "variation did not change the volume"
    );
    masks.global_effects[0].settings.fog.seed = 421.0;
    let reseeded = scene.render(&masks)?;
    assert!(
        mean_difference(&banks, &reseeded) > 0.001,
        "seed did not change the volume"
    );
    masks.global_effects[0].settings.fog.seed = 137.0;
    assert_close(
        &scene.render(&masks)?,
        &banks,
        RGB_TOLERANCE,
        "returning to a seed must reproduce the volume",
    );
    Ok(())
}

#[test]
fn fog_gpu_depth_and_noise_match_full_frame_in_overlapping_tiles() -> anyhow::Result<()> {
    let Some(scene) = FogScene::new(320, 240, ProcessingQuality::High)? else {
        return Ok(());
    };
    let settings = FogEffectSettings {
        variation: 80.0,
        seed: 73.0,
        scale: 35.0,
        softness: 55.0,
        ..uniform_fog()
    };
    let masks = global_fog(settings, Some(depth_ramp(37, 29)));
    let baseline = scene.render(&MaskStack::default())?;
    let full = scene.render(&masks)?;
    assert!(
        mean_difference(&full, &baseline) > 0.01,
        "tile fixture fog is inactive"
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
        // Airlight depends on full-image illumination, not each tile's histogram.
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
        assert_close(
            &actual,
            &expected,
            RGB_TOLERANCE,
            &format!("fog tile at {x},{y} must match the full frame"),
        );
    }
    Ok(())
}
