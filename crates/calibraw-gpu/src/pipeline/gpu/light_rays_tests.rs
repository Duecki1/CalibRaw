use super::{tests::request_test_device, GpuParams, ProcessingQuality, RawGpuPipeline};
use crate::pipeline::{
    extract_padded_tile, EffectComponent, ExportTile, ExposureParams, LightRaysEffectSettings,
    LoadedRaw, LocalMask, MaskEffect, MaskKind, MaskStack, ProcessingStage,
};

const WIDTH: u32 = 160;
const HEIGHT: u32 = 96;
const MASK_EDGE: u32 = 64;
const RGB_TOLERANCE: f32 = 2.0e-4;
const MIN_LIFT: f32 = 5.0e-4;

fn neutral_exposure() -> ExposureParams {
    ExposureParams {
        sharpen_amount: 0.0,
        texture: 0.0,
        clarity: 0.0,
        dehaze: 0.0,
        ..Default::default()
    }
}

fn rays() -> LightRaysEffectSettings {
    LightRaysEffectSettings {
        amount: 100.0,
        length: 70.0,
        source: [20.0, 25.0],
        spread: 8.0,
        fade: 10.0,
        ray_count: 18.0,
        variation: 70.0,
        softness: 55.0,
        color: [1.0; 3],
    }
}

fn rays_component(settings: LightRaysEffectSettings) -> EffectComponent {
    let mut component = EffectComponent::new(MaskEffect::LightRays);
    component.settings.light_rays = settings;
    component
}

fn global_rays(settings: LightRaysEffectSettings) -> MaskStack {
    MaskStack {
        global_effects: vec![rays_component(settings)],
        ..Default::default()
    }
}

fn local_rays(settings: LightRaysEffectSettings) -> MaskStack {
    let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
    mask.effect_components.push(rays_component(settings));
    MaskStack {
        masks: vec![mask],
        ..Default::default()
    }
}

fn two_lights() -> anyhow::Result<LoadedRaw> {
    let pixels = (0..WIDTH * HEIGHT)
        .flat_map(|i| {
            let (x, y) = (i % WIDTH, i / WIDTH);
            let level = if (16..32).contains(&y) && (24..40).contains(&x) {
                2.0
            } else if (16..32).contains(&y) && (112..128).contains(&x) {
                0.8
            } else {
                0.02
            };
            [level; 3]
        })
        .collect();
    LoadedRaw::from_scene_linear_rec2020(WIDTH, HEIGHT, pixels)
}

fn landscape() -> anyhow::Result<LoadedRaw> {
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let v = y as f32 / HEIGHT as f32;
            let mut rgb = [0.06 + 0.025 * v, 0.08 + 0.015 * v, 0.12 - 0.04 * v];
            // A warm opening in the sky is the only luminous region.
            if (28..44).contains(&x) && (12..28).contains(&y) {
                rgb = [2.4, 2.1, 1.8];
            }
            // Distant ridge, foreground ground and narrow tree silhouettes.
            let ridge = 54 + x.abs_diff(80) / 5;
            if y >= ridge {
                rgb = [0.018, 0.027, 0.025];
            }
            if y > 82 {
                rgb = [0.007, 0.011, 0.009];
            }
            for (tree_x, top) in [(64_u32, 32_u32), (102, 42), (145, 26)] {
                if y >= top
                    && ((x.abs_diff(tree_x) <= 1)
                        || (y < top + 36 && x.abs_diff(tree_x) < (y - top) / 4 + 2))
                {
                    rgb = [0.003, 0.005, 0.004];
                }
            }
            pixels.extend_from_slice(&rgb);
        }
    }
    LoadedRaw::from_scene_linear_rec2020(WIDTH, HEIGHT, pixels)
}

struct LightRaysScene {
    device: wgpu::Device,
    queue: wgpu::Queue,
    source: LoadedRaw,
    exposure: ExposureParams,
    pipeline: RawGpuPipeline,
}

