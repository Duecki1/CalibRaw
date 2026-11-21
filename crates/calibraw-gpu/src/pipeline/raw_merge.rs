//! Shared by HDR merge and focus stacking: develops RAW sources to linear camera RGB with
//! the tiled demosaicer, registers them to a reference frame and writes the
//! floating-point master. Source sidecar edits are deliberately excluded: merging needs
//! linear measurements.
use super::*;
use anyhow::{ensure, Context, Result};
use calibraw_core::pipeline::alignment::{
    correct_lens_distortion, Alignment, AlignmentReference, Registration,
};
use image::ImageEncoder;
use rayon::prelude::*;
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// A merged master: scene-linear working Rec.2020, row-major RGB.
pub struct MergedImage {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<f32>,
    /// Whether a lens profile corrected it: false when lens correction was off or no
    /// profile matched the reference source.
    pub lens_corrected: bool,
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

/// How a merge develops and combines its sources. [`Default`] is what a merge does
/// unless the user turns something off: correct the lens, align, and reject motion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MergeOptions {
    /// Repair each source's hot pixels ([`remove_hot_pixels`]) in its mosaic, as
    /// the Detail card does for a single RAW. A source without a mosaic (a linear
    /// DNG) has no photosites to repair and merges as decoded.
    pub remove_hot_pixels: bool,
    /// Correct distortion, lateral chromatic aberration and vignetting with the
    /// Lensfun profile that matches the reference source automatically. Every
    /// source takes that profile at its own focal length and aperture. Without a
    /// match, the merge is not lens corrected ([`MergedImage::lens_corrected`]).
    pub lens_correction: bool,
    /// Register every source to the reference. Without it the sources are merged
    /// pixel for pixel, as shot from a tripod.
    pub align: bool,
    /// HDR merge only: fade out a frame where it disagrees with the reference, so
    /// moving subjects do not ghost.
    pub reject_motion: bool,
}

impl Default for MergeOptions {
    fn default() -> Self {
        Self {
            remove_hot_pixels: false,
            lens_correction: true,
            align: true,
            reject_motion: true,
        }
    }
}

/// Installs an AI-denoised image on a decoded source
/// ([`LoadedRaw::set_ai_denoised_image`]), which the merge then develops from.
/// calibraw-gpu cannot run AI models, so the caller supplies this. It gets each
/// source with a sensor mosaic, after the merge's corrections and white balance.
pub type SourceDenoiser<'a> = dyn FnMut(&Arc<LoadedRaw>) -> Result<()> + 'a;

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
    /// The reference's distortion correction, which develops leave to a later
    /// geometry pass: applied to the merged master.
    lens_geometry: Option<Arc<LensGeometryMap>>,
    /// The Lensfun profile every source is corrected with; `None` leaves them as
    /// decoded.
    lens: Option<LensfunLens>,
    /// The reference camera's sensor diagonal relative to 35 mm film, when Lensfun
    /// knows the camera.
    crop_factor: Option<f32>,
}

