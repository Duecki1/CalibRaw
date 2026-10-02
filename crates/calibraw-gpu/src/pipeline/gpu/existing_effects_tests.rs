use super::{tests::request_test_device, GpuParams, ProcessingQuality, RawGpuPipeline};
use crate::pipeline::{
    EffectComponent, ExposureParams, LoadedRaw, LocalMask, MaskEffect, MaskKind, MaskStack,
};

const MASK_EDGE: u32 = 64;
const EPSILON: f32 = 2.0e-4;

struct Fixture {
    device: wgpu::Device,
    queue: wgpu::Queue,
    source: LoadedRaw,
    exposure: ExposureParams,
    pipeline: RawGpuPipeline,
}

impl Fixture {
    fn new(
        width: u32,
        height: u32,
        pixel: impl Fn(u32, u32) -> [f32; 3],
    ) -> anyhow::Result<Option<Self>> {
        let Some((device, queue)) = request_test_device() else {
            eprintln!("existing effects GPU regression skipped: no headless wgpu adapter");
            return Ok(None);
        };
        let pixels = (0..width * height)
            .flat_map(|i| pixel(i % width, i / width))
            .collect();
        let source = LoadedRaw::from_scene_linear_rec2020(width, height, pixels)?;
        let exposure = ExposureParams {
            sharpen_amount: 0.0,
            ..Default::default()
        };
        let initial = MaskStack {
            masks: vec![LocalMask::new(MaskKind::Fullscreen, 1)],
            ..Default::default()
        };
        let pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
            &device,
            &queue,
            &source,
            &GpuParams::new(&exposure, &initial, &source),
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

    fn render(&self, component: Option<EffectComponent>) -> anyhow::Result<Vec<f32>> {
        self.render_masks(&MaskStack {
            global_effects: component.into_iter().collect(),
            ..Default::default()
        })
    }

    fn render_masks(&self, masks: &MaskStack) -> anyhow::Result<Vec<f32>> {
        self.pipeline.recompute(
            &self.queue,
            &self.device,
            &GpuParams::new(&self.exposure, masks, &self.source),
        );
        let result = self.pipeline.read_display_linear_region_blocking(
            &self.device,
            &self.queue,
            0,
            0,
            self.source.width,
            self.source.height,
        )?;
        assert!(
            result.iter().all(|v| v.is_finite()),
            "non-finite rendered value"
        );
        Ok(result)
    }
}

fn difference(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max)
}

fn blur(effect: MaskEffect, radius: f32) -> EffectComponent {
    let mut component = EffectComponent::new(effect);
    let s = &mut component.settings;
    s.blur.amount = 100.0;
    s.blur.radius = radius;
    s.lens_blur.amount = 100.0;
    s.lens_blur.radius = radius;
    s.motion_blur.amount = 100.0;
    s.motion_blur.distance = radius;
    s.motion_blur.angle = 0.0;
    s.radial_blur.amount = 100.0;
    s.radial_blur.strength = radius;
    s.radial_blur.center = [0.0, 0.0];
    s.tilt_shift.amount = 100.0;
    s.tilt_shift.radius = radius;
    s.tilt_shift.center = [50.0, 50.0];
    s.tilt_shift.focus_width = 20.0;
    s.tilt_shift.feather = 30.0;
    component
}

const BLURS: [MaskEffect; 5] = [
    MaskEffect::Blur,
    MaskEffect::LensBlur,
    MaskEffect::MotionBlur,
    MaskEffect::RadialBlur,
    MaskEffect::TiltShift,
];

#[test]
fn existing_blurs_preserve_flat_colors_and_hdr() -> anyhow::Result<()> {
    let Some(scene) = Fixture::new(192, 64, |x, _| {
        if x < 64 {
            [0.03, 0.15, 0.4]
        } else if x < 128 {
            [2.0, 4.0, 1.0]
        } else {
            [0.0; 3]
        }
    })?
    else {
        return Ok(());
    };
    let baseline = scene.render(None)?;
    for effect in BLURS {
        let actual = scene.render(Some(blur(effect, 12.0)))?;
        for x in [24, 96, 168] {
            let i = (32 * 192 + x) * 3;
            assert!(
                difference(&actual[i..i + 3], &baseline[i..i + 3]) < EPSILON,
                "{effect:?} changes a flat color at {x}"
            );
        }
    }
    Ok(())
}

