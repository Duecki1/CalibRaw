use super::{
    tests::request_test_device_with_info, GpuParams, ProcessingQuality, RawGpuPipeline,
};
use crate::pipeline::{ExposureParams, LoadedRaw, LocalMask, MaskKind, MaskStack};

#[test]
fn blacks_match_between_global_masks_preview_and_export() -> anyhow::Result<()> {
    let Some((device, queue, adapter_info)) = request_test_device_with_info() else {
        eprintln!("Blacks pipeline regression skipped: no headless wgpu adapter");
        return Ok(());
    };
    if adapter_info.device_type == wgpu::DeviceType::Cpu {
        eprintln!(
            "Blacks pipeline regression skipped on software adapter: {}",
            adapter_info.name
        );
        return Ok(());
    }
    const WIDTH: u32 = 96;
    const HEIGHT: u32 = 32;
    const MASK_EDGE: u32 = 64;
    // Include noisy fabric-like patches on both sides of the former steep toe,
    // a colored shadow, true black, and white. No spatial adjustments are active.
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let noise = if (x + y) % 2 == 0 { -0.0005 } else { 0.0005 };
            let rgb = match x / 16 {
                0 => [0.0; 3],
                1 => [0.012 + noise; 3],
                2 => [0.022 + noise; 3],
                3 => [0.026 + noise; 3],
                4 => [0.05 + noise, 0.03 + noise, 0.015 + noise],
                _ => [1.0; 3],
            };
            pixels.extend_from_slice(&rgb);
        }
    }
    let source = LoadedRaw::from_scene_linear_rec2020(WIDTH, HEIGHT, pixels)?;
    let neutral = ExposureParams {
        sharpen_amount: 0.0,
        ..Default::default()
    };
    let no_masks = MaskStack::default();
    let initial = GpuParams::new(&neutral, &no_masks, &source);
    let mut preview_results = Vec::new();
    for quality in [ProcessingQuality::Preview, ProcessingQuality::High] {
        let pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
            &device, &queue, &source, &initial, quality, MASK_EDGE,
        )?;
        pipeline.update_mask_layer(
            &queue,
            0,
            &vec![half::f16::ONE.to_bits(); (MASK_EDGE * MASK_EDGE) as usize],
        )?;
        for (index, blacks) in [-100.0, -50.0, -10.0, 0.0, 50.0, 100.0]
            .into_iter()
            .enumerate()
        {
            let global = ExposureParams { blacks, ..neutral };
            let params = GpuParams::new(&global, &no_masks, &source);
            pipeline.recompute(&queue, &device, &params);
            let global_output =
                pipeline.read_output_region_blocking(&device, &queue, 0, 0, WIDTH, HEIGHT)?;

            if quality == ProcessingQuality::High && blacks < 0.0 {
                let linear = pipeline
                    .read_display_linear_region_blocking(&device, &queue, 0, 0, WIDTH, HEIGHT)?;
                for (patch, level) in [(1, 0.012), (2, 0.022), (3, 0.026)] {
                    let start = patch * 16 * 3;
                    let before = srgb(level + 0.0005) - srgb(level - 0.0005);
                    let after = (srgb(linear[start + 3]) - srgb(linear[start])).abs();
                    assert!(
                        after <= before * 1.1,
                        "Blacks {blacks}, patch {level}: noise amplified by {}",
                        after / before
                    );
                }
            }

            let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
            mask.adjustments.blacks = blacks;
            let masks = MaskStack {
                masks: vec![mask],
                ..Default::default()
            };
            let params = GpuParams::new(&neutral, &masks, &source);
            pipeline.recompute(&queue, &device, &params);
            let local_output =
                pipeline.read_output_region_blocking(&device, &queue, 0, 0, WIDTH, HEIGHT)?;
            let local_difference = global_output
                .iter()
                .zip(&local_output)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert_eq!(
                local_difference, 0,
                "global/local Blacks {blacks}, quality={quality:?}"
            );

            if quality == ProcessingQuality::Preview {
                preview_results.push(global_output);
            } else {
                let max_difference = global_output
                    .iter()
                    .zip(&preview_results[index])
                    .map(|(a, b)| a.abs_diff(*b))
                    .max()
                    .unwrap();
                assert!(
                    max_difference <= 1,
                    "preview/export Blacks {blacks}: {max_difference} code values"
                );
            }
        }
    }
    Ok(())
}

fn srgb(linear: f32) -> f32 {
    if linear <= 0.003_130_8 {
        12.92 * linear
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}