impl LightRaysScene {
    fn new(source: LoadedRaw, exposure: ExposureParams) -> anyhow::Result<Option<Self>> {
        let Some((device, queue)) = request_test_device() else {
            eprintln!("Light Rays GPU regression skipped: no headless wgpu adapter");
            return Ok(None);
        };
        // Reserve one real atlas layer, even when initially rendering globally.
        let masks = local_rays(rays());
        let pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
            &device,
            &queue,
            &source,
            &GpuParams::new(&exposure, &masks, &source),
            ProcessingQuality::High,
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
        assert!(rgb.iter().all(|value| value.is_finite()), "non-finite RGB");
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
            "{context}: non-finite channel {i}"
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
    assert!(!a.is_empty());
    a.iter().zip(b).map(|(a, b)| (a - b).abs()).sum::<f32>() / a.len() as f32
}

fn region(rgb: &[f32], width: u32, x: u32, y: u32, w: u32, h: u32) -> Vec<f32> {
    (y..y + h)
        .flat_map(|row| {
            let start = ((row * width + x) * 3) as usize;
            rgb[start..start + (w * 3) as usize].iter().copied()
        })
        .collect()
}

fn mean_lift(actual: &[f32], baseline: &[f32]) -> f32 {
    assert_eq!(actual.len(), baseline.len());
    assert!(!actual.is_empty());
    actual.iter().zip(baseline).map(|(a, b)| a - b).sum::<f32>() / actual.len() as f32
}

#[test]
fn light_rays_gpu_zero_amount_and_disabled_effect_preserve_pixels() -> anyhow::Result<()> {
    let Some(scene) = LightRaysScene::new(two_lights()?, neutral_exposure())? else {
        return Ok(());
    };
    let baseline = scene.render(&MaskStack::default())?;
    let settings = rays();
    let active = scene.render(&global_rays(settings))?;
    assert!(
        mean_difference(&active, &baseline) > MIN_LIFT,
        "manual fixture is inactive"
    );
    assert_close(
        &scene.render(&global_rays(LightRaysEffectSettings {
            amount: 0.0,
            ..settings
        }))?,
        &baseline,
        RGB_TOLERANCE,
        "zero amount must preserve pixels",
    );
    let mut disabled = global_rays(settings);
    disabled.global_effects[0].enabled = false;
    assert_close(
        &scene.render(&disabled)?,
        &baseline,
        RGB_TOLERANCE,
        "disabled rays must preserve pixels",
    );
    Ok(())
}

#[test]
fn light_rays_gpu_manual_source_motion_changes_landscape_shafts() -> anyhow::Result<()> {
    let Some(scene) = LightRaysScene::new(landscape()?, neutral_exposure())? else {
        return Ok(());
    };
    let clear = MaskStack::default();
    let baseline = scene.render(&clear)?;
    let manual = global_rays(LightRaysEffectSettings {
        source: [22.5, 20.833334],
        ..rays()
    });
    let moved = global_rays(LightRaysEffectSettings {
        source: [77.5, 20.833334],
        ..manual.global_effects[0].settings.light_rays
    });
    let left = scene.render(&manual)?;
    let right = scene.render(&moved)?;
    assert!(
        mean_difference(&left, &right) > MIN_LIFT,
        "moving a manual source did not move its shafts"
    );
    for (x, near, far) in [(24, &left, &right), (112, &right, &left)] {
        let clear_patch = region(&baseline, WIDTH, x, 38, 24, 28);
        let near_lift = mean_lift(&region(near, WIDTH, x, 38, 24, 28), &clear_patch);
        let far_lift = mean_lift(&region(far, WIDTH, x, 38, 24, 28), &clear_patch);
        assert!(
            near_lift > far_lift + MIN_LIFT,
            "manual shafts must follow source motion at x={x}: near={near_lift:e}, far={far_lift:e}"
        );
    }
    assert_close(
        &scene.render(&manual)?,
        &left,
        1.0e-6,
        "manual shafts must repeat after moving the source",
    );
    maybe_write_preview(&scene, [&clear, &manual, &moved])?;
    Ok(())
}

// Runs as part of the landscape regression only when explicitly requested.
// Panels are baseline / source left / source right, from left to right.
fn maybe_write_preview(scene: &LightRaysScene, panels: [&MaskStack; 3]) -> anyhow::Result<()> {
    let Some(path) = std::env::var_os("CALIBRAW_LIGHT_RAYS_PREVIEW") else {
        return Ok(());
    };
    let path = std::path::PathBuf::from(path);
    let mut comparison = image::RgbaImage::new(scene.source.width * 3, scene.source.height);
    for (index, masks) in panels.into_iter().enumerate() {
        scene.render(masks)?;
        let bytes = scene.pipeline.read_output_region_blocking(
            &scene.device,
            &scene.queue,
            0,
            0,
            scene.source.width,
            scene.source.height,
        )?;
        let panel = image::RgbaImage::from_raw(scene.source.width, scene.source.height, bytes)
            .ok_or_else(|| anyhow::anyhow!("invalid Light Rays preview RGBA dimensions"))?;
        image::imageops::replace(
            &mut comparison,
            &panel,
            (index as u32 * scene.source.width).into(),
            0,
        );
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    comparison.save_with_format(&path, image::ImageFormat::Png)?;
    eprintln!(
        "Light Rays preview (baseline / source left / source right): {}",
        path.display()
    );
    Ok(())
}

#[test]
fn light_rays_gpu_off_tile_manual_source_matches_full_frame() -> anyhow::Result<()> {
    const FULL_WIDTH: u32 = 320;
    const FULL_HEIGHT: u32 = 224;
    const HALO: u32 = 64;
    let pixels = (0..FULL_WIDTH * FULL_HEIGHT)
        .flat_map(|i| {
            let (x, y) = (i % FULL_WIDTH, i / FULL_WIDTH);
            [if (12..68).contains(&x) && (12..84).contains(&y) {
                2.0
            } else {
                0.025
            }; 3]
        })
        .collect();
    let source = LoadedRaw::from_scene_linear_rec2020(FULL_WIDTH, FULL_HEIGHT, pixels)?;
    // Make full-frame tone statistics relevant, while keeping all spatial detail off.
    let exposure = ExposureParams {
        highlights: -45.0,
        shadows: 35.0,
        ..neutral_exposure()
    };
    let Some(scene) = LightRaysScene::new(source, exposure)? else {
        return Ok(());
    };
    let baseline = scene.render(&MaskStack::default())?;
    let masks = global_rays(LightRaysEffectSettings {
        length: 200.0,
        source: [12.5, 21.428572],
        ..rays()
    });
    let full = scene.render(&masks)?;
    for (x, y) in [(168_u32, 112_u32), (171, 115)] {
        let tile = ExportTile {
            core_x: x,
            core_y: y,
            core_width: 80,
            core_height: 56,
            local_core_x: HALO,
            local_core_y: HALO,
            padded_width: 80 + 2 * HALO,
            padded_height: 56 + 2 * HALO,
            global_origin_x: x as i32 - HALO as i32,
            global_origin_y: y as i32 - HALO as i32,
        };
        assert!(
            tile.global_origin_x > 68,
            "source must be outside the entire padded tile"
        );
        let raw = extract_padded_tile(&scene.source, tile);
        let params = GpuParams::new_for_tile(
            &scene.exposure,
            &masks,
            &raw,
            tile.global_origin_x,
            tile.global_origin_y,
            FULL_WIDTH,
            FULL_HEIGHT,
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
        let read = || {
            pipeline.read_display_linear_region_blocking(
                &scene.device,
                &scene.queue,
                tile.local_core_x,
                tile.local_core_y,
                tile.core_width,
                tile.core_height,
            )
        };
        let output = || {
            pipeline.dispatch_stage(
                &scene.queue,
                &scene.device,
                &params,
                ProcessingStage::Output,
            )
        };
        let expected = region(&full, FULL_WIDTH, x, y, tile.core_width, tile.core_height);
        let clear = region(
            &baseline,
            FULL_WIDTH,
            x,
            y,
            tile.core_width,
            tile.core_height,
        );
        assert!(
            mean_lift(&expected, &clear) > MIN_LIFT,
            "off-tile source must cast visible rays into the crop"
        );

        pipeline.dispatch_stage(&scene.queue, &scene.device, &params, ProcessingStage::Raw);
        pipeline.dispatch_tone_guide_with_inherited_statistics(
            &scene.queue,
            &scene.device,
            &params,
            &scene.pipeline,
        );
        output();
        assert_close(
            &read()?,
            &expected,
            RGB_TOLERANCE,
            &format!("manual rays and inherited tone statistics must match at {x},{y}"),
        );
    }
    Ok(())
}