#[test]
fn existing_blurs_have_exact_zero_and_continuous_fractional_radii() -> anyhow::Result<()> {
    let Some(scene) = Fixture::new(64, 64, |x, _| [if x % 2 == 0 { 0.08 } else { 0.7 }; 3])? else {
        return Ok(());
    };
    let baseline = scene.render(None)?;
    for effect in BLURS {
        assert!(
            difference(&scene.render(Some(blur(effect, 0.0)))?, &baseline) < EPSILON,
            "{effect:?} changes pixels at zero"
        );
        let tiny = difference(&scene.render(Some(blur(effect, 0.002)))?, &baseline);
        let fractional = difference(&scene.render(Some(blur(effect, 0.35)))?, &baseline);
        let full = difference(&scene.render(Some(blur(effect, 4.0)))?, &baseline);
        assert!(full > 0.01, "{effect:?} fixture does not blur");
        assert!(
            tiny < full * 0.025 + EPSILON,
            "{effect:?} jumps away from zero: {tiny} vs {full}"
        );
        assert!(fractional > tiny && fractional < full * 0.8,
            "{effect:?} ignores fractional radius: tiny={tiny}, fractional={fractional}, full={full}");
    }
    Ok(())
}

#[test]
fn existing_tilt_shift_keeps_focus_sharp_and_progressively_defocuses() -> anyhow::Result<()> {
    let Some(scene) = Fixture::new(96, 96, |x, _| {
        [if (x / 2) % 2 == 0 { 0.06 } else { 0.7 }; 3]
    })?
    else {
        return Ok(());
    };
    let baseline = scene.render(None)?;
    let component = blur(MaskEffect::TiltShift, 30.0);
    let actual = scene.render(Some(component))?;
    let row_difference = |y: usize| {
        let a = (y * 96 + 24) * 3;
        let b = (y * 96 + 72) * 3;
        difference(&actual[a..b], &baseline[a..b])
    };
    assert!(row_difference(48) < EPSILON, "focus band lost detail");
    assert!(row_difference(4) > 0.02, "outside focus band stayed sharp");
    assert!(
        row_difference(35) > EPSILON,
        "feather never starts blurring"
    );
    Ok(())
}

#[test]
fn existing_pixelate_averages_checkerboards_and_partial_edge_blocks() -> anyhow::Result<()> {
    // At the minimum reference scale, size 14.5 becomes an even 8-pixel block.
    // The old 3x3 sampler biased this one-pixel checkerboard in every block.
    let Some(scene) = Fixture::new(66, 66, |x, y| [if (x + y) % 2 == 0 { 0.1 } else { 0.8 }; 3])?
    else {
        return Ok(());
    };
    let mut effect = EffectComponent::new(MaskEffect::Pixelate);
    effect.settings.pixelate.amount = 100.0;
    effect.settings.pixelate.block_size = 14.5;
    let actual = scene.render(Some(effect))?;
    let mid = (32 * 66 + 32) * 3;
    for (x, y) in [(8, 8), (24, 40), (64, 64), (65, 65)] {
        let i = (y * 66 + x) * 3;
        assert!(
            difference(&actual[mid..mid + 3], &actual[i..i + 3]) < EPSILON,
            "block average changed at {x},{y}"
        );
    }
    Ok(())
}

#[test]
fn existing_luminous_edges_reject_flat_fields_and_glow_rejects_shadows() -> anyhow::Result<()> {
    let Some(scene) = Fixture::new(192, 64, |x, _| {
        [if (80..112).contains(&x) { 0.8 } else { 0.03 }; 3]
    })?
    else {
        return Ok(());
    };
    let baseline = scene.render(None)?;
    for effect in [MaskEffect::EdgeGlow, MaskEffect::Neon, MaskEffect::Glow] {
        let mut component = EffectComponent::new(effect);
        component.settings.edge_glow.amount = 100.0;
        component.settings.edge_glow.color = [1.0; 3];
        component.settings.neon.amount = 100.0;
        component.settings.neon.color = [1.0; 3];
        component.settings.neon.background = 100.0;
        component.settings.glow.amount = 100.0;
        component.settings.glow.color = [1.0; 3];
        let actual = scene.render(Some(component))?;
        for x in [16, 176] {
            let i = (32 * 192 + x) * 3;
            assert!(
                difference(&actual[i..i + 3], &baseline[i..i + 3]) < EPSILON,
                "{effect:?} illuminates a flat shadow"
            );
        }
        if effect != MaskEffect::Glow {
            let i = (32 * 192 + 96) * 3;
            assert!(
                difference(&actual[i..i + 3], &baseline[i..i + 3]) < EPSILON,
                "{effect:?} emits in a flat bright field"
            );
        }
        assert!(
            difference(&actual, &baseline) > 0.002,
            "{effect:?} does not illuminate the edge"
        );
    }
    Ok(())
}

