use super::{tests::request_test_device, GpuParams, ProcessingQuality, RawGpuPipeline};
use crate::pipeline::{ExposureParams, LoadedRaw, MaskStack, PointColor, ProcessingStage};

const WIDTH: u32 = 4;
const HEIGHT: u32 = 1;

fn source() -> anyhow::Result<LoadedRaw> {
    // Two saturated patches followed by neutral pixels. Keeping the patches
    // separate makes the range and hue-domain assertions easy to diagnose.
    LoadedRaw::from_scene_linear_rec2020(
        WIDTH,
        HEIGHT,
        vec![
            0.50, 0.025, 0.015, // red
            0.025, 0.50, 0.025, // green
            0.20, 0.20, 0.20, // neutral
            0.05, 0.05, 0.05, // neutral
        ],
    )
}

fn render(exposure: &ExposureParams, quality: ProcessingQuality) -> anyhow::Result<Vec<f32>> {
    let Some((device, queue)) = request_test_device() else {
        return Ok(Vec::new());
    };
    let source = source()?;
    let masks = MaskStack::default();
    let params = GpuParams::new(exposure, &masks, &source);
    let pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
        &device, &queue, &source, &params, quality, 64,
    )?;
    pipeline.dispatch_stage(&queue, &device, &params, ProcessingStage::Raw);
    pipeline.dispatch_stage(&queue, &device, &params, ProcessingStage::Tone);
    pipeline.dispatch_stage(&queue, &device, &params, ProcessingStage::Output);
    let mut output = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for x in 0..WIDTH {
        output.extend_from_slice(&super::read_float_texture_pixel_blocking(
            &device,
            &queue,
            &pipeline.display_linear_texture,
            pipeline.scene_format,
            x,
            0,
        )?);
    }
    Ok(output)
}

fn max_delta(a: &[f32], b: &[f32], pixel: usize) -> f32 {
    a[pixel * 3..pixel * 3 + 3]
        .iter()
        .zip(&b[pixel * 3..pixel * 3 + 3])
        .map(|(left, right)| (left - right).abs())
        .fold(0.0, f32::max)
}

#[test]
fn zero_shift_point_color_is_an_identity() -> anyhow::Result<()> {
    let Some((_, _)) = request_test_device() else {
        eprintln!("point color GPU regression skipped: no headless wgpu adapter");
        return Ok(());
    };
    let baseline = render(
        &ExposureParams::scene_referred_default(),
        ProcessingQuality::High,
    )?;
    let mut exposure = ExposureParams::scene_referred_default();
    exposure
        .point_colors
        .push(PointColor::from_srgb([0.8, 0.05, 0.03]));
    let with_point = render(&exposure, ProcessingQuality::High)?;
    assert_eq!(baseline.len(), with_point.len());
    assert!(baseline
        .iter()
        .zip(&with_point)
        .all(|(left, right)| (left - right).abs() < 2e-5));
    Ok(())
}

#[test]
fn sampled_hue_shift_changes_matching_patch_only() -> anyhow::Result<()> {
    let Some((_, _)) = request_test_device() else {
        eprintln!("point color GPU regression skipped: no headless wgpu adapter");
        return Ok(());
    };
    let baseline = render(
        &ExposureParams::scene_referred_default(),
        ProcessingQuality::High,
    )?;
    let mut exposure = ExposureParams::scene_referred_default();
    let mut point = PointColor::from_srgb([0.8, 0.05, 0.03]);
    point.hue_shift = 30.0;
    exposure.point_colors.push(point);
    let shifted = render(&exposure, ProcessingQuality::High)?;
    assert!(max_delta(&baseline, &shifted, 0) > 1e-3);
    assert!(
        max_delta(&baseline, &shifted, 1) < 2e-4,
        "green changed by {}: baseline={:?}, shifted={:?}",
        max_delta(&baseline, &shifted, 1),
        &baseline[3..6],
        &shifted[3..6]
    );
    Ok(())
}