impl MergeCamera {
    /// `raw` is the reference, corrected with `lens`.
    fn new(raw: &LoadedRaw, lens: Option<LensfunLens>, crop_factor: Option<f32>) -> Self {
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
            lens_geometry: raw.lens_geometry.clone(),
            lens,
            crop_factor,
        }
    }

    /// The master of merged camera RGB `rgb` (reference units, sensor coordinates):
    /// distortion corrected when the sources were lens corrected, then converted to
    /// working Rec.2020 ([`Self::to_working_rec2020`]).
    pub(super) fn finish(&self, mut rgb: Vec<f32>) -> MergedImage {
        if let Some(map) = &self.lens_geometry {
            rgb = correct_lens_distortion(self.width, self.height, &rgb, map);
        }
        self.to_working_rec2020(&mut rgb);
        MergedImage {
            width: self.width,
            height: self.height,
            rgb,
            lens_corrected: self.lens_corrected(),
        }
    }

    /// Converts camera RGB in reference units to working Rec.2020 with the reference's
    /// white balance and camera matrix ([`camera_to_working_rec2020`]).
    pub(super) fn to_working_rec2020(&self, rgb: &mut [f32]) {
        camera_to_working_rec2020(rgb, self.color_transform);
    }

    /// Whether a Lensfun profile corrected the sources.
    pub(super) fn lens_corrected(&self) -> bool {
        self.lens.is_some()
    }

    pub(super) fn crop_factor(&self) -> Option<f32> {
        self.crop_factor
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
/// callback returns false to cancel between frames and tiles. `options` corrects
/// and registers each source; `denoise`, when given, AI-denoises it.
///
/// A source's frame stays in sensor coordinates, as its saturation peak is
/// measured: the distortion correction is applied to the merged master
/// ([`MergeCamera::finish`]), so alignment registers frames that share the lens's
/// distortion and only one resampling corrects it.
#[allow(clippy::too_many_arguments)]
pub(super) fn merge_sources(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    sources: &[MergeSource],
    options: MergeOptions,
    denoise: Option<&mut SourceDenoiser>,
    program_prewarm: Option<Arc<GpuProgramPrewarm>>,
    noun: &str,
    progress: &mut impl FnMut(String) -> bool,
    mut merge: impl FnMut(&MergeCamera, MergeFrame) -> Result<()>,
) -> Result<MergeCamera> {
    let count = sources.len();
    let mut developer = SourceDeveloper::new(device, queue, options, denoise, program_prewarm);
    let mut reference: Option<AlignmentReference> = None;
    let first = sources.first().context("Nothing to merge")?;
    let (reference_exposure, reference_light) = (first.exposure, first.light);
    for (index, source) in sources.iter().enumerate() {
        let path = source.path;
        let step = format!("{noun} {}/{count}", index + 1);
        let developed = developer.develop(path, &step, progress)?;
        let camera = developer.camera().context("Nothing to merge")?;
        let exposure = source.exposure / reference_exposure;
        let unregistered = Registration {
            alignment: Alignment::default(),
            gain: 1.0,
        };
        let registration = match &reference {
            _ if !options.align => unregistered,
            Some(reference) => {
                ensure!(progress(format!("Aligning {step}")), "Merge cancelled");
                reference
                    .align(&developed.rgb, exposure)
                    .with_context(|| format!("align {}", path.display()))?
            }
            None => {
                ensure!(progress(format!("Aligning {step}")), "Merge cancelled");
                reference = Some(AlignmentReference::new(
                    camera.width,
                    camera.height,
                    &developed.rgb,
                    exposure,
                )?);
                unregistered
            }
        };
        ensure!(progress(format!("Merging {step}")), "Merge cancelled");
        merge(
            camera,
            MergeFrame {
                rgb: &developed.rgb,
                peak: &developed.peak,
                exposure,
                light: source.light / reference_light,
                registration,
            },
        )?;
    }
    developer.into_camera().context("Nothing to merge")
}

/// One source developed by [`SourceDeveloper::develop`], in sensor coordinates.
pub(super) struct DevelopedSource {
    /// Black-subtracted, white-normalized camera RGB without white balance, row-major.
    /// Clipped highlights are reconstructed the way a single RAW develops them.
    pub(super) rgb: Vec<f32>,
    /// Per pixel, the brightest normalized sensor sample (white = 1) of its demosaic
    /// footprint ([`sensor_footprint_peak`]).
    pub(super) peak: Vec<f32>,
    /// This source's distortion correction, which the develop leaves to a later
    /// geometry pass. Sources shot at other focal lengths than the reference have
    /// their own.
    pub(super) lens_geometry: Option<Arc<LensGeometryMap>>,
    /// Focal length in millimetres from the source's metadata; 0 when unknown.
    pub(super) focal_length_mm: f32,
}

/// Develops merge sources one at a time the way every merge needs them: from the
/// original RAW, with hot pixels repaired, the reference's Lensfun profile and white
/// balance, and optionally AI denoise. The first source developed is the reference.
pub(super) struct SourceDeveloper<'a, 'd> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    options: MergeOptions,
    denoise: Option<&'a mut SourceDenoiser<'d>>,
    program_prewarm: Option<Arc<GpuProgramPrewarm>>,
    camera: Option<MergeCamera>,
}

impl<'a, 'd> SourceDeveloper<'a, 'd> {
    pub(super) fn new(
        device: &'a wgpu::Device,
        queue: &'a wgpu::Queue,
        options: MergeOptions,
        denoise: Option<&'a mut SourceDenoiser<'d>>,
        program_prewarm: Option<Arc<GpuProgramPrewarm>>,
    ) -> Self {
        Self {
            device,
            queue,
            options,
            denoise,
            program_prewarm,
            camera: None,
        }
    }

