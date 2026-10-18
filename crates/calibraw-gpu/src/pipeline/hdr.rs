//! HDR merge of a RAW exposure bracket into a floating-point master.
use super::raw_merge::{ensure_distinct_sources, merge_sources, MergeSource, MergedImage};
use super::*;
use anyhow::{ensure, Context, Result};
use calibraw_core::pipeline::hdr::{collected_light, exposure_scale, HdrAccumulator, HdrFrame};
use std::path::PathBuf;
use std::sync::Arc;

/// The callback reports progress and returns false to cancel between frames/tiles.
pub fn merge_hdr_bracket(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    paths: &[PathBuf],
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
        program_prewarm,
        "exposure",
        &mut progress,
        |camera, frame| {
            let accumulator = match &mut accumulator {
                Some(accumulator) => accumulator,
                None => accumulator.insert(HdrAccumulator::new(camera.width, camera.height)?),
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
    let mut rgb = accumulator.context("HDR bracket is empty")?.finish()?;
    camera.to_working_rec2020(&mut rgb);
    Ok(MergedImage {
        width: camera.width,
        height: camera.height,
        rgb,
    })
}

#[cfg(test)]
mod tests {
    use super::super::raw_merge::{
        camera_to_working_rec2020, render_camera_rgb, sensor_footprint_peak,
    };
    use super::*;
    use calibraw_core::pipeline::alignment::{Alignment, Registration};

    #[test]
    #[ignore = "requires a compute-capable GPU"]
    fn raw_demosaic_merge_preserves_highlights_and_shadows() {
        let crate::HeadlessDevice { device, queue, .. } =
            crate::request_headless_device(&crate::HeadlessDeviceRequest {
                label: "HDR merge test",
                adapter: crate::HeadlessAdapter::Preferred(wgpu::PowerPreference::None),
                texture_dimension: wgpu::Limits::default().max_texture_dimension_2d,
            })
            .unwrap();
        let (width, height) = (256u32, 96u32);
        let mut merge = HdrAccumulator::new(width, height).unwrap();
        for exposure in [1.0, 1.0 / 16.0, 16.0] {
            let mut raw = LoadedRaw::from_scene_linear_rec2020(
                width,
                height,
                vec![0.0; width as usize * height as usize * 3],
            )
            .unwrap();
            raw.scene_linear_raster = None;
            raw.wb_coeffs = [2.0, 1.0, 1.5, 1.0];
            raw.black_levels = [64.0; 4];
            raw.black_levels_per_pixel =
                CompactPixelMap::repeating(width, height, 1, 1, vec![64.0]);
            raw.white_levels = [4095.0; 4];
            raw.color_indices = CompactPixelMap::repeating(width, height, 2, 2, vec![0, 1, 3, 2]);
            raw.raw_pixels = (0..width * height)
                .map(|i| {
                    let x = i % width;
                    let physical = raw.color_indices[i as usize];
                    let c = if physical == 3 { 1 } else { physical as usize };
                    let scene = if x < 64 {
                        [0.0001f32, 0.0002, 0.0003]
                    } else if x < 128 {
                        [0.2, 0.1, 0.05]
                    } else if x < 192 {
                        [4.0, 2.0, 0.5]
                    } else {
                        [64.0, 64.0, 64.0]
                    };
                    (64.0 + (scene[c] * exposure).min(1.0) * 4031.0).round() as u16
                })
                .collect();
            let raw = Arc::new(raw);
            let rgb = render_camera_rgb(&device, &queue, &raw, None, |_| true).unwrap();
            let peak = sensor_footprint_peak(&raw);
            merge
                .add(HdrFrame {
                    rgb: &rgb,
                    peak: &peak,
                    exposure,
                    light: exposure,
                    registration: Registration {
                        alignment: Alignment::default(),
                        gain: 1.0,
                    },
                })
                .unwrap();
        }
        let mut rgb = merge.finish().unwrap();
        camera_to_working_rec2020(
            &mut rgb,
            [[2.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.5]],
        );
        let at = |x: usize| &rgb[(48 * width as usize + x) * 3..][..3];
        for (x, expected, tolerance) in [
            (32, [0.0002, 0.0002, 0.00045], 0.00004),
            (96, [0.4, 0.1, 0.075], 0.004),
            (160, [8.0, 2.0, 0.75], 0.02),
        ] {
            for (c, (actual, expected)) in at(x).iter().zip(expected).enumerate() {
                assert!(
                    (actual - expected).abs() < tolerance,
                    "x={x} channel={c}: {actual} != {expected}"
                );
            }
        }
        // Clipped in every exposure: the shortest exposure's reconstruction, which never
        // falls below the clip it measured.
        for (c, clip) in [32.0, 16.0, 24.0].into_iter().enumerate() {
            assert!(
                at(224)[c] >= 0.99 * clip,
                "clipped channel {c}: {:?}",
                at(224)
            );
        }
    }
}
