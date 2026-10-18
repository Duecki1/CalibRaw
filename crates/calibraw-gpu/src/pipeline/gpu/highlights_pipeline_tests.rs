use super::{
    tests::request_test_device_with_info, GpuParams, PipelineOptions, ProcessingQuality,
    RawGpuPipeline,
};
use crate::pipeline::{ExposureParams, LoadedRaw, MaskStack};

const WIDTH: u32 = 256;
const HEIGHT: u32 = 16;
const LOWEST_EV: f32 = -9.0;
const HIGHEST_EV: f32 = 2.0;

/// Scene EV relative to middle grey (0.18) of a column of the test ramp.
fn column_ev(x: u32) -> f32 {
    LOWEST_EV + (HIGHEST_EV - LOWEST_EV) * x as f32 / (WIDTH - 1) as f32
}

#[test]
fn highlights_leave_deepest_shadows_and_fade_in_smoothly() -> anyhow::Result<()> {
    let Some((device, queue, adapter_info)) = request_test_device_with_info() else {
        eprintln!("Highlights pipeline regression skipped: no headless wgpu adapter");
        return Ok(());
    };
    if adapter_info.device_type == wgpu::DeviceType::Cpu {
        eprintln!(
            "Highlights pipeline regression skipped on software adapter: {}",
            adapter_info.name
        );
        return Ok(());
    }
    // A neutral horizontal ramp, uniform in EV, so the guide follows the
    // pixels and the percentiles span the whole range (p50 is near -3.5 EV).
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for _ in 0..HEIGHT {
        for x in 0..WIDTH {
            let luminance = 0.18 * column_ev(x).exp2();
            pixels.extend_from_slice(&[luminance; 3]);
        }
    }
    let source = LoadedRaw::from_scene_linear_rec2020(WIDTH, HEIGHT, pixels)?;
    let neutral = ExposureParams {
        sharpen_amount: 0.0,
        ..Default::default()
    };
    let masks = MaskStack::default();
    let pipeline = RawGpuPipeline::new(
        &device,
        &queue,
        &source,
        &GpuParams::new(&neutral, &masks, &source),
        PipelineOptions::new(ProcessingQuality::High),
    )?;
    let row_luminance = |highlights: f32| -> anyhow::Result<Vec<f32>> {
        let exposure = ExposureParams {
            highlights,
            ..neutral
        };
        pipeline.recompute(&queue, &device, &GpuParams::new(&exposure, &masks, &source));
        let row = pipeline.read_display_linear_region_blocking(
            &device,
            &queue,
            0,
            HEIGHT / 2,
            WIDTH,
            1,
        )?;
        Ok(row
            .chunks_exact(3)
            .map(|rgb| 0.2627 * rgb[0] + 0.6780 * rgb[1] + 0.0593 * rgb[2])
            .collect())
    };

    let baseline = row_luminance(0.0)?;
    for highlights in [-100.0_f32, 100.0] {
        let adjusted = row_luminance(highlights)?;
        let change_ev = adjusted
            .iter()
            .zip(&baseline)
            .map(|(after, before)| (after / before).log2())
            .collect::<Vec<_>>();
        for (x, change) in change_ev.iter().enumerate() {
            let ev = column_ev(x as u32);
            if ev <= -5.5 {
                assert!(
                    change.abs() <= 1e-3,
                    "Highlights {highlights} changed scene EV {ev} by {change} EV"
                );
            }
            if ev >= 1.0 {
                assert!(
                    change * highlights.signum() >= 0.2,
                    "Highlights {highlights} changed scene EV {ev} by only {change} EV"
                );
            }
        }
        let steepest = change_ev
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0_f32, f32::max);
        eprintln!("Highlights {highlights}: steepest step {steepest} EV per column");
        assert!(
            steepest <= 0.03,
            "Highlights {highlights} changes by {steepest} EV between adjacent columns"
        );
    }
    Ok(())
}