#[test]
fn preview_and_high_quality_point_color_outputs_agree() -> anyhow::Result<()> {
    let Some((device, queue)) = request_test_device() else {
        eprintln!("point color GPU regression skipped: no headless wgpu adapter");
        return Ok(());
    };
    let source = source()?;
    let masks = MaskStack::default();
    let mut exposure = ExposureParams::scene_referred_default();
    let mut point = PointColor::from_srgb([0.8, 0.05, 0.03]);
    point.hue_shift = 20.0;
    exposure.point_colors.push(point);
    let params = GpuParams::new(&exposure, &masks, &source);
    let mut outputs = Vec::new();
    for quality in [ProcessingQuality::Preview, ProcessingQuality::High] {
        let pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
            &device, &queue, &source, &params, quality, 64,
        )?;
        pipeline.dispatch_stage(&queue, &device, &params, ProcessingStage::Raw);
        pipeline.dispatch_stage(&queue, &device, &params, ProcessingStage::Tone);
        pipeline.dispatch_stage(&queue, &device, &params, ProcessingStage::Output);
        let mut output = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
        for x in 0..WIDTH {
            output.extend_from_slice(&super::read_float_texture_pixel_blocking(
                &device,
                &queue,
                &pipeline.display_linear_texture,
                pipeline.scene_format,
                x,
                0,
            )?);
        }
        outputs.push(output);
    }
    assert_eq!(outputs[0].len(), outputs[1].len());
    let error = outputs[0]
        .iter()
        .zip(&outputs[1])
        .map(|(left, right)| (left - right).abs())
        .fold(0.0, f32::max);
    assert!(error < 0.03, "preview/high point color error was {error}");
    Ok(())
}

#[test]
fn sampling_domain_is_stable_and_visualization_preserves_unselected_colors() -> anyhow::Result<()> {
    let Some((device, queue)) = request_test_device() else {
        eprintln!("point color GPU regression skipped: no headless wgpu adapter");
        return Ok(());
    };
    let source = source()?;
    let masks = MaskStack::default();
    let mut baseline_exposure = ExposureParams::scene_referred_default();
    let baseline_params = GpuParams::new(&baseline_exposure, &masks, &source);
    let pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
        &device,
        &queue,
        &source,
        &baseline_params,
        ProcessingQuality::Preview,
        64,
    )?;
    pipeline.dispatch_stage(&queue, &device, &baseline_params, ProcessingStage::Raw);
    pipeline.dispatch_stage(&queue, &device, &baseline_params, ProcessingStage::Tone);
    pipeline.dispatch_stage(&queue, &device, &baseline_params, ProcessingStage::Output);
    let before =
        pipeline.read_point_color_sample_blocking(&device, &queue, &baseline_params, 0, 0)?;

    let mut point = PointColor::from_srgb([0.8, 0.05, 0.03]);
    point.hue_shift = 30.0;
    baseline_exposure.point_colors.push(point);
    baseline_exposure.point_color_visualize = Some(0);
    let edited_params = GpuParams::new(&baseline_exposure, &masks, &source);
    let after = pipeline.read_point_color_sample_blocking(&device, &queue, &edited_params, 0, 0)?;
    assert!(before.iter().zip(after).all(|(a, b)| (a - b).abs() < 2e-3));

    let red = super::read_float_texture_pixel_blocking(
        &device,
        &queue,
        &pipeline.display_linear_texture,
        pipeline.scene_format,
        0,
        0,
    )?;
    let green = super::read_float_texture_pixel_blocking(
        &device,
        &queue,
        &pipeline.display_linear_texture,
        pipeline.scene_format,
        1,
        0,
    )?;
    assert!((red[0] - red[1]).abs() > 1e-3 || (red[1] - red[2]).abs() > 1e-3);
    assert!(
        (green[0] - green[1]).abs() > 1e-3 || (green[1] - green[2]).abs() > 1e-3,
        "unselected green visualization pixel lost its chroma: {:?}",
        green
    );
    Ok(())
}
