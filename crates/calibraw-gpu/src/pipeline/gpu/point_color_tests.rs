use super::{tests::request_test_device, GpuParams, ProcessingQuality, RawGpuPipeline};
use crate::pipeline::{
    ExposureParams, LoadedRaw, LocalMask, MaskKind, MaskStack, PointColor, ProcessingStage,
};

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
            0.05, 0.0505, 0.05, // near-neutral color noise
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

fn sampled_srgb(rgb: [f32; 3]) -> [f32; 3] {
    let linear = [
        1.660_491 * rgb[0] - 0.587_641_1 * rgb[1] - 0.072_849_9 * rgb[2],
        -0.124_550_5 * rgb[0] + 1.132_899_9 * rgb[1] - 0.008_349_4 * rgb[2],
        -0.018_150_8 * rgb[0] - 0.100_578_9 * rgb[1] + 1.118_729_7 * rgb[2],
    ];
    linear.map(|value| {
        let value = value.clamp(0.0, 1.0);
        if value <= 0.003_130_8 {
            value * 12.92
        } else {
            1.055 * value.powf(1.0 / 2.4) - 0.055
        }
    })
}

#[test]
fn local_point_color_respects_color_and_mask_coverage() -> anyhow::Result<()> {
    let Some((device, queue)) = request_test_device() else {
        eprintln!("point color GPU regression skipped: no headless wgpu adapter");
        return Ok(());
    };
    let source = source()?;
    let exposure = ExposureParams::scene_referred_default();
    let mut masks = MaskStack::default();
    let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
    let mut point = PointColor::from_srgb([0.8, 0.05, 0.03]);
    point.hue_shift = 30.0;
    mask.adjustments.point_colors.push(point);
    masks.masks.push(mask);
    let params = GpuParams::new(&exposure, &masks, &source);
    let render = |coverage: u16| -> anyhow::Result<Vec<f32>> {
        let pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
            &device,
            &queue,
            &source,
            &params,
            ProcessingQuality::High,
            64,
        )?;
        pipeline.update_mask_layer(&queue, 0, &vec![coverage; 64 * 64])?;
        pipeline.dispatch_stage(&queue, &device, &params, ProcessingStage::Raw);
        pipeline.dispatch_stage(&queue, &device, &params, ProcessingStage::Tone);
        pipeline.dispatch_stage(&queue, &device, &params, ProcessingStage::Output);
        let mut output = Vec::new();
        for x in 0..WIDTH {
            output.extend_from_slice(
                &super::read_float_texture_pixel_blocking(
                    &device,
                    &queue,
                    &pipeline.display_linear_texture,
                    pipeline.scene_format,
                    x,
                    0,
                )
                .map_err(|error| anyhow::anyhow!("pixel {x}: {error:#}"))?,
            );
        }
        Ok(output)
    };
    let unmasked = render(0).map_err(|error| anyhow::anyhow!("zero coverage: {error:#}"))?;
    let masked = render(half::f16::from_f32(1.0).to_bits())
        .map_err(|error| anyhow::anyhow!("full coverage: {error:#}"))?;
    assert!(max_delta(&unmasked, &masked, 0) > 1e-3);
    assert!(max_delta(&unmasked, &masked, 1) < 2e-4);
    Ok(())
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
fn increasing_saturation_does_not_colorize_neutral_pixels() -> anyhow::Result<()> {
    let Some((_, _)) = request_test_device() else {
        eprintln!("point color GPU regression skipped: no headless wgpu adapter");
        return Ok(());
    };
    let mut exposure = ExposureParams::scene_referred_default();
    let mut point = PointColor::from_srgb([0.5, 0.5, 0.5]);
    point.saturation_shift = 100.0;
    point.hue_range = crate::pipeline::PointColorRange::new(-0.5, -0.5, 0.5, 0.5);
    point.saturation_range = crate::pipeline::PointColorRange::new(-1.0, -1.0, 1.0, 1.0);
    point.luminance_range = crate::pipeline::PointColorRange::new(-1.0, -1.0, 1.0, 1.0);
    exposure.point_colors.push(point);

    let output = render(&exposure, ProcessingQuality::High)?;
    for pixel in [2, 3] {
        let rgb = &output[pixel * 3..pixel * 3 + 3];
        let spread = rgb.iter().copied().fold(f32::NEG_INFINITY, f32::max)
            - rgb.iter().copied().fold(f32::INFINITY, f32::min);
        assert!(
            spread < 2e-3,
            "neutral pixel {pixel} was colorized: {rgb:?}"
        );
    }
    Ok(())
}

