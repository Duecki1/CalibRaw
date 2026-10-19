//! Scene gamut compression (`compress_scene_gamut` in color.wgsl, applied by
//! `scene_working_at` in scene_source.wgsl). Narrow-band light that a camera
//! matrix puts outside Rec.2020 keeps its colour instead of turning black, and
//! developed photos keep their rendered colours.

use super::{
    fog_tests::neutral_exposure, tests::request_test_device, GpuParams, PipelineOptions,
    ProcessingQuality, RawGpuPipeline,
};
use crate::pipeline::{LoadedRaw, MaskStack, ProcessingStage};

const EDGE: u32 = 4;
/// Rec.2020 luminance weights.
const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

/// Display-linear RGB of a uniform working-Rec.2020 raster, or `None` without
/// a GPU adapter. A uniform frame keeps neighbourhood filters out of the result.
fn render_uniform(rgb: [f32; 3], scene_referred: bool) -> anyhow::Result<Option<[f32; 3]>> {
    let Some((device, queue)) = request_test_device() else {
        eprintln!("scene gamut GPU regression skipped: no headless wgpu adapter");
        return Ok(None);
    };
    let mut source =
        LoadedRaw::from_scene_linear_rec2020(EDGE, EDGE, rgb.repeat((EDGE * EDGE) as usize))?;
    source.scene_referred_raster = scene_referred;
    let exposure = neutral_exposure();
    let masks = MaskStack::default();
    let params = GpuParams::new(&exposure, &masks, &source);
    let pipeline = RawGpuPipeline::new(
        &device,
        &queue,
        &source,
        &params,
        PipelineOptions::new(ProcessingQuality::High).mask_atlas_edge(64),
    )?;
    for stage in [
        ProcessingStage::Raw,
        ProcessingStage::Tone,
        ProcessingStage::Output,
    ] {
        pipeline.dispatch_stage(&queue, &device, &params, stage);
    }
    let rgb = super::read_float_texture_pixel_blocking(
        &device,
        &queue,
        &pipeline.display_linear_texture,
        pipeline.scene_format,
        EDGE / 2,
        EDGE / 2,
    )?;
    Ok(Some(rgb))
}

#[test]
fn out_of_gamut_light_keeps_its_hue_and_brightness() -> anyhow::Result<()> {
    // Violet LED light after a camera matrix: green below zero, down to
    // negative luminance. The boundary colour itself is past the threshold,
    // so it is compressed too and bounds the green of anything farther out.
    let Some(boundary) = render_uniform([0.30, 0.0, 1.0], true)? else {
        return Ok(());
    };
    assert!(
        boundary[2] > boundary[0] && boundary[0] > boundary[1] && boundary[2] > 0.1,
        "boundary violet should render as a bright violet: {boundary:?}"
    );
    for green in [-2.0, -0.4, -0.05] {
        let input = [0.30, green, 1.0];
        let Some(rendered) = render_uniform(input, true)? else {
            return Ok(());
        };
        assert!(
            rendered
                .iter()
                .all(|value| value.is_finite() && *value >= 0.0),
            "{input:?} rendered {rendered:?}"
        );
        for channel in [0, 2] {
            assert!(
                (rendered[channel] - boundary[channel]).abs() < 0.05 * boundary[channel],
                "{input:?} rendered {rendered:?}, lost the red or blue of {boundary:?}"
            );
        }
        assert!(
            rendered[1] <= boundary[1],
            "{input:?} rendered {rendered:?}, less saturated than the boundary {boundary:?}"
        );
    }
    let luminance: f32 = [0.30f32, -2.0, 1.0]
        .iter()
        .zip(LUMA)
        .map(|(c, w)| c * w)
        .sum();
    assert!(luminance < 0.0, "the test must cover negative luminance");
    Ok(())
}

#[test]
fn developed_rasters_keep_their_saturated_colours() -> anyhow::Result<()> {
    // sRGB blue in Rec.2020 lies past the compression threshold; a developed
    // photo must still render it unchanged.
    let srgb_blue = [0.043_303, 0.011_360, 0.895_617];
    let Some(rendered) = render_uniform(srgb_blue, false)? else {
        return Ok(());
    };
    let input_ratio = srgb_blue[1] / srgb_blue[2];
    let rendered_ratio = rendered[1] / rendered[2];
    assert!(
        (rendered_ratio - input_ratio).abs() < 1e-3,
        "rendered {rendered:?} changed the green/blue ratio {input_ratio} to {rendered_ratio}"
    );
    Ok(())
}