    /// The reference's camera, once a source has been developed.
    pub(super) fn camera(&self) -> Option<&MergeCamera> {
        self.camera.as_ref()
    }

    pub(super) fn into_camera(self) -> Option<MergeCamera> {
        self.camera
    }

    /// Develops the RAW at `path`. `step` names it in progress messages ("frame
    /// 2/5"); the callback returns false to cancel between steps and tiles.
    pub(super) fn develop(
        &mut self,
        path: &Path,
        step: &str,
        progress: &mut impl FnMut(String) -> bool,
    ) -> Result<DevelopedSource> {
        ensure!(progress(format!("Decoding {step}")), "Merge cancelled");
        let mut raw = load_raw_file_with_profile_config(path, CameraProfileMode::MatrixOnly, None)?;
        // Linear DNGs merge like sensor RAWs: their pixels are camera RGB
        // before white balance, as `render_camera_rgb` develops a mosaic.
        ensure!(
            !raw.is_display_referred_raster(),
            "Merging requires original RAW files, not rendered TIFFs"
        );
        if let Some(camera) = &self.camera {
            ensure!(
                camera.identity == (raw.camera_make.clone(), raw.camera_model.clone())
                    && (camera.width, camera.height) == (raw.width, raw.height),
                "Merge sources must have the same camera and image dimensions"
            );
        }
        // The reference picks the profile, so every source gets the same one.
        let (lens, crop_factor) = match &self.camera {
            Some(camera) => (camera.lens.clone(), camera.crop_factor),
            None => {
                let catalog = lensfun_catalog(&raw);
                let lens = if self.options.lens_correction {
                    if catalog.auto_match.is_none() {
                        calibraw_core::diagnostics::record(format!(
                            "Merge: no Lensfun profile matches {}; not lens corrected",
                            path.display()
                        ));
                    }
                    catalog.auto_match
                } else {
                    None
                };
                (lens, catalog.camera_crop_factor)
            }
        };
        if lens.is_some() || self.options.remove_hot_pixels {
            ensure!(progress(format!("Correcting {step}")), "Merge cancelled");
        }
        // Before anything reads the mosaic, so neither the developed frame nor
        // its saturation peak sees a hot photosite.
        if self.options.remove_hot_pixels {
            if let Some(repaired) = remove_hot_pixels(&raw) {
                raw = repaired;
            }
        }
        // A sample clips at the sensor, so the peak is measured before the
        // vignetting correction brightens the corners past white. The lens
        // correction moves no sample further than lateral chromatic
        // aberration, well inside the peak's demosaic footprint.
        let peak = sensor_footprint_peak(&raw);
        if let Some(lens) = &lens {
            raw = apply_lens_correction(&raw, &LensCorrection::Profile(lens.clone()))
                .with_context(|| format!("correct the lens of {}", path.display()))?;
        }
        let camera = &*self
            .camera
            .get_or_insert_with(|| MergeCamera::new(&raw, lens, crop_factor));
        // One white balance for every source, so per-shot auto WB cannot create seams.
        raw.wb_coeffs = camera.wb_coeffs;
        let lens_geometry = raw.lens_geometry.clone();
        let focal_length_mm = raw.focal_length;
        let raw = Arc::new(raw);
        let denoised = match self.denoise.as_deref_mut() {
            Some(denoise) if !raw.is_pre_demosaiced_raster() => {
                ensure!(progress(format!("Denoising {step}")), "Merge cancelled");
                denoise(&raw).with_context(|| format!("AI denoise {}", path.display()))?;
                true
            }
            _ => false,
        };
        let rgb = render_camera_rgb(
            self.device,
            self.queue,
            &raw,
            denoised,
            self.program_prewarm.clone(),
            |development| match development {
                Development::Developed(percent) => {
                    progress(format!("Developing {step} ({percent:.0}%)"))
                }
                Development::WaitingForGpu(waited) => progress(format!(
                    "Developing {step}: waiting for the GPU ({} s)",
                    waited.as_secs()
                )),
            },
        )?;
        Ok(DevelopedSource {
            rgb,
            peak,
            lens_geometry,
            focal_length_mm,
        })
    }
}

/// Applies `color_transform` (camera RGB → working Rec.2020) and preserves negative
/// out-of-gamut values and values above 1.
fn camera_to_working_rec2020(rgb: &mut [f32], color_transform: [[f32; 3]; 3]) {
    rgb.par_chunks_exact_mut(3).for_each(|p| {
        let source = [p[0], p[1], p[2]];
        for row in 0..3 {
            p[row] = (0..3).map(|c| color_transform[row][c] * source[c]).sum();
        }
    });
}

