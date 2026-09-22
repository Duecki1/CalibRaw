use super::{tests::request_test_device, GpuParams, ProcessingQuality, RawGpuPipeline};
use crate::pipeline::{ExposureParams, LoadedRaw, LocalMask, MaskKind, MaskStack};

#[test]
fn film_effects_pass_selection_respects_disabled_masks_and_grain_only() -> anyhow::Result<()> {
    let source = LoadedRaw::from_scene_linear_rec2020(1, 1, vec![0.18; 3])?;
    let neutral = ExposureParams {
        sharpen_amount: 0.0,
        ..Default::default()
    };
    let masks = MaskStack::default();
    let grain = ExposureParams {
        grain_amount: 100.0,
        ..neutral
    };
    let params = GpuParams::new(&grain, &masks, &source);
    assert!(!params.needs_intermediate_adjustment_passes());
    assert!(!params.needs_glow_passes());
    let halation = ExposureParams {
        halation_amount: 100.0,
        ..neutral
    };
    let params = GpuParams::new(&halation, &masks, &source);
    assert!(params.needs_intermediate_adjustment_passes());
    assert!(params.needs_glow_passes());
    let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
    mask.adjustments.halation_amount = 100.0;
    let mut masks = MaskStack {
        masks: vec![mask],
        ..Default::default()
    };
    let params = GpuParams::new(&neutral, &masks, &source);
    assert!(params.needs_intermediate_adjustment_passes());
    assert!(params.needs_glow_passes());
    masks.masks[0].enabled = false;
    let params = GpuParams::new(&neutral, &masks, &source);
    assert!(!params.needs_intermediate_adjustment_passes());
    assert!(!params.needs_glow_passes());
    Ok(())
}

#[test]
fn halation_gpu_preserves_flat_fields_and_cores_and_respects_masks() -> anyhow::Result<()> {
    let Some((device, queue)) = request_test_device() else {
        eprintln!("halation GPU regression skipped: no headless wgpu adapter");
        return Ok(());
    };
    const W: u32 = 192;
    const H: u32 = 64;
    const EDGE: u32 = 64;
    let pixels = (0..W * H)
        .flat_map(|i| {
            // Use a normal bright scene-linear highlight, not an extreme HDR value.
            // This guards against halation becoming effectively invisible in real photos.
            let level = if (80..112).contains(&(i % W)) {
                0.8
            } else {
                0.08
            };
            [level; 3]
        })
        .collect();
    let source = LoadedRaw::from_scene_linear_rec2020(W, H, pixels)?;
    let neutral = ExposureParams {
        sharpen_amount: 0.0,
        ..Default::default()
    };
    let no_masks = MaskStack::default();
    let initial_masks = MaskStack {
        masks: vec![LocalMask::new(MaskKind::Fullscreen, 1); 2],
        ..Default::default()
    };
    let initial = GpuParams::new(&neutral, &initial_masks, &source);
    for quality in [ProcessingQuality::High] {
        let pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
            &device, &queue, &source, &initial, quality, EDGE,
        )?;
        let render = |exposure: &ExposureParams, masks: &MaskStack| -> anyhow::Result<Vec<f32>> {
            pipeline.recompute(&queue, &device, &GpuParams::new(exposure, masks, &source));
            pipeline.read_display_linear_region_blocking(&device, &queue, 0, 0, W, H)
        };
        let baseline = render(&neutral, &no_masks)?;
        let halation = ExposureParams {
            halation_amount: 100.0,
            ..neutral
        };
        let global = render(&halation, &no_masks)?;
        assert!(global.iter().all(|value| value.is_finite()));
        let pixel = |x: usize| (32 * W as usize + x) * 3;
        for x in [16, 96, 176] {
            let i = pixel(x);
            assert!(
                (global[i] - baseline[i]).abs() < 0.0002,
                "flat/core changed at {x}"
            );
        }
        let i = pixel(78);
        let delta: Vec<_> = (0..3).map(|c| global[i + c] - baseline[i + c]).collect();
        assert!(
            delta[0] > 0.004 && delta[0] > delta[1] && delta[1] > delta[2],
            "expected warm halo: {delta:?}"
        );

        let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
        mask.adjustments.halation_amount = 100.0;
        let mut masks = MaskStack {
            masks: vec![mask],
            ..Default::default()
        };
        pipeline.update_mask_layer(
            &queue,
            0,
            &vec![half::f16::ONE.to_bits(); (EDGE * EDGE) as usize],
        )?;
        let local = render(&neutral, &masks)?;
        eprintln!("DEBUG mask rgb={:?}", &local[i..i + 3]);
        let (worst, difference) = global
            .iter()
            .zip(&local)
            .enumerate()
            .map(|(i, (a, b))| (i, (a - b).abs()))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        eprintln!("global/local max difference {difference} at {worst}: global={}, local={}, baseline={}, halo global={}, local={}", global[worst], local[worst], baseline[worst], global[i], local[i]);
        assert!(
            global
                .iter()
                .zip(&local)
                .all(|(a, b)| (a - b).abs() < 0.0002),
            "full mask differs from global"
        );
        pipeline.update_mask_layer(&queue, 0, &vec![0; (EDGE * EDGE) as usize])?;
        let empty = render(&neutral, &masks)?;
        assert!(baseline
            .iter()
            .zip(&empty)
            .all(|(a, b)| (a - b).abs() < 0.0002));
        // Half coverage should yield half the added scene-linear energy.
        pipeline.update_mask_layer(
            &queue,
            0,
            &vec![half::f16::from_f32(0.5).to_bits(); (EDGE * EDGE) as usize],
        )?;
        let half = render(&neutral, &masks)?;
        assert!((half[i] - baseline[i] - 0.5 * delta[0]).abs() < 0.001);
        masks.masks[0].enabled = false;
        let disabled = render(&neutral, &masks)?;
        assert_eq!(baseline, disabled);
        let reset = render(&neutral, &no_masks)?;
        assert_eq!(baseline, reset, "zero must bypass stale diffusion buffers");
        // Glow and halation share storage, but neither may overwrite the other.
        let glow = ExposureParams {
            glow_amount: 40.0,
            ..neutral
        };
        let glow_only = render(&glow, &no_masks)?;
        let both = render(
            &ExposureParams {
                halation_amount: 100.0,
                ..glow
            },
            &no_masks,
        )?;
        assert!(both[i] > glow_only[i]);
        let params = GpuParams::new(&halation, &no_masks, &source);
        pipeline.recompute(&queue, &device, &params);
        let high_output = pipeline.read_output_region_blocking(&device, &queue, 0, 0, W, H)?;
        let preview = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
            &device,
            &queue,
            &source,
            &params,
            ProcessingQuality::Preview,
            EDGE,
        )?;
        preview.recompute(&queue, &device, &params);
        let preview_output = preview.read_output_region_blocking(&device, &queue, 0, 0, W, H)?;
        assert!(high_output
            .iter()
            .zip(&preview_output)
            .all(|(a, b)| a.abs_diff(*b) <= 1));
    }
    Ok(())
}

