use super::super::{read_rgba32_texture_rgb_blocking, DEHAZE_PASS_COUNT};
use super::*;

fn fixture(width: u32, height: u32, transmission: f32) -> (LoadedRaw, Vec<f32>) {
    let ambient = [0.8, 0.9, 1.0];
    let mut clear = Vec::new();
    let mut hazy = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let color = if y < height / 4 {
                ambient
            } else {
                match (x / 2 + y / 2) % 4 {
                    0 => [0.0, 0.0, 0.0],
                    1 => [0.5, 0.08, 0.04],
                    2 => [0.06, 0.4, 0.1],
                    _ => [0.1, 0.15, 0.5],
                }
            };
            for c in 0..3 {
                clear.push(color[c]);
                hazy.push(color[c] * transmission + ambient[c] * (1.0 - transmission));
            }
        }
    }
    (
        LoadedRaw::from_scene_linear_rec2020(width, height, hazy).unwrap(),
        clear,
    )
}

fn scene_result(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &RawGpuPipeline,
    params: &GpuParams,
) -> anyhow::Result<Vec<f32>> {
    pipeline.upload_params(queue, params);
    let mut encoder = device.create_command_encoder(&Default::default());
    pipeline.encode_pass(&mut encoder, pipeline.adjustment_prepare_pass_index);
    if params.needs_dehaze_passes() {
        pipeline.encode_pass_range(
            &mut encoder,
            pipeline.passes.len() - DEHAZE_PASS_COUNT,
            pipeline.passes.len(),
        );
    }
    queue.submit(Some(encoder.finish()));
    read_rgba32_texture_rgb_blocking(
        device,
        queue,
        &pipeline._tex1,
        pipeline.width,
        pipeline.height,
        "dehaze regression",
    )
}

#[test]
fn dehaze_recovers_known_scattering_and_respects_masks() -> anyhow::Result<()> {
    let Some((device, queue)) = request_test_device() else {
        eprintln!("dehaze GPU regression skipped: no adapter");
        return Ok(());
    };
    let (source, clear) = fixture(128, 96, 0.5);
    let mut exposure = ExposureParams {
        sharpen_amount: 0.0,
        ..Default::default()
    };
    let mut masks = MaskStack::default();
    let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
    mask.adjustments.dehaze = 0.0;
    masks.masks.push(mask);
    let mut params = GpuParams::new(&exposure, &masks, &source);
    let pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
        &device,
        &queue,
        &source,
        &params,
        ProcessingQuality::High,
        64,
    )?;
    pipeline.recompute(&queue, &device, &params);
    let baseline = scene_result(&device, &queue, &pipeline, &params)?;
    let mut errors = Vec::new();
    for amount in [0.0, 25.0, 50.0, 100.0] {
        exposure.dehaze = amount;
        params = GpuParams::new(&exposure, &masks, &source);
        let output = scene_result(&device, &queue, &pipeline, &params)?;
        assert!(output.iter().all(|v| v.is_finite() && *v >= -1e-6));
        let error = output
            .iter()
            .zip(&clear)
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f32>()
            / clear.len() as f32;
        errors.push(error.sqrt());
        // The airlight-colored sky must retain its color and brightness.
        for c in 0..3 {
            assert!((output[c] - baseline[c]).abs() < 0.002);
        }
        if amount == 100.0 {
            exposure.exposure = 1.0;
            let brighter = scene_result(
                &device,
                &queue,
                &pipeline,
                &GpuParams::new(&exposure, &masks, &source),
            )?;
            let delta = output
                .iter()
                .zip(&brighter)
                .map(|(a, b)| (2.0 * a - b).abs())
                .fold(0.0, f32::max);
            assert!(delta < 0.002, "exposure covariance: {delta}");
            exposure.exposure = 0.0;
        }
    }
    eprintln!("dehaze RMSE at 0/25/50/100: {errors:?}");
    assert!(errors.windows(2).all(|v| v[1] < v[0]));
    assert!(
        errors[3] < errors[0] * 0.25,
        "must remove physical haze, not just darken"
    );
    exposure.dehaze = -100.0;
    let added = scene_result(
        &device,
        &queue,
        &pipeline,
        &GpuParams::new(&exposure, &masks, &source),
    )?;
    let ambient = [0.8, 0.9, 1.0];
    for (i, (a, b)) in baseline.iter().zip(&added).enumerate() {
        assert!((b - ambient[i % 3]).abs() <= (a - ambient[i % 3]).abs() + 0.002);
    }
    // A local-only edit must run the passes and leave zero-coverage pixels exact.
    exposure.dehaze = 0.0;
    masks.masks[0].adjustments.dehaze = 100.0;
    let coverage: Vec<u16> = (0..64 * 64)
        .map(|i| {
            if i % 64 < 32 {
                half::f16::ONE.to_bits()
            } else {
                0
            }
        })
        .collect();
    pipeline.update_mask_layer(&queue, 0, &coverage)?;
    params = GpuParams::new(&exposure, &masks, &source);
    assert!(params.needs_dehaze_passes());
    let local = scene_result(&device, &queue, &pipeline, &params)?;
    let y = 70;
    let left = (y * 128 + 20) * 3;
    let right = (y * 128 + 100) * 3;
    assert!((local[left] - baseline[left]).abs() > 0.1);
    assert_eq!(&local[right..right + 3], &baseline[right..right + 3]);
    masks.masks[0].enabled = false;
    assert!(!GpuParams::new(&exposure, &masks, &source).needs_dehaze_passes());
    // Exercise full output ordering and all new GPU resource layouts as well.
    pipeline.recompute(&queue, &device, &params);
    Ok(())
}

