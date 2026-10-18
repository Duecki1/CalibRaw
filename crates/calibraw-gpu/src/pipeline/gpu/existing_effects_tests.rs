use super::{
    tests::request_test_device, GpuParams, PipelineOptions, ProcessingQuality, RawGpuPipeline,
};
use crate::pipeline::{
    extract_padded_tile, required_export_tile_halo, EffectComponent, ExportTile, ExposureParams,
    LoadedRaw, LocalMask, MaskEffect, MaskKind, MaskStack, ProcessingStage,
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
        let pipeline = RawGpuPipeline::new(
            &device,
            &queue,
            &source,
            &GpuParams::new(&exposure, &initial, &source),
            PipelineOptions::new(ProcessingQuality::High).mask_atlas_edge(MASK_EDGE),
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
        // Highlight Glow follows bright pixels; self-illuminating Glow lights
        // dark surfaces by design.
        component.settings.glow.self_illuminating = false;
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
fn masked_blurs_gather_nothing_from_outside_their_mask() -> anyhow::Result<()> {
    // A red subject (x < 88) beside a flat background whose mask starts at
    // x = 93 (atlas column 31 of 64 across 192 pixels): blurring the
    // background must not pull the subject's red into it.
    let Some(scene) = Fixture::new(192, 96, |x, _| {
        if x < 88 {
            [0.9, 0.05, 0.02]
        } else {
            [0.1, 0.2, 0.3]
        }
    })?
    else {
        return Ok(());
    };
    let background = (0..MASK_EDGE * MASK_EDGE)
        .map(|i| {
            if i % MASK_EDGE >= 31 {
                half::f16::ONE.to_bits()
            } else {
                0
            }
        })
        .collect::<Vec<_>>();
    scene
        .pipeline
        .update_mask_layer(&scene.queue, 0, &background)?;
    let baseline = scene.render(None)?;
    for effect in BLURS {
        let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
        mask.effect_components.push(blur(effect, 40.0));
        let actual = scene.render_masks(&MaskStack {
            masks: vec![mask],
            ..Default::default()
        })?;
        for y in [8, 88] {
            for x in [96, 100, 104] {
                let i = (y * 192 + x) * 3;
                assert!(
                    difference(&actual[i..i + 3], &baseline[i..i + 3]) < EPSILON,
                    "{effect:?} pulls the masked-out subject into {x},{y}: {:?} vs {:?}",
                    &actual[i..i + 3],
                    &baseline[i..i + 3],
                );
            }
        }
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
    // Self-illuminating Glow shares one diffusion radius by design.
    tight.settings.glow.self_illuminating = false;
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

#[test]
fn self_illuminating_glow_lights_dark_surfaces_in_its_color() -> anyhow::Result<()> {
    const WIDTH: u32 = 192;
    let Some(scene) = Fixture::new(WIDTH, 128, |_, _| [0.03; 3])? else {
        return Ok(());
    };
    // A vertical stripe in the middle of the mask atlas: x in [84, 108).
    let stripe: Vec<u16> = (0..MASK_EDGE * MASK_EDGE)
        .map(|i| {
            if (28..36).contains(&(i % MASK_EDGE)) {
                half::f16::ONE.to_bits()
            } else {
                0
            }
        })
        .collect();
    scene.pipeline.update_mask_layer(&scene.queue, 0, &stripe)?;
    let render = |self_illuminating| {
        let mut component = EffectComponent::new(MaskEffect::Glow);
        component.settings.glow.amount = 80.0;
        component.settings.glow.radius = 100.0;
        component.settings.glow.color = [1.0, 0.25, 0.1];
        component.settings.glow.self_illuminating = self_illuminating;
        let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
        mask.effect_components.push(component);
        scene.render_masks(&MaskStack {
            masks: vec![mask],
            ..Default::default()
        })
    };
    let baseline = scene.render_masks(&MaskStack {
        masks: vec![LocalMask::new(MaskKind::Fullscreen, 1)],
        ..Default::default()
    })?;
    let rgb = |image: &[f32], x: u32| {
        let i = ((64 * WIDTH + x) * 3) as usize;
        [image[i], image[i + 1], image[i + 2]]
    };

    let highlight = render(false)?;
    assert!(
        difference(&highlight, &baseline) < EPSILON,
        "highlight Glow lit a flat dark surface"
    );

    let lit = render(true)?;
    let gain = |x| {
        let (lit, base) = (rgb(&lit, x), rgb(&baseline, x));
        [lit[0] - base[0], lit[1] - base[1], lit[2] - base[2]]
    };
    let core = gain(96);
    assert!(
        core[0] > 0.1,
        "self-illuminating Glow did not light the mask: {core:?}"
    );
    assert!(
        core[0] > 2.0 * core[2],
        "core is not tinted toward the Glow color: {core:?}"
    );
    let halo = gain(81);
    assert!(
        halo[0] > 0.005 && halo[0] < core[0],
        "no halo spreads beyond the mask: halo {halo:?}, core {core:?}"
    );
    assert!(
        gain(8)[0] < halo[0],
        "the halo does not fall off with distance"
    );
    Ok(())
}

#[test]
fn masked_texture_leaves_flat_areas_unchanged_after_global_presence() -> anyhow::Result<()> {
    const WIDTH: u32 = 192;
    // A flat, hazy left half beside fine low-contrast texture (for Texture to
    // act on) and a hard checkerboard (for tonal range), whose white is the
    // airlight. Global Dehaze thins the flat half's haze in the adjustment
    // base that masked Texture reads its detail against. The flat samples lie
    // farther from the texture than any presence neighbourhood reaches.
    let Some(mut scene) = Fixture::new(WIDTH, 96, |x, y| {
        if x < 96 {
            [0.55, 0.6, 0.7]
        } else if y < 48 {
            [if (x / 2 + y / 2) % 2 == 0 { 0.16 } else { 0.20 }; 3]
        } else {
            [if (x / 8 + y / 8) % 2 == 0 { 0.03 } else { 0.8 }; 3]
        }
    })?
    else {
        return Ok(());
    };
    let flat_luma = |rgb: &[f32]| {
        let samples = (32..64)
            .flat_map(|y| (16..48).map(move |x| (y * WIDTH + x) as usize * 3))
            .map(|i| 0.2627 * rgb[i] + 0.6780 * rgb[i + 1] + 0.0593 * rgb[i + 2]);
        samples.sum::<f32>() / 1024.0
    };

    let untouched = flat_luma(&scene.render(None)?);
    scene.exposure.dehaze = 60.0;
    scene.exposure.clarity = 50.0;
    let global = scene.render(None)?;
    assert!(
        (flat_luma(&global) - untouched).abs() > 1e-3,
        "global Dehaze/Clarity must shift the flat area for this test to apply"
    );

    scene.pipeline.update_mask_layer(
        &scene.queue,
        0,
        &vec![half::f16::ONE.to_bits(); (MASK_EDGE * MASK_EDGE) as usize],
    )?;
    for texture in [80.0, -80.0] {
        let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
        mask.adjustments.texture = texture;
        let textured = scene.render_masks(&MaskStack {
            masks: vec![mask],
            ..Default::default()
        })?;
        let (before, after) = (flat_luma(&global), flat_luma(&textured));
        assert!(
            (after - before).abs() < EPSILON,
            "masked Texture {texture} changed flat brightness: {before} -> {after}"
        );
        assert!(
            difference(&textured, &global) > 1e-3,
            "masked Texture {texture} had no effect on the fine texture"
        );
    }
    Ok(())
}

#[test]
fn clarity_leaves_flat_tones_unchanged() -> anyhow::Result<()> {
    // A flat shadow and a flat midtone beside a hard checkerboard, which gives
    // the image a wide tonal range. Clarity used to darken the deepest shadows
    // by up to 1.25 EV and lift the upper midtones regardless of local detail.
    const WIDTH: u32 = 288;
    let Some(mut scene) = Fixture::new(WIDTH, 96, |x, y| {
        if x < 96 {
            [0.01; 3]
        } else if x < 192 {
            [0.5; 3]
        } else {
            [if (x / 4 + y / 4) % 2 == 0 { 0.02 } else { 0.9 }; 3]
        }
    })?
    else {
        return Ok(());
    };
    let block_luma = |rgb: &[f32], x0: u32| {
        let samples = (32..64)
            .flat_map(|y| (x0..x0 + 32).map(move |x| (y * WIDTH + x) as usize * 3))
            .map(|i| 0.2627 * rgb[i] + 0.6780 * rgb[i + 1] + 0.0593 * rgb[i + 2]);
        samples.sum::<f32>() / 1024.0
    };
    let before = scene.render(None)?;
    scene.exposure.clarity = 100.0;
    let after = scene.render(None)?;
    for x0 in [32, 128] {
        let (flat_before, flat_after) = (block_luma(&before, x0), block_luma(&after, x0));
        assert!(
            (flat_after / flat_before).log2().abs() < 0.01,
            "Clarity moved the flat block at x={x0}: {flat_before} -> {flat_after}"
        );
    }
    assert!(
        (block_luma(&after, 224) - block_luma(&before, 224)).abs() > 1e-3
            || difference(&after, &before) > 1e-3,
        "Clarity had no effect on the checkerboard"
    );
    Ok(())
}

/// Mean log2 luminance of each column over rows 16..32.
fn column_ev(rgb: &[f32], width: u32) -> Vec<f32> {
    (0..width)
        .map(|x| {
            (16..32)
                .map(|y| {
                    let i = (y * width + x) as usize * 3;
                    (0.2627 * rgb[i] + 0.6780 * rgb[i + 1] + 0.0593 * rgb[i + 2])
                        .max(1e-6)
                        .log2()
                })
                .sum::<f32>()
                / 16.0
        })
        .collect()
}

#[test]
fn clarity_boosts_fine_stripes_evenly() -> anyhow::Result<()> {
    // The old Clarity base sampled the image 3 px apart at this size, so
    // stripes near that spacing passed into the base and lost most of the
    // boost (a 4 px period gained a third of what 2 px did). Every period finer
    // than the Clarity radius belongs to its band and gains alike.
    const WIDTH: u32 = 168;
    let mut gains = Vec::new();
    for period in [2.0f32, 3.0, 4.0, 5.0] {
        let Some(mut scene) = Fixture::new(WIDTH, 48, |x, _| {
            let ev = 0.2 * (std::f32::consts::TAU * x as f32 / period).cos();
            [0.18 * ev.exp2(); 3]
        })?
        else {
            return Ok(());
        };
        let swing = |rgb: &[f32]| {
            let columns = column_ev(rgb, WIDTH);
            let interior = &columns[40..128];
            interior.iter().copied().fold(f32::MIN, f32::max)
                - interior.iter().copied().fold(f32::MAX, f32::min)
        };
        let before = swing(&scene.render(None)?);
        scene.exposure.clarity = 100.0;
        let after = swing(&scene.render(None)?);
        gains.push((period, after / before));
    }
    let largest = gains.iter().map(|(_, gain)| *gain).fold(0.0, f32::max);
    assert!(
        largest > 1.5,
        "Clarity hardly boosts fine stripes: {gains:?}"
    );
    for (period, gain) in &gains {
        assert!(
            *gain > 0.5 * largest,
            "Clarity boosts {period} px stripes {gain}x, against {largest}x at most: {gains:?}"
        );
    }
    Ok(())
}

#[test]
fn texture_and_clarity_tiles_match_the_full_frame() -> anyhow::Result<()> {
    const WIDTH: u32 = 160;
    const HEIGHT: u32 = 120;
    let Some(mut scene) = Fixture::new(WIDTH, HEIGHT, |x, y| {
        let u = x as f32 / WIDTH as f32;
        let v = y as f32 / HEIGHT as f32;
        let dots = if (x * 7 + y * 13) % 11 == 0 { 0.4 } else { 0.0 };
        let stripes = if (x / 3 + y / 5) % 2 == 0 { 0.05 } else { 0.0 };
        [
            0.05 + 0.6 * u + dots + stripes,
            0.1 + 0.5 * v,
            0.3 + 0.2 * u * v,
        ]
    })?
    else {
        return Ok(());
    };
    let masks = MaskStack::default();
    // Negative Texture builds the second chain; positive Texture without
    // Clarity stops after the Texture levels.
    for (texture, clarity) in [(-60.0, 80.0), (60.0, 0.0)] {
        scene.exposure.texture = texture;
        scene.exposure.clarity = clarity;
        let full = scene.render_masks(&masks)?;
        let halo = required_export_tile_halo(&scene.exposure, &masks);
        // Every halo reaches past the image, where the cascade must clamp to the
        // image edge as the full frame does.
        for (x, y) in [(41, 29), (10, 7), (100, 70)] {
            let core_width = (WIDTH - x).min(50);
            let core_height = (HEIGHT - y).min(40);
            let tile = ExportTile {
                core_x: x,
                core_y: y,
                core_width,
                core_height,
                local_core_x: halo,
                local_core_y: halo,
                padded_width: core_width + 2 * halo,
                padded_height: core_height + 2 * halo,
                global_origin_x: x as i32 - halo as i32,
                global_origin_y: y as i32 - halo as i32,
            };
            let raw = extract_padded_tile(&scene.source, tile);
            let params = GpuParams::new_for_tile(
                &scene.exposure,
                &masks,
                &raw,
                tile.global_origin_x,
                tile.global_origin_y,
                WIDTH,
                HEIGHT,
            );
            let pipeline = RawGpuPipeline::new(
                &scene.device,
                &scene.queue,
                &raw,
                &params,
                PipelineOptions::new(ProcessingQuality::High)
                    .mask_atlas_edge(MASK_EDGE)
                    .programs(&scene.pipeline.program_template()),
            )?;
            pipeline.dispatch_stage(&scene.queue, &scene.device, &params, ProcessingStage::Raw);
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
                core_width,
                core_height,
            )?;
            for row in 0..core_height {
                for column in 0..core_width {
                    let i = ((row * core_width + column) * 3) as usize;
                    let j = (((y + row) * WIDTH + x + column) * 3) as usize;
                    assert!(
                        difference(&actual[i..i + 3], &full[j..j + 3]) < EPSILON,
                        "Texture {texture}, Clarity {clarity}, tile at {x},{y}: pixel {},{} \
                         {:?} != {:?}",
                        x + column,
                        y + row,
                        &actual[i..i + 3],
                        &full[j..j + 3],
                    );
                }
            }
        }
    }
    Ok(())
}

/// Renders `raw` with `exposure` through a pipeline of its own size.
fn render_raw(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    raw: &LoadedRaw,
    exposure: &ExposureParams,
) -> anyhow::Result<Vec<f32>> {
    let masks = MaskStack::default();
    let params = GpuParams::new(exposure, &masks, raw);
    let pipeline = RawGpuPipeline::new(
        device,
        queue,
        raw,
        &params,
        PipelineOptions::new(ProcessingQuality::High).mask_atlas_edge(MASK_EDGE),
    )?;
    pipeline.recompute(queue, device, &params);
    pipeline.read_display_linear_region_blocking(device, queue, 0, 0, raw.width, raw.height)
}

/// Log2 luminance of `rgb` averaged in linear light over `factor`×`factor` blocks.
fn block_ev(rgb: &[f32], width: u32, height: u32, factor: u32) -> Vec<f32> {
    let (out_width, out_height) = (width / factor, height / factor);
    let mut out = Vec::with_capacity((out_width * out_height) as usize);
    for y in 0..out_height {
        for x in 0..out_width {
            let mut sum = 0.0f32;
            for sy in y * factor..(y + 1) * factor {
                for sx in x * factor..(x + 1) * factor {
                    let i = ((sy * width + sx) * 3) as usize;
                    sum += 0.2627 * rgb[i] + 0.6780 * rgb[i + 1] + 0.0593 * rgb[i + 2];
                }
            }
            out.push((sum / (factor * factor) as f32).max(1e-6).log2());
        }
    }
    out
}

#[test]
fn reduced_previews_match_the_exports_texture_clarity_and_tone() -> anyhow::Result<()> {
    // A 1080 px photo previewed at a quarter of its size. The preview used to
    // apply its own radius limits, so Clarity reached about twice as far across
    // the photo and the Highlights/Shadows neighbourhood four times as far.
    const EDGE: u32 = 1080;
    const FACTOR: u32 = 4;
    let Some((device, queue)) = request_test_device() else {
        eprintln!("preview consistency GPU regression skipped: no headless wgpu adapter");
        return Ok(());
    };
    let pixels = (0..EDGE * EDGE)
        .flat_map(|i| {
            let (x, y) = ((i % EDGE) as f32, (i / EDGE) as f32);
            let wave = |period: f32, phase: f32| {
                (std::f32::consts::TAU * (x + 0.6 * y) / period + phase).sin()
                    * (std::f32::consts::TAU * (y - 0.3 * x) / (1.3 * period)).cos()
            };
            let block = if ((x / 150.0) as u32 + (y / 210.0) as u32).is_multiple_of(3) {
                -2.5
            } else {
                0.0
            };
            let ev =
                0.6 * wave(48.0, 0.0) + 0.9 * wave(160.0, 1.0) + 1.2 * wave(420.0, 2.0) + block;
            [0.18 * ev.exp2(), 0.17 * ev.exp2(), 0.16 * ev.exp2()]
        })
        .collect();
    let full = LoadedRaw::from_scene_linear_rec2020(EDGE, EDGE, pixels)?;
    let preview = crate::pipeline::build_proxy(
        &full,
        crate::pipeline::ProxySpec {
            max_edge: EDGE / FACTOR,
        },
    );
    assert_eq!(preview.width, EDGE / FACTOR);
    let neutral = ExposureParams {
        sharpen_amount: 0.0,
        ..Default::default()
    };
    let small = EDGE / FACTOR;
    let full_base = block_ev(
        &render_raw(&device, &queue, &full, &neutral)?,
        EDGE,
        EDGE,
        FACTOR,
    );
    let preview_base = block_ev(
        &render_raw(&device, &queue, &preview, &neutral)?,
        small,
        small,
        1,
    );
    // Relative errors this test measured before previews took the export's
    // radii: Clarity 0.62, Highlights 0.24, Shadows 0.054, Texture 1.9 or more.
    // A quarter-size preview cannot hold all of the export's fine detail, so
    // some error remains, most for Texture.
    for (name, edit, limit) in [
        (
            "Clarity",
            ExposureParams {
                clarity: 80.0,
                ..neutral
            },
            0.30,
        ),
        (
            "Highlights",
            ExposureParams {
                highlights: -80.0,
                ..neutral
            },
            0.18,
        ),
        (
            "Shadows",
            ExposureParams {
                shadows: 80.0,
                ..neutral
            },
            0.05,
        ),
        (
            "Texture",
            ExposureParams {
                texture: 80.0,
                ..neutral
            },
            1.0,
        ),
    ] {
        let full_edit = block_ev(
            &render_raw(&device, &queue, &full, &edit)?,
            EDGE,
            EDGE,
            FACTOR,
        );
        let preview_edit = block_ev(
            &render_raw(&device, &queue, &preview, &edit)?,
            small,
            small,
            1,
        );
        // Compare what the edit changes, away from the image border.
        let (mut error, mut signal) = (0.0f64, 0.0f64);
        for y in 16..small - 16 {
            for x in 16..small - 16 {
                let i = (y * small + x) as usize;
                let export_change = f64::from(full_edit[i] - full_base[i]);
                let preview_change = f64::from(preview_edit[i] - preview_base[i]);
                error += (preview_change - export_change).powi(2);
                signal += export_change.powi(2);
            }
        }
        let relative = (error / signal.max(1e-12)).sqrt();
        assert!(signal > 1e-3, "{name} changed nothing");
        assert!(
            relative < limit,
            "{name} in the preview differs from the export by {relative:.3} (relative RMS)"
        );
    }
    Ok(())
}