#[test]
fn existing_effects_honor_empty_masks_and_full_masks_match_global() -> anyhow::Result<()> {
    let Some(scene) = Fixture::new(64, 64, |x, y| {
        [if (x / 8 + y / 8) % 2 == 0 { 0.05 } else { 0.8 }; 3]
    })?
    else {
        return Ok(());
    };
    let baseline = scene.render(None)?;
    for effect in [
        MaskEffect::LensBlur,
        MaskEffect::MotionBlur,
        MaskEffect::RadialBlur,
        MaskEffect::TiltShift,
        MaskEffect::Pixelate,
        MaskEffect::EdgeGlow,
        MaskEffect::Neon,
        MaskEffect::Glow,
        MaskEffect::Smoke,
    ] {
        let component = if BLURS.contains(&effect) {
            blur(effect, 12.0)
        } else {
            EffectComponent::new(effect)
        };
        let global = scene.render(Some(component.clone()))?;
        let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
        mask.effect_components.push(component);
        let masks = MaskStack {
            masks: vec![mask],
            ..Default::default()
        };
        scene.pipeline.update_mask_layer(
            &scene.queue,
            0,
            &vec![0; (MASK_EDGE * MASK_EDGE) as usize],
        )?;
        assert!(
            difference(&scene.render_masks(&masks)?, &baseline) < EPSILON,
            "{effect:?} ignores an empty mask"
        );
        scene.pipeline.update_mask_layer(
            &scene.queue,
            0,
            &vec![half::f16::ONE.to_bits(); (MASK_EDGE * MASK_EDGE) as usize],
        )?;
        assert!(
            difference(&scene.render_masks(&masks)?, &global) < EPSILON,
            "{effect:?} differs between global and full mask"
        );
    }
    Ok(())
}

#[test]
fn glow_radius_is_independent_of_another_masked_glow() -> anyhow::Result<()> {
    let Some(scene) = Fixture::new(192, 128, |x, _| {
        [if (88..104).contains(&x) { 1.0 } else { 0.03 }; 3]
    })?
    else {
        return Ok(());
    };
    let mut tight = EffectComponent::new(MaskEffect::Glow);
    tight.settings.glow.amount = 85.0;
    tight.settings.glow.radius = 12.0;
    tight.settings.glow.core = 0.0;
    let baseline = scene.render(Some(tight.clone()))?;
    let mut wide = tight.clone();
    wide.settings.glow.radius = 100.0;
    let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
    mask.effect_components.push(wide);
    scene.pipeline.update_mask_layer(
        &scene.queue,
        0,
        &vec![0; (MASK_EDGE * MASK_EDGE) as usize],
    )?;
    let masked = scene.render_masks(&MaskStack {
        masks: vec![mask],
        global_effects: vec![tight],
        ..Default::default()
    })?;
    assert!(
        difference(&masked, &baseline) < EPSILON,
        "a second glow with no coverage must not broaden the first glow"
    );
    Ok(())
}

/// Optional visual/performance fixture for reviewing every module together.
/// CALIBRAW_EFFECT_REVIEW_DIR selects where the PNG contact sheet is written.
#[test]
#[ignore = "writes a visual contact sheet; set CALIBRAW_EFFECT_REVIEW_DIR"]
fn render_effect_review() -> anyhow::Result<()> {
    const W: u32 = 640;
    const H: u32 = 400;
    let output = std::env::var("CALIBRAW_EFFECT_REVIEW_DIR")?;
    let Some(scene) = Fixture::new(W, H, |x, y| {
        let u = x as f32 / W as f32;
        let v = y as f32 / H as f32;
        let mut color = if y < 240 {
            [0.06 + v * 0.12, 0.14 + v * 0.22, 0.28 + v * 0.25]
        } else {
            [0.04 + u * 0.12, 0.07 + u * 0.10, 0.03 + u * 0.04]
        };
        let sun = (u - 0.72).powi(2) + (v - 0.25).powi(2);
        if sun < 0.008 {
            color = [2.8, 2.1, 1.2];
        }
        if (x / 28) % 3 == 0 && y > 170 && y < 320 {
            color = [0.025, 0.04, 0.028];
        }
        if (260..360).contains(&x) && (150..350).contains(&y) {
            color = [0.40, 0.11, 0.055];
            if (x / 12 + y / 12) % 2 == 0 {
                color = color.map(|v| v * 0.85);
            }
        }
        color
    })?
    else {
        return Ok(());
    };
    std::fs::create_dir_all(&output)?;
    let mut sheet = image::RgbImage::new(W * 4, H * 4);
    for (i, effect) in MaskEffect::ALL.into_iter().enumerate() {
        let mut component = EffectComponent::new(effect);
        component.settings.grain.amount = 75.0;
        component.settings.grain.size = 2.0;
        component.settings.halation.amount = 75.0;
        component.settings.halation.radius = 24.0;
        let start = std::time::Instant::now();
        let rgb = scene.render((effect != MaskEffect::Adjustment).then_some(component))?;
        eprintln!("visual {effect:?}: {:?}", start.elapsed());
        for (pixel, color) in rgb.chunks_exact(3).enumerate() {
            let encoded = crate::pipeline::display_linear_rec2020_to_model_srgb([
                color[0], color[1], color[2],
            ]);
            sheet.put_pixel(
                i as u32 % 4 * W + pixel as u32 % W,
                i as u32 / 4 * H + pixel as u32 / W,
                image::Rgb(encoded.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)),
            );
        }
    }
    sheet.save(std::path::Path::new(&output).join("effects.png"))?;
    Ok(())
}