#[test]
fn dehaze_export_crop_matches_full_frame() -> anyhow::Result<()> {
    let Some((device, queue)) = request_test_device() else {
        return Ok(());
    };
    let (source, _) = fixture(320, 240, 0.6);
    let exposure = ExposureParams {
        sharpen_amount: 0.0,
        dehaze: 80.0,
        ..Default::default()
    };
    let masks = MaskStack::default();
    let params = GpuParams::new(&exposure, &masks, &source);
    let pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
        &device,
        &queue,
        &source,
        &params,
        ProcessingQuality::High,
        64,
    )?;
    pipeline.recompute(&queue, &device, &params);
    let core = NativeRect {
        x: 98,
        y: 82,
        width: 96,
        height: 80,
    };
    let crop =
        render_tone_consistency_crop(&device, &queue, &source, &exposure, &masks, &pipeline, core)?;
    let full = pipeline.read_display_linear_region_blocking(
        &device,
        &queue,
        core.x,
        core.y,
        core.width,
        core.height,
    )?;
    let delta = full
        .iter()
        .zip(crop)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    assert!(delta < 0.002, "dehaze crop seam: {delta}");
    // Export accumulates disjoint source cores before reducing shared airlight.
    let before = scene_result(&device, &queue, &pipeline, &params)?;
    pipeline.begin_export_tone_analysis(&queue, &device);
    for (x, y) in [(160, 120), (0, 120), (160, 0), (0, 0)] {
        let region_params = GpuParams::new(&exposure, &masks, &source)
            .with_global_tone_histogram_bounds(x, y, 160, 120);
        pipeline.upload_params(&queue, &region_params);
        let mut encoder = device.create_command_encoder(&Default::default());
        pipeline.encode_pass(&mut encoder, pipeline.tone_prepare_pass_index);
        queue.submit(Some(encoder.finish()));
    }
    pipeline.finish_export_tone_analysis(&queue, &device);
    let after = scene_result(&device, &queue, &pipeline, &params)?;
    let error = before
        .iter()
        .zip(after)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    assert!(
        error < 1e-5,
        "airlight must not depend on export tile order: {error}"
    );
    Ok(())
}

#[test]
fn dehaze_preserves_clear_content_and_handles_dense_haze() -> anyhow::Result<()> {
    let Some((device, queue)) = request_test_device() else {
        return Ok(());
    };
    let masks = MaskStack::default();
    let exposure = ExposureParams {
        sharpen_amount: 0.0,
        dehaze: 100.0,
        ..Default::default()
    };
    for transmission in [1.0, 0.2] {
        let (source, clear) = fixture(96, 64, transmission);
        let params = GpuParams::new(&exposure, &masks, &source);
        let pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
            &device,
            &queue,
            &source,
            &params,
            ProcessingQuality::High,
            64,
        )?;
        pipeline.recompute(&queue, &device, &params);
        let output = scene_result(&device, &queue, &pipeline, &params)?;
        let rmse = (output
            .iter()
            .zip(&clear)
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f32>()
            / clear.len() as f32)
            .sqrt();
        assert!(
            rmse < if transmission == 1.0 { 0.002 } else { 0.15 },
            "t={transmission}: {rmse}"
        );
    }
    for (edge, value) in [(1, 0.18), (16, 0.0), (16, 0.18), (16, 1.0), (16, 8.0)] {
        let source = LoadedRaw::from_scene_linear_rec2020(
            edge,
            edge,
            vec![value; (edge * edge * 3) as usize],
        )?;
        let params = GpuParams::new(&exposure, &masks, &source);
        let pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
            &device,
            &queue,
            &source,
            &params,
            ProcessingQuality::High,
            64,
        )?;
        pipeline.recompute(&queue, &device, &params);
        let output = scene_result(&device, &queue, &pipeline, &params)?;
        assert!(
            output
                .iter()
                .all(|v| v.is_finite() && (v - value).abs() < 0.003 * value.max(1.0)),
            "flat field {value}"
        );
    }
    Ok(())
}

#[test]
fn dehaze_preview_matches_high_quality() -> anyhow::Result<()> {
    let Some((device, queue)) = request_test_device() else {
        return Ok(());
    };
    let (source, _) = fixture(128, 96, 0.5);
    let masks = MaskStack::default();
    let exposure = ExposureParams {
        sharpen_amount: 0.0,
        dehaze: 100.0,
        ..Default::default()
    };
    let params = GpuParams::new(&exposure, &masks, &source);
    let mut images = Vec::new();
    for quality in [ProcessingQuality::Preview, ProcessingQuality::High] {
        let pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
            &device, &queue, &source, &params, quality, 64,
        )?;
        pipeline.recompute(&queue, &device, &params);
        images.push(pipeline.read_output_region_blocking(&device, &queue, 0, 0, 128, 96)?);
    }
    let difference = images[0]
        .iter()
        .zip(&images[1])
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap();
    assert!(
        difference <= 3,
        "preview/export difference: {difference} code values"
    );
    Ok(())
}
