//! Full-resolution RAW bracket rendering and HDR merge. Uses the existing tiled demosaicer.
use super::*;
use anyhow::{ensure, Context, Result};
use calibraw_core::pipeline::hdr::{exposure_scale, Alignment, AlignmentReference, HdrAccumulator};
use image::ImageEncoder;
use rayon::prelude::*;
use std::io::{Seek, Write};
use std::path::PathBuf;
use std::sync::Arc;

pub struct HdrMergeResult {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<f32>,
}

impl HdrMergeResult {
    /// Scene-linear Rec.2020, explicitly tagged, with no range compression or integer quantization.
    pub fn write_tiff(&self, writer: impl Write + Seek) -> Result<()> {
        let mut encoder = image::codecs::tiff::TiffEncoder::new(writer);
        encoder.set_icc_profile(super::export::linear_rec2020_icc())?;
        encoder.write_image(
            bytemuck::cast_slice(&self.rgb),
            self.width,
            self.height,
            image::ExtendedColorType::Rgb32F,
        )?;
        Ok(())
    }
}

/// The callback reports progress and returns false to cancel between frames/tiles.
/// Source sidecar edits are deliberately excluded: merging requires linear measurements.
pub fn merge_hdr_bracket(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    paths: &[PathBuf],
    program_prewarm: Option<Arc<GpuProgramPrewarm>>,
    mut progress: impl FnMut(String) -> bool,
) -> Result<HdrMergeResult> {
    ensure!(
        paths.len() >= 2,
        "Select at least two RAW exposures for HDR merge"
    );
    let mut unique = std::collections::HashSet::new();
    let mut bracket = Vec::with_capacity(paths.len());
    for path in paths {
        ensure!(
            unique.insert(std::fs::canonicalize(path)?),
            "HDR sources must be distinct files"
        );
        let metadata = load_raw_display_metadata(path)?;
        let exposure = exposure_scale(
            metadata.shutter_seconds,
            metadata.iso_speed,
            metadata.aperture,
        )
        .with_context(|| format!("{} is not a RAW exposure bracket source", path.display()))?;
        bracket.push((path, exposure));
    }
    bracket.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(b.0)));
    let reference_index = bracket.len() / 2;
    let reference_exposure = bracket[reference_index].1;
    bracket.swap(0, reference_index);
    let mut accumulator = None;
    let mut alignment_reference: Option<AlignmentReference> = None;
    let mut camera = None;
    let mut color_transform = [[0.0; 3]; 3];
    let mut dimensions = (0, 0);
    let mut reference_wb = [1.0; 4];
    for (index, (path, source_exposure)) in bracket.into_iter().enumerate() {
        ensure!(
            progress(format!(
                "HDR: decoding exposure {}/{}",
                index + 1,
                paths.len()
            )),
            "HDR merge cancelled"
        );
        let mut raw = load_raw_file_with_profile_config(path, CameraProfileMode::MatrixOnly, None)?;
        ensure!(
            !raw.is_pre_demosaiced_raster(),
            "HDR merge requires original RAW exposures, not rendered TIFFs"
        );
        let identity = (raw.camera_make.clone(), raw.camera_model.clone());
        if let Some(camera) = &camera {
            ensure!(
                *camera == identity && dimensions == (raw.width, raw.height),
                "HDR sources must have the same camera and image dimensions"
            );
        } else {
            camera = Some(identity);
            dimensions = (raw.width, raw.height);
            reference_wb = raw.wb_coeffs;
            let (wb, matrix, _) = raw.adjusted_white_balance_and_camera_transform(0.0, 0.0);
            let wb = [wb[0], 0.5 * (wb[1] + wb[3]), wb[2]];
            for row in 0..3 {
                for c in 0..3 {
                    color_transform[row][c] = matrix[row][c] * wb[c];
                }
            }
        }
        raw.wb_coeffs = reference_wb;
        let raw = Arc::new(raw);
        let relative_exposure = source_exposure / reference_exposure;
        let rgb = render_camera_rgb(device, queue, &raw, program_prewarm.clone(), |percent| {
            progress(format!(
                "HDR: developing exposure {}/{} ({percent:.0}%)",
                index + 1,
                paths.len()
            ))
        })?;
        let clipped = sensor_clipping_mask(&raw);
        drop(raw);
        ensure!(
            progress(format!(
                "HDR: aligning exposure {}/{}",
                index + 1,
                paths.len()
            )),
            "HDR merge cancelled"
        );
        let alignment = if let Some(reference) = &alignment_reference {
            reference
                .align(&rgb, relative_exposure)
                .with_context(|| format!("align {}", path.display()))?
        } else {
            alignment_reference = Some(AlignmentReference::new(
                dimensions.0,
                dimensions.1,
                &rgb,
                relative_exposure,
            )?);
            accumulator = Some(HdrAccumulator::new(dimensions.0, dimensions.1)?);
            Alignment::default()
        };
        ensure!(
            progress(format!(
                "HDR: merging exposure {}/{}",
                index + 1,
                paths.len()
            )),
            "HDR merge cancelled"
        );
        accumulator
            .as_mut()
            .unwrap()
            .add(&rgb, relative_exposure, alignment, Some(&clipped))?;
    }
    ensure!(
        progress("HDR: preparing floating-point master".to_owned()),
        "HDR merge cancelled"
    );
    let mut rgb = accumulator.context("HDR bracket is empty")?.finish()?;
    // Use one reference WB and matrix for the entire bracket; individual auto-WB differences
    // must not create color seams. Preserve negative out-of-gamut values and values above 1.
    rgb.par_chunks_exact_mut(3).for_each(|p| {
        let source = [p[0], p[1], p[2]];
        for row in 0..3 {
            p[row] = (0..3).map(|c| color_transform[row][c] * source[c]).sum();
        }
    });
    Ok(HdrMergeResult {
        width: dimensions.0,
        height: dimensions.1,
        rgb,
    })
}