#[test]
fn grain_gpu_is_stable_monochrome_and_matches_overlapping_tiles() -> anyhow::Result<()> {
    let Some((device, queue)) = request_test_device() else {
        eprintln!("grain GPU regression skipped: no headless wgpu adapter");
        return Ok(());
    };
    const W: u32 = 96;
    const H: u32 = 64;
    let source = LoadedRaw::from_scene_linear_rec2020(W, H, vec![0.18; (W * H * 3) as usize])?;
    let masks = MaskStack::default();
    let neutral = ExposureParams {
        sharpen_amount: 0.0,
        ..Default::default()
    };
    let grain = ExposureParams {
        grain_amount: 100.0,
        ..neutral
    };
    let initial = GpuParams::new_for_tile(&neutral, &masks, &source, 0, 0, 2160, 2160);
    for quality in [ProcessingQuality::High] {
        let pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
            &device, &queue, &source, &initial, quality, 64,
        )?;
        let render = |exposure: &ExposureParams, x, y| -> anyhow::Result<Vec<f32>> {
            let params = GpuParams::new_for_tile(exposure, &masks, &source, x, y, 2160, 2160);
            pipeline.recompute(&queue, &device, &params);
            pipeline.read_display_linear_region_blocking(&device, &queue, 0, 0, W, H)
        };
        let baseline = render(&neutral, 0, 0)?;
        let full = render(&grain, 0, 0)?;
        assert_eq!(full, render(&grain, 0, 0)?, "grain must not flicker");
        let mean = full.iter().sum::<f32>() / full.len() as f32;
        let variance = full.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / full.len() as f32;
        assert!(variance > 0.0001, "grain did not add texture: {variance}");
        assert!(
            (mean - baseline[0]).abs() < 0.005,
            "grain changed exposure: {mean}"
        );
        for rgb in full.chunks_exact(3) {
            assert!(rgb.iter().all(|v| v.is_finite() && *v >= 0.0));
            assert!((rgb[0] - rgb[1]).abs() < 0.0002 && (rgb[1] - rgb[2]).abs() < 0.0002);
        }
        let shifted = render(&grain, 16, 8)?;
        for y in 8..H {
            for x in 16..W {
                let a = ((y * W + x) * 3) as usize;
                let b = (((y - 8) * W + x - 16) * 3) as usize;
                assert!(
                    (full[a] - shifted[b]).abs() < 0.0002,
                    "grain moved with tile at {x},{y}"
                );
            }
        }
        assert_eq!(baseline, render(&neutral, 0, 0)?);
    }
    Ok(())
}
