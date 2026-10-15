//! HDR merge of a RAW exposure bracket into a floating-point master.
use super::raw_merge::{ensure_distinct_sources, merge_sources, MergedImage};
use super::*;
use anyhow::{ensure, Context, Result};
use calibraw_core::pipeline::hdr::{exposure_scale, HdrAccumulator};
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
        let exposure = exposure_scale(
            metadata.shutter_seconds,
            metadata.iso_speed,
            metadata.aperture,
        )
        .with_context(|| format!("{} is not a RAW exposure bracket source", path.display()))?;
        bracket.push((path.as_path(), exposure));
    }
    // The middle exposure is the reference; it is processed first.
    bracket.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(b.0)));
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
            accumulator.add(
                frame.rgb,
                frame.exposure,
                frame.registration.alignment,
                Some(frame.clipped),
            )
        },
    )?;
    ensure!(
        progress("Preparing floating-point master".to_owned()),
        "Merge cancelled"
    );
    let (mut rgb, clipped) = accumulator
        .context("HDR bracket is empty")?
        .finish_with_clipped_highlights()?;
    camera.to_working_rec2020(&mut rgb, &clipped);
    Ok(MergedImage {
        width: camera.width,
        height: camera.height,
        rgb,
    })
}

#[cfg(test)]
mod tests {
    use super::super::raw_merge::{
        camera_to_working_rec2020, render_camera_rgb, sensor_clipping_mask,
    };
    use super::*;
    use calibraw_core::pipeline::alignment::Alignment;

    /// Merges `accumulator` like `merge_hdr_bracket` with an explicit camera transform.
    fn finish_hdr_merge(
        accumulator: HdrAccumulator,
        color_transform: [[f32; 3]; 3],
    ) -> Result<Vec<f32>> {
        let (mut rgb, clipped) = accumulator.finish_with_clipped_highlights()?;
        camera_to_working_rec2020(&mut rgb, &clipped, color_transform);
        Ok(rgb)
    }

    #[test]
    fn unrecoverable_highlights_stay_neutral_at_minus_five_ev() {
        let mut merge = HdrAccumulator::new(3, 1).unwrap();
        // Pixel 0 clips even in the shortest exposure. Pixel 1 is real magenta
        // with valid measurements, and pixel 2 is black in every exposure.
        for exposure in [1.0, 1.0 / 16.0] {
            let frame = [
                1.0,
                1.0,
                1.0,
                0.2 * exposure,
                0.05 * exposure,
                0.2 * exposure,
                0.0,
                0.0,
                0.0,
            ];
            merge
                .add(&frame, exposure, Alignment::default(), None)
                .unwrap();
        }
        let rgb =
            finish_hdr_merge(merge, [[2.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.5]]).unwrap();
        let expected_luminance = 0.2627 * 32.0 + 0.6780 * 16.0 + 0.0593 * 24.0;
        assert!(expected_luminance > 1.0);
        for channel in &rgb[..3] {
            assert!(
                (channel - expected_luminance).abs() < 1e-5,
                "clipped highlight: {rgb:?}"
            );
        }
        for (actual, expected) in rgb[3..].iter().zip([0.4, 0.05, 0.3, 0.0, 0.0, 0.0]) {
            assert!(
                (actual - expected).abs() < 1e-6,
                "valid color changed: {rgb:?}"
            );
        }
        let master = MergedImage {
            width: 3,
            height: 1,
            rgb,
        };
        let mut tiff = std::io::Cursor::new(Vec::new());
        master.write_tiff(&mut tiff).unwrap();
        let reopened =
            image::load_from_memory_with_format(tiff.get_ref(), image::ImageFormat::Tiff)
                .unwrap()
                .into_rgb32f();
        let dimmed = reopened.get_pixel(0, 0).0.map(|v| v * 2.0f32.powi(-5));
        assert_eq!(dimmed[0], dimmed[1]);
        assert_eq!(dimmed[1], dimmed[2]);
    }

    #[test]
    #[ignore = "requires a compute-capable GPU"]
    fn raw_demosaic_merge_preserves_highlights_and_shadows() {
        let instance = wgpu::Instance::default();
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
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
            let clipped = sensor_clipping_mask(&raw);
            merge
                .add(&rgb, exposure, Alignment::default(), Some(&clipped))
                .unwrap();
        }
        let rgb =
            finish_hdr_merge(merge, [[2.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.5]]).unwrap();
        for (x, expected, tolerance) in [
            (32, [0.0002, 0.0002, 0.00045], 0.00004),
            (96, [0.4, 0.1, 0.075], 0.004),
            (160, [8.0, 2.0, 0.75], 0.02),
            (224, [20.6776; 3], 0.01),
        ] {
            let i = (48 * width as usize + x) * 3;
            for c in 0..3 {
                assert!(
                    (rgb[i + c] - expected[c]).abs() < tolerance,
                    "x={x} channel={c}: {} != {}",
                    rgb[i + c],
                    expected[c]
                );
            }
        }
    }
}
