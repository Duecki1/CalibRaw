//! Shared by HDR merge and focus stacking: develops RAW sources to linear camera RGB with
//! the tiled demosaicer, registers them to a reference frame and writes the
//! floating-point master. Source sidecar edits are deliberately excluded: merging needs
//! linear measurements.
use super::*;
use anyhow::{ensure, Context, Result};
use calibraw_core::pipeline::alignment::{Alignment, AlignmentReference, Registration};
use image::ImageEncoder;
use rayon::prelude::*;
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A merged master: scene-linear working Rec.2020, row-major RGB.
pub struct MergedImage {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<f32>,
}

impl MergedImage {
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

/// Requires at least two distinct files.
pub(super) fn ensure_distinct_sources(paths: &[PathBuf], noun: &str) -> Result<()> {
    ensure!(paths.len() >= 2, "Select at least two RAW {noun}s");
    let mut unique = std::collections::HashSet::new();
    for path in paths {
        ensure!(
            unique.insert(std::fs::canonicalize(path)?),
            "Merge sources must be distinct files"
        );
    }
    Ok(())
}

/// The reference source's camera: every source must match it.
pub(super) struct MergeCamera {
    pub(super) width: u32,
    pub(super) height: u32,
    identity: (String, String),
    wb_coeffs: [f32; 4],
    /// Reference white balance and camera matrix: camera RGB → working Rec.2020.
    color_transform: [[f32; 3]; 3],
}

impl MergeCamera {
    fn new(raw: &LoadedRaw) -> Self {
        let (wb, matrix, _) = raw.adjusted_white_balance_and_camera_transform(0.0, 0.0);
        let wb = [wb[0], 0.5 * (wb[1] + wb[3]), wb[2]];
        let mut color_transform = [[0.0; 3]; 3];
        for (row, matrix) in color_transform.iter_mut().zip(matrix) {
            for c in 0..3 {
                row[c] = matrix[c] * wb[c];
            }
        }
        Self {
            width: raw.width,
            height: raw.height,
            identity: (raw.camera_make.clone(), raw.camera_model.clone()),
            wb_coeffs: raw.wb_coeffs,
            color_transform,
        }
    }

    /// Converts merged camera RGB to working Rec.2020; see [`camera_to_working_rec2020`].
    pub(super) fn to_working_rec2020(&self, rgb: &mut [f32]) {
        camera_to_working_rec2020(rgb, self.color_transform);
    }
}

/// A RAW file to merge.
pub(super) struct MergeSource<'a> {
    pub(super) path: &'a Path,
    /// Exposure (light × sensor gain) in a unit shared by all sources.
    pub(super) exposure: f32,
    /// Light collected (shutter × aperture area) in a unit shared by all sources.
    pub(super) light: f32,
}

/// One developed source, registered to the reference.
pub(super) struct MergeFrame<'a> {
    /// Black-subtracted, white-normalized camera RGB without white balance. Clipped
    /// highlights are reconstructed the way a single RAW develops them.
    pub(super) rgb: &'a [f32],
    /// Per pixel, the brightest normalized sensor sample (white = 1) of its demosaic
    /// footprint.
    pub(super) peak: &'a [f32],
    /// Exposure relative to the reference.
    pub(super) exposure: f32,
    /// Collected light relative to the reference.
    pub(super) light: f32,
    pub(super) registration: Registration,
}