#[test]
fn neutral_target_adjusts_near_neutral_pixel_regardless_of_noise_hue() -> anyhow::Result<()> {
    let Some((_, _)) = request_test_device() else {
        eprintln!("point color GPU regression skipped: no headless wgpu adapter");
        return Ok(());
    };
    let baseline = render(
        &ExposureParams::scene_referred_default(),
        ProcessingQuality::High,
    )?;
    let mut exposure = ExposureParams::scene_referred_default();
    let mut point = PointColor::from_srgb([0.5, 0.5, 0.5]);
    point.luminance_shift = 20.0;
    point.luminance_range = crate::pipeline::PointColorRange::new(-1.0, -1.0, 1.0, 1.0);
    exposure.point_colors.push(point);

    let adjusted = render(&exposure, ProcessingQuality::High)?;
    assert!(
        max_delta(&baseline, &adjusted, 3) > 1e-3,
        "near-neutral color noise made a gray target miss its pixel"
    );
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
fn isolated_color_noise_does_not_punch_a_hole_in_the_selection() -> anyhow::Result<()> {
    let Some((device, queue)) = request_test_device() else {
        eprintln!("point color GPU regression skipped: no headless wgpu adapter");
        return Ok(());
    };
    const SIZE: u32 = 9;
    let mut pixels = Vec::with_capacity((SIZE * SIZE * 3) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let rgb = if x == 4 && y == 4 {
                [0.30, 0.17, 0.035]
            } else if x == SIZE - 1 {
                [0.02, 0.03, 0.10]
            } else {
                [0.30, 0.09, 0.035]
            };
            pixels.extend_from_slice(&rgb);
        }
    }
    let source = LoadedRaw::from_scene_linear_rec2020(SIZE, SIZE, pixels)?;
    let masks = MaskStack::default();
    let mut exposure = ExposureParams::scene_referred_default();
    let params = GpuParams::new(&exposure, &masks, &source);
    let pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
        &device,
        &queue,
        &source,
        &params,
        ProcessingQuality::High,
        64,
    )?;
    pipeline.dispatch_stage(&queue, &device, &params, ProcessingStage::Raw);
    pipeline.dispatch_stage(&queue, &device, &params, ProcessingStage::Tone);
    pipeline.dispatch_stage(&queue, &device, &params, ProcessingStage::Output);
    let baseline = pipeline.read_output_region_blocking(&device, &queue, 0, 0, SIZE, SIZE)?;
    let target =
        sampled_srgb(pipeline.read_point_color_sample_blocking(&device, &queue, &params, 2, 2)?);
    let noisy =
        sampled_srgb(pipeline.read_point_color_sample_blocking(&device, &queue, &params, 4, 4)?);
    let mut point = PointColor::from_srgb(target);
    point.hue_range = crate::pipeline::PointColorRange::new(-0.06, -0.02, 0.02, 0.06);
    assert!(point.weight_for_srgb(noisy) < 0.05);
    exposure.point_colors.push(point);
    exposure.point_color_visualize = Some(0);
    let params = GpuParams::new(&exposure, &masks, &source);
    let pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
        &device,
        &queue,
        &source,
        &params,
        ProcessingQuality::High,
        64,
    )?;
    pipeline.dispatch_stage(&queue, &device, &params, ProcessingStage::Raw);
    pipeline.dispatch_stage(&queue, &device, &params, ProcessingStage::Tone);
    pipeline.dispatch_stage(&queue, &device, &params, ProcessingStage::Output);
    let visual = pipeline.read_output_region_blocking(&device, &queue, 0, 0, SIZE, SIZE)?;
    let selection = |x: u32, y: u32| {
        let index = ((y * SIZE + x) * 4 + 2) as usize;
        let before = f32::from(baseline[index]);
        let after = f32::from(visual[index]);
        ((after - before) / (255.0 - before) / (92.0 / 255.0)).clamp(0.0, 1.0)
    };
    assert!(selection(3, 4) > 0.9);
    assert!(
        selection(4, 4) > 0.75,
        "isolated color noise lost its selection"
    );
    assert!(selection(7, 4) > 0.9);
    assert!(selection(8, 4) < 0.1, "selection bled across a color edge");

    exposure.point_color_visualize = None;
    for hue_shift in [-100.0, 100.0] {
        exposure.point_colors[0].hue_shift = hue_shift;
        let params = GpuParams::new(&exposure, &masks, &source);
        let pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
            &device,
            &queue,
            &source,
            &params,
            ProcessingQuality::High,
            64,
        )?;
        pipeline.dispatch_stage(&queue, &device, &params, ProcessingStage::Raw);
        pipeline.dispatch_stage(&queue, &device, &params, ProcessingStage::Tone);
        pipeline.dispatch_stage(&queue, &device, &params, ProcessingStage::Output);
        let shifted = pipeline.read_output_region_blocking(&device, &queue, 0, 0, SIZE, SIZE)?;
        let color_distance = |pixels: &[u8]| {
            let first = ((4 * SIZE + 3) * 4) as usize;
            let noisy = ((4 * SIZE + 4) * 4) as usize;
            (0..3)
                .map(|channel| {
                    (i32::from(pixels[first + channel]) - i32::from(pixels[noisy + channel]))
                        .unsigned_abs()
                })
                .max()
                .unwrap_or(0)
        };
        let effect = |x: u32| {
            let index = ((4 * SIZE + x) * 4) as usize;
            (0..3)
                .map(|channel| {
                    (i32::from(shifted[index + channel]) - i32::from(baseline[index + channel]))
                        .unsigned_abs()
                })
                .max()
                .unwrap_or(0)
        };
        assert!(
            effect(3) > 100,
            "the {hue_shift} hue shift did not reach the selected color"
        );
        assert!(
            effect(4) > effect(3) * 3 / 4,
            "a noisy pixel missed the {hue_shift} hue shift"
        );
        assert!(
            color_distance(&shifted) < color_distance(&baseline) + 25,
            "the {hue_shift} hue shift amplified color noise"
        );
    }
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
    let local_input =
        pipeline.read_local_point_color_sample_blocking(&device, &queue, &edited_params, 0, 0)?;
    assert!(before
        .iter()
        .zip(local_input)
        .any(|(a, b)| (a - b).abs() > 1e-3));

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