fn render_camera_rgb(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    raw: &Arc<LoadedRaw>,
    program_prewarm: Option<Arc<GpuProgramPrewarm>>,
    mut progress: impl FnMut(f32) -> bool,
) -> Result<Vec<f32>> {
    let exposure = ExposureParams {
        highlight_method: HighlightReconstructionMethod::Off,
        highlight_reconstruction: 0.0,
        frequency_chroma: 0.0,
        sharpen_amount: 0.0,
        ..ExposureParams::default()
    };
    let mut rgb = vec![0.0f32; raw.width as usize * raw.height as usize * 3];
    let wb = remove_scene_white_balance(raw, &exposure);
    // Small GPU tiles keep merge independent of the device's full-image texture limit.
    const TILE: u32 = 1024;
    for y in (0..raw.height).step_by(TILE as usize) {
        for x in (0..raw.width).step_by(TILE as usize) {
            ensure!(
                progress(
                    100.0
                        * (y as f32 * raw.width as f32
                            + x as f32 * TILE.min(raw.height - y) as f32)
                        / (raw.width as f32 * raw.height as f32)
                ),
                "HDR merge cancelled"
            );
            let crop = NativeRect {
                x,
                y,
                width: TILE.min(raw.width - x),
                height: TILE.min(raw.height - y),
            };
            let tile = render_remove_scene_crop(DevelopedCropJob {
                device: device.clone(),
                queue: queue.clone(),
                raw: Arc::clone(raw),
                geometry: GeometryTransform::default(),
                exposure,
                masks: MaskStack::default(),
                remove: RemoveEditState::default(),
                crop,
                program_prewarm: program_prewarm.clone(),
            })?;
            for row in 0..crop.height as usize {
                let dest = ((y as usize + row) * raw.width as usize + x as usize) * 3;
                let start = row * crop.width as usize * 3;
                for (out, pixel) in rgb[dest..dest + crop.width as usize * 3]
                    .chunks_exact_mut(3)
                    .zip(tile[start..start + crop.width as usize * 3].chunks_exact(3))
                {
                    for c in 0..3 {
                        out[c] = pixel[c] / wb[c];
                    }
                }
            }
        }
    }
    Ok(rgb)
}

/// Demosaicing can smooth a saturated photosite below the clipping threshold. Dilate the
/// sensor clipping mask over its local color neighborhood before interpolating aligned RGB.
fn sensor_clipping_mask(raw: &LoadedRaw) -> Vec<u8> {
    let width = raw.width as usize;
    let height = raw.height as usize;
    let radius = match raw.cfa_kind {
        CfaKind::Bayer => 3,
        CfaKind::XTrans => 6,
    };
    let sensor: Vec<u8> = raw
        .raw_pixels
        .par_iter()
        .enumerate()
        .map(|(i, value)| {
            let c = usize::from(raw.color_indices[i].min(3));
            let black = raw.black_levels_per_pixel[i];
            let range = (raw.white_levels[c] - black).max(1.0);
            u8::from((*value as f32 - black) / range >= 0.98)
        })
        .collect();
    let horizontal: Vec<u8> = (0..sensor.len())
        .into_par_iter()
        .map(|i| {
            let x = i % width;
            let row = i - x;
            u8::from(
                sensor[row + x.saturating_sub(radius)..=row + (x + radius).min(width - 1)]
                    .contains(&1),
            )
        })
        .collect();
    (0..sensor.len())
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % width, i / width);
            u8::from(
                (y.saturating_sub(radius)..=(y + radius).min(height - 1))
                    .any(|sy| horizontal[sy * width + x] != 0),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn float_tiff_round_trip_preserves_hdr_range_and_color_profile() {
        let source = HdrMergeResult {
            width: 2,
            height: 1,
            rgb: vec![4.0, 2.0, 0.5, -0.01, 0.0001, 16.0],
        };
        let mut output = std::io::Cursor::new(Vec::new());
        source.write_tiff(&mut output).unwrap();
        let decoded =
            image::load_from_memory_with_format(output.get_ref(), image::ImageFormat::Tiff)
                .unwrap()
                .into_rgb32f();
        assert_eq!(decoded.as_raw(), &source.rgb);
        let mut decoder =
            image::codecs::tiff::TiffDecoder::new(std::io::Cursor::new(output.get_ref())).unwrap();
        let profile = image::ImageDecoder::icc_profile(&mut decoder)
            .unwrap()
            .unwrap();
        assert_eq!(profile, super::super::export::linear_rec2020_icc());
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
        let (width, height) = (192u32, 96u32);
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
                    } else {
                        [4.0, 2.0, 0.5]
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
        let rgb = merge.finish().unwrap();
        for (x, expected, tolerance) in [
            (32, [0.0001, 0.0002, 0.0003], 0.00002),
            (96, [0.2, 0.1, 0.05], 0.002),
            (160, [4.0, 2.0, 0.5], 0.01),
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