/// Develops and registers `sources` (in processing order; the first is the reference)
/// one at a time and hands each to `merge`, so only one developed frame is resident.
/// `noun` names a source in progress messages ("exposure", "frame"). The progress
/// callback returns false to cancel between frames and tiles.
pub(super) fn merge_sources(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    sources: &[MergeSource],
    program_prewarm: Option<Arc<GpuProgramPrewarm>>,
    noun: &str,
    progress: &mut impl FnMut(String) -> bool,
    mut merge: impl FnMut(&MergeCamera, MergeFrame) -> Result<()>,
) -> Result<MergeCamera> {
    let count = sources.len();
    let mut reference_camera: Option<MergeCamera> = None;
    let mut reference: Option<AlignmentReference> = None;
    let first = sources.first().context("Nothing to merge")?;
    let (reference_exposure, reference_light) = (first.exposure, first.light);
    for (index, source) in sources.iter().enumerate() {
        let path = source.path;
        let step = format!("{noun} {}/{count}", index + 1);
        ensure!(progress(format!("Decoding {step}")), "Merge cancelled");
        let mut raw = load_raw_file_with_profile_config(path, CameraProfileMode::MatrixOnly, None)?;
        ensure!(
            !raw.is_pre_demosaiced_raster(),
            "Merging requires original RAW files, not rendered TIFFs"
        );
        if let Some(camera) = &reference_camera {
            ensure!(
                camera.identity == (raw.camera_make.clone(), raw.camera_model.clone())
                    && (camera.width, camera.height) == (raw.width, raw.height),
                "Merge sources must have the same camera and image dimensions"
            );
        }
        let camera = &*reference_camera.get_or_insert_with(|| MergeCamera::new(&raw));
        // One white balance for every source, so per-shot auto WB cannot create seams.
        raw.wb_coeffs = camera.wb_coeffs;
        let raw = Arc::new(raw);
        let rgb = render_camera_rgb(device, queue, &raw, program_prewarm.clone(), |percent| {
            progress(format!("Developing {step} ({percent:.0}%)"))
        })?;
        let peak = sensor_footprint_peak(&raw);
        drop(raw);
        ensure!(progress(format!("Aligning {step}")), "Merge cancelled");
        let exposure = source.exposure / reference_exposure;
        let registration = match &reference {
            Some(reference) => reference
                .align(&rgb, exposure)
                .with_context(|| format!("align {}", path.display()))?,
            None => {
                reference = Some(AlignmentReference::new(
                    camera.width,
                    camera.height,
                    &rgb,
                    exposure,
                )?);
                Registration {
                    alignment: Alignment::default(),
                    gain: 1.0,
                }
            }
        };
        ensure!(progress(format!("Merging {step}")), "Merge cancelled");
        merge(
            camera,
            MergeFrame {
                rgb: &rgb,
                peak: &peak,
                exposure,
                light: source.light / reference_light,
                registration,
            },
        )?;
    }
    reference_camera.context("Nothing to merge")
}

/// Applies `color_transform` (camera RGB → working Rec.2020) and preserves negative
/// out-of-gamut values and values above 1.
pub(super) fn camera_to_working_rec2020(rgb: &mut [f32], color_transform: [[f32; 3]; 3]) {
    rgb.par_chunks_exact_mut(3).for_each(|p| {
        let source = [p[0], p[1], p[2]];
        for row in 0..3 {
            p[row] = (0..3).map(|c| color_transform[row][c] * source[c]).sum();
        }
    });
}

/// Develops `raw` to camera RGB without white balance. Clipped highlights get the
/// default reconstruction, which only replaces saturated photosites: a merge weighs
/// those pixels out wherever another frame measured them, and keeps the reconstruction
/// where none did, as a single RAW would show it.
pub(super) fn render_camera_rgb(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    raw: &Arc<LoadedRaw>,
    program_prewarm: Option<Arc<GpuProgramPrewarm>>,
    mut progress: impl FnMut(f32) -> bool,
) -> Result<Vec<f32>> {
    let exposure = ExposureParams {
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
                "Merge cancelled"
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

/// Per pixel, the brightest normalized sensor sample (white = 1) within the demosaic
/// footprint, so a pixel interpolated from a saturated photosite reads as saturated even
/// where demosaicing smoothed its value below the white level.
pub(super) fn sensor_footprint_peak(raw: &LoadedRaw) -> Vec<f32> {
    let width = raw.width as usize;
    let height = raw.height as usize;
    let radius = match raw.cfa_kind {
        CfaKind::Bayer => 3,
        CfaKind::XTrans => 6,
    };
    let sensor: Vec<f32> = raw
        .raw_pixels
        .par_iter()
        .enumerate()
        .map(|(i, value)| {
            let c = usize::from(raw.color_indices[i].min(3));
            let black = raw.black_levels_per_pixel[i];
            let range = (raw.white_levels[c] - black).max(1.0);
            (f32::from(*value) - black) / range
        })
        .collect();
    let horizontal: Vec<f32> = (0..sensor.len())
        .into_par_iter()
        .map(|i| {
            let x = i % width;
            let row = i - x;
            sensor[row + x.saturating_sub(radius)..=row + (x + radius).min(width - 1)]
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max)
        })
        .collect();
    (0..sensor.len())
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % width, i / width);
            (y.saturating_sub(radius)..=(y + radius).min(height - 1))
                .map(|sy| horizontal[sy * width + x])
                .fold(f32::NEG_INFINITY, f32::max)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_tiff_round_trip_preserves_hdr_range_and_color_profile() {
        let source = MergedImage {
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
}