/// How far [`render_camera_rgb`] has got.
#[derive(Clone, Copy, Debug)]
pub(super) enum Development {
    /// Percent of the image developed.
    Developed(f32),
    /// A tile has been waiting this long for the GPU, at least [`SLOW_GPU_WAIT`].
    WaitingForGpu(Duration),
}

/// Develops `raw` to camera RGB without white balance. Clipped highlights get the
/// default reconstruction, which only replaces saturated photosites: a merge weighs
/// those pixels out wherever another frame measured them, and keeps the reconstruction
/// where none did, as a single RAW would show it. `ai_denoised` develops the
/// AI-denoised image installed on `raw` instead of its mosaic.
pub(super) fn render_camera_rgb(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    raw: &Arc<LoadedRaw>,
    ai_denoised: bool,
    program_prewarm: Option<Arc<GpuProgramPrewarm>>,
    mut progress: impl FnMut(Development) -> bool,
) -> Result<Vec<f32>> {
    let exposure = ExposureParams {
        frequency_chroma: 0.0,
        sharpen_amount: 0.0,
        ai_denoise_enabled: ai_denoised,
        ..ExposureParams::default()
    };
    let mut rgb = vec![0.0f32; raw.width as usize * raw.height as usize * 3];
    let wb = remove_scene_white_balance(raw, &exposure);
    // Small GPU tiles keep merge independent of the device's full-image texture limit.
    const TILE: u32 = 1024;
    for y in (0..raw.height).step_by(TILE as usize) {
        for x in (0..raw.width).step_by(TILE as usize) {
            let percent = 100.0
                * (y as f32 * raw.width as f32 + x as f32 * TILE.min(raw.height - y) as f32)
                / (raw.width as f32 * raw.height as f32);
            ensure!(progress(Development::Developed(percent)), "Merge cancelled");
            // The tile waits for the GPU until the merge is cancelled; the
            // callback answers that whenever it is called.
            let mut watch_gpu = |waited: Duration| {
                progress(if waited >= SLOW_GPU_WAIT {
                    Development::WaitingForGpu(waited)
                } else {
                    Development::Developed(percent)
                })
            };
            let crop = NativeRect {
                x,
                y,
                width: TILE.min(raw.width - x),
                height: TILE.min(raw.height - y),
            };
            let tile = render_remove_scene_crop(
                DevelopedCropJob {
                    device: device.clone(),
                    queue: queue.clone(),
                    raw: Arc::clone(raw),
                    geometry: GeometryTransform::default(),
                    exposure,
                    masks: MaskStack::default(),
                    remove: RemoveEditState::default(),
                    crop,
                    program_prewarm: program_prewarm.clone(),
                },
                GpuWait::Watched(&mut watch_gpu),
            )?;
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
///
/// A linear DNG was demosaiced by the camera, so its photosites are gone: each pixel's
/// brightest channel stands in for them (its channels are normalized to the file's white
/// level), and the footprint covers the smoothing a camera's demosaic leaves.
pub(super) fn sensor_footprint_peak(raw: &LoadedRaw) -> Vec<f32> {
    /// Pixels of a linear DNG within which the camera's demosaic spread a clipped
    /// photosite, as for a Bayer mosaic.
    const LINEAR_DNG_RADIUS: usize = 3;
    let width = raw.width as usize;
    let height = raw.height as usize;
    let (sensor, radius): (Vec<f32>, usize) = match raw.scene_linear_raster() {
        Some(rgb) if raw.is_camera_linear_raster() => (
            rgb.par_chunks_exact(3)
                .map(|pixel| pixel[0].max(pixel[1]).max(pixel[2]))
                .collect(),
            LINEAR_DNG_RADIUS,
        ),
        _ => (
            raw.raw_pixels
                .par_iter()
                .enumerate()
                .map(|(i, value)| {
                    let c = usize::from(raw.color_indices[i].min(3));
                    let black = raw.black_levels_per_pixel[i];
                    let range = (raw.white_levels[c] - black).max(1.0);
                    (f32::from(*value) - black) / range
                })
                .collect(),
            match raw.cfa_kind {
                CfaKind::Bayer => 3,
                CfaKind::XTrans => 6,
            },
        ),
    };
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
