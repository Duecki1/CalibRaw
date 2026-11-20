//! HDR merge of a RAW exposure bracket into a floating-point master.
use super::raw_merge::{
    ensure_distinct_sources, merge_sources, MergeOptions, MergeSource, MergedImage, SourceDenoiser,
};
use super::*;
use anyhow::{ensure, Context, Result};
use calibraw_core::pipeline::hdr::{collected_light, exposure_scale, HdrAccumulator, HdrFrame};
use std::path::PathBuf;
use std::sync::Arc;

/// `options.reject_motion` deghosts. `denoise`, when given, AI-denoises every source.
/// The callback reports progress and returns false to cancel between frames/tiles.
pub fn merge_hdr_bracket(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    paths: &[PathBuf],
    options: MergeOptions,
    denoise: Option<&mut SourceDenoiser>,
    program_prewarm: Option<Arc<GpuProgramPrewarm>>,
    mut progress: impl FnMut(String) -> bool,
) -> Result<MergedImage> {
    ensure_distinct_sources(paths, "exposure")?;
    let mut bracket = Vec::with_capacity(paths.len());
    for path in paths {
        let metadata = load_raw_display_metadata(path)?;
        let not_a_bracket = || format!("{} is not a RAW exposure bracket source", path.display());
        let exposure = exposure_scale(
            metadata.shutter_seconds,
            metadata.iso_speed,
            metadata.aperture,
        )
        .with_context(not_a_bracket)?;
        let light = collected_light(metadata.shutter_seconds, metadata.aperture)
            .with_context(not_a_bracket)?;
        bracket.push(MergeSource {
            path,
            exposure,
            light,
        });
    }
    // The middle exposure is the reference; it is processed first.
    bracket.sort_by(|a, b| {
        a.exposure
            .total_cmp(&b.exposure)
            .then_with(|| a.path.cmp(b.path))
    });
    let reference_index = bracket.len() / 2;
    bracket.swap(0, reference_index);
    let mut accumulator = None;
    let camera = merge_sources(
        device,
        queue,
        &bracket,
        options,
        denoise,
        program_prewarm,
        "exposure",
        &mut progress,
        |camera, frame| {
            let accumulator = match &mut accumulator {
                Some(accumulator) => accumulator,
                None => accumulator.insert(HdrAccumulator::new(
                    camera.width,
                    camera.height,
                    options.reject_motion,
                )?),
            };
            accumulator.add(HdrFrame {
                rgb: frame.rgb,
                peak: frame.peak,
                exposure: frame.exposure,
                light: frame.light,
                registration: frame.registration,
            })
        },
    )?;
    ensure!(
        progress("Preparing floating-point master".to_owned()),
        "Merge cancelled"
    );
    let rgb = accumulator.context("HDR bracket is empty")?.finish()?;
    Ok(camera.finish(rgb))
}
