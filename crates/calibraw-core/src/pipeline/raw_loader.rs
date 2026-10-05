// SPDX-License-Identifier: GPL-3.0-or-later
// Highlight reconstruction includes adaptations from darktable 5.6.0.
// Copyright (C) 2010-2026 darktable developers.
// Copyright (C) 2026 CalibRaw contributors (Rust adaptation).

use super::basicadj::{
    temperature_kelvin_from_offset, temperature_offset_from_kelvin, white_balance_tint_from_offset,
    white_balance_tint_offset, ExposureParams, HighlightReconstructionMethod,
    GLOBAL_TEMPERATURE_LIMIT, GLOBAL_TINT_OFFSET_LIMIT,
};
use super::color_profile::CameraProfile;
use super::geometry::LensGeometryMap;
use super::noise::NoiseProfile;
use super::source_format::RenderedImageFormat;
use super::white_balance_presets::WhiteBalancePreset;
#[cfg(not(libraw_available))]
use anyhow::anyhow;
use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::ops::{Deref, DerefMut, Index};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

mod compact_pixel_map;
mod opposed_chroma;
mod white_balance;
pub use compact_pixel_map::*;
pub use opposed_chroma::*;

#[derive(Debug)]
struct UnsupportedRawFormat {
    detail: String,
}

impl fmt::Display for UnsupportedRawFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for UnsupportedRawFormat {}

/// Reports whether a decoder failure has been explicitly categorized as an
/// unsupported RAW format. Callers should not infer this from display text.
pub fn is_unsupported_raw_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.downcast_ref::<UnsupportedRawFormat>().is_some())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraProfileMode {
    MatrixOnly,
    DcpProfiles,
    #[default]
    Automatic,
}

impl CameraProfileMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::MatrixOnly => "Embedded matrix only",
            Self::DcpProfiles => "Use DCP profiles",
            Self::Automatic => "Automatic",
        }
    }

    pub const fn cache_key(self) -> &'static str {
        match self {
            Self::MatrixOnly => "matrix",
            Self::DcpProfiles => "dcp",
            Self::Automatic => "auto",
        }
    }

    pub const fn prefers_external_dcp(self) -> bool {
        matches!(self, Self::DcpProfiles)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraProfileCandidate {
    pub path: PathBuf,
    pub name: String,
}

/// Sensor RAW (and TIFF) extensions. Rendered JPEG/PNG/HEIF extensions live in
/// [`super::SUPPORTED_RENDERED_EXTENSIONS`]; use
/// [`super::is_supported_image_path`] to ask whether CalibRaw opens a file.
pub const SUPPORTED_RAW_EXTENSIONS: &[&str] = &[
    "3fr", "ari", "arw", "bay", "bmq", "cap", "cine", "cr2", "cr3", "crw", "cs1", "dc2", "dcr",
    "dcs", "dng", "drf", "eip", "erf", "fff", "gpr", "iiq", "k25", "kc2", "kdc", "mdc", "mef",
    "mos", "mrw", "nef", "nrw", "obm", "orf", "pef", "ptx", "pxn", "qtk", "r3d", "raf", "raw",
    "rdc", "rw2", "rwl", "rwz", "sr2", "srf", "srw", "sti", "tif", "tiff", "x3f",
];

pub fn is_supported_raw_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            SUPPORTED_RAW_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

/// An sRGB RGBA8 library preview of any supported photo, RAW or rendered.
#[derive(Clone, Debug)]
pub struct RawThumbnail {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub const MAX_RAW_EDGE: u32 = 32_768;
/// Largest decoded image of any kind. Desktop covers 150 MP medium-format
/// sensors, 200 MP phone captures and 16k panoramas; a scene-linear raster
/// costs 12 bytes per pixel in memory.
#[cfg(target_os = "android")]
pub const MAX_RAW_PIXELS: u64 = 50_000_000;
#[cfg(not(target_os = "android"))]
pub const MAX_RAW_PIXELS: u64 = 200_000_000;
#[cfg(all(libraw_available, target_os = "android"))]
const MAX_RAW_FILE_BYTES: u64 = 2_000_000_000;
#[cfg(all(libraw_available, not(target_os = "android")))]
const MAX_RAW_FILE_BYTES: u64 = 8_000_000_000;
#[cfg(all(libraw_available, target_os = "android"))]
const MAX_SENSOR_PIXELS: u64 = 70_000_000;
#[cfg(all(libraw_available, not(target_os = "android")))]
const MAX_SENSOR_PIXELS: u64 = 160_000_000;
#[cfg(libraw_available)]
const MAX_SENSOR_EDGE: u32 = 40_000;

pub fn validate_raw_dimensions(width: u32, height: u32) -> Result<usize> {
    anyhow::ensure!(width > 0 && height > 0, "RAW dimensions must be non-zero");
    anyhow::ensure!(
        width <= MAX_RAW_EDGE && height <= MAX_RAW_EDGE,
        "RAW dimensions {width}x{height} exceed the {MAX_RAW_EDGE}-pixel edge limit"
    );
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .context("RAW pixel count overflow")?;
    anyhow::ensure!(
        pixels <= MAX_RAW_PIXELS,
        "RAW dimensions {width}x{height} contain {pixels} pixels; the limit is {MAX_RAW_PIXELS}"
    );
    usize::try_from(pixels).context("RAW pixel count does not fit this platform")
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CfaKind {
    #[default]
    Bayer,
    XTrans,
}

#[derive(Clone, Copy, Debug)]
pub struct DngColorEndpoint {
    pub cct: Option<f32>,
    pub color_matrix: [[f32; 3]; 4],
    pub calibration: [[f32; 4]; 4],
    pub forward_matrix: Option<[[f32; 4]; 3]>,
}

#[derive(Clone, Debug)]
pub enum CameraColorModel {
    Dng {
        endpoints: Box<[DngColorEndpoint; 2]>,
        analog_balance: [[f32; 4]; 4],
    },
    Matrix {
        xyz_to_camera: [[f32; 3]; 4],
    },
}

#[derive(Clone, Debug)]
pub struct CameraWhiteBalanceModel {
    pub base_wb: [f32; 4],
    pub cdesc: [u8; 4],
    pub base_cct: f32,
    pub color: CameraColorModel,
}

#[derive(Clone, Debug, Default)]
pub struct CaptureMetadata {
    /// Original EXIF date/time, subsecond and UTC-offset ASCII tags.
    pub exif_dates: Vec<(u16, String)>,
    pub iso_speed: f32,
    pub shutter_seconds: f32,
    pub flash: Option<u16>,
    pub description: String,
    pub artist: String,
}

/// Lightweight metadata available after identifying a RAW, without unpacking
/// its full pixel payload.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RawDisplayMetadata {
    pub aperture: f32,
    pub dimensions: [u32; 2],
    pub iso_speed: f32,
    pub shutter_seconds: f32,
    pub focal_length: f32,
}

#[derive(Clone, Debug)]
pub struct AiDenoisedImage {
    pub width: u32,
    pub height: u32,
    pub rgb16f: Arc<[u16]>,
    pub raw_cfa16: Arc<[u16]>,
}

impl AiDenoisedImage {
    pub fn new(width: u32, height: u32, rgb16f: Vec<u16>) -> Result<Self> {
        let expected = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(3))
            .and_then(|elements| usize::try_from(elements).ok())
            .context("AI-denoise image dimensions overflow")?;
        anyhow::ensure!(
            width > 0 && height > 0 && rgb16f.len() == expected,
            "AI-denoise image has {} values, expected {expected} for {width}x{height}",
            rgb16f.len()
        );
        Ok(Self {
            width,
            height,
            rgb16f: rgb16f.into(),
            raw_cfa16: Arc::from([]),
        })
    }

    pub fn new_bayer_cfa(width: u32, height: u32, raw_cfa16: Vec<u16>) -> Result<Self> {
        let expected = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| usize::try_from(pixels).ok())
            .context("AI-denoise Bayer dimensions overflow")?;
        anyhow::ensure!(
            width > 0 && height > 0 && raw_cfa16.len() == expected,
            "AI-denoise Bayer image has {} values, expected {expected} for {width}x{height}",
            raw_cfa16.len()
        );
        Ok(Self {
            width,
            height,
            rgb16f: Arc::from([]),
            raw_cfa16: raw_cfa16.into(),
        })
    }

    pub fn bayer_cfa(&self) -> Option<&[u16]> {
        (!self.raw_cfa16.is_empty()).then_some(self.raw_cfa16.as_ref())
    }

    pub fn camera_rgb16f(&self) -> Option<&[u16]> {
        (!self.rgb16f.is_empty()).then_some(self.rgb16f.as_ref())
    }

    pub fn payload(&self) -> &[u16] {
        if let Some(raw_cfa) = self.bayer_cfa() {
            raw_cfa
        } else {
            self.rgb16f.as_ref()
        }
    }

    pub fn is_valid_for(&self, width: u32, height: u32) -> bool {
        if self.width != width || self.height != height {
            return false;
        }
        let Some(pixels) = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| usize::try_from(pixels).ok())
        else {
            return false;
        };
        (self.raw_cfa16.len() == pixels && self.rgb16f.is_empty())
            || (self.rgb16f.len() == pixels.saturating_mul(3) && self.raw_cfa16.is_empty())
    }
}

/// A decoded source photo ready for the develop pipeline. Despite the name it
/// holds either sensor data (`raw_pixels` + CFA metadata) or, for rendered
/// TIFF/JPEG/PNG/HEIF photos, a scene-linear Rec.2020 raster; see
/// [`LoadedRaw::is_display_referred_raster`].
#[derive(Clone, Debug)]
pub struct LoadedRaw {
    pub width: u32,
    pub height: u32,
    pub camera_make: String,
    pub camera_model: String,
    pub lens_make: String,
    pub lens_model: String,
    pub focal_length: f32,
    pub aperture: f32,
    pub focus_distance: f32,
    pub capture_metadata: CaptureMetadata,
    pub cfa_kind: CfaKind,
    pub raw_pixels: Vec<u16>,
    pub scene_linear_raster: Option<Arc<[f32]>>,
    pub color_indices: CompactPixelMap<u8>,
    pub wb_coeffs: [f32; 4],
    pub cam_to_srgb: [[f32; 4]; 3],
    pub black_levels: [f32; 4],
    pub black_levels_per_pixel: CompactPixelMap<f32>,
    pub white_levels: [f32; 4],
    pub noise_profile: NoiseProfile,
    pub camera_profile: CameraProfile,
    pub camera_profile_source: Option<PathBuf>,
    pub available_camera_profiles: Vec<CameraProfileCandidate>,
    pub white_balance_model: Option<CameraWhiteBalanceModel>,
    pub lens_geometry: Option<Arc<LensGeometryMap>>,
    pub ai_denoised: Arc<RwLock<Option<AiDenoisedImage>>>,
    pub opposed_chroma_cache: OpposedChromaCache,
    /// Runtime identity of the full sensor source used to estimate opposed-highlight chroma.
    /// Crops, proxies, and tiles retain this token so shared cache entries cannot cross images.
    pub opposed_chroma_source_identity: Arc<u8>,
    /// Only full-source RAWs may populate the shared opposed-chroma cache. Derived views may
    /// consume it, but fall back to a local estimate on a miss rather than poisoning it.
    pub opposed_chroma_reference_source: bool,
}

impl LoadedRaw {
    /// Builds a sensor-mosaic view of the same capture with new pixels (a crop, proxy, tile or
    /// corrected mosaic). Capture metadata, colour calibration and the noise profile are copied
    /// from `self`. The view has no scene-linear raster, no lens geometry and a fresh, empty
    /// AI-denoise slot. It shares `self`'s opposed-chroma cache and source identity as a
    /// non-reference consumer. Callers override per-view fields with struct update syntax.
    pub fn derive_with(
        &self,
        width: u32,
        height: u32,
        raw_pixels: Vec<u16>,
        color_indices: CompactPixelMap<u8>,
        black_levels_per_pixel: CompactPixelMap<f32>,
    ) -> Self {
        Self {
            width,
            height,
            camera_make: self.camera_make.clone(),
            camera_model: self.camera_model.clone(),
            lens_make: self.lens_make.clone(),
            lens_model: self.lens_model.clone(),
            focal_length: self.focal_length,
            aperture: self.aperture,
            focus_distance: self.focus_distance,
            capture_metadata: self.capture_metadata.clone(),
            cfa_kind: self.cfa_kind,
            raw_pixels,
            scene_linear_raster: None,
            color_indices,
            wb_coeffs: self.wb_coeffs,
            cam_to_srgb: self.cam_to_srgb,
            black_levels: self.black_levels,
            black_levels_per_pixel,
            white_levels: self.white_levels,
            noise_profile: self.noise_profile,
            camera_profile: self.camera_profile.clone(),
            camera_profile_source: self.camera_profile_source.clone(),
            available_camera_profiles: self.available_camera_profiles.clone(),
            white_balance_model: self.white_balance_model.clone(),
            lens_geometry: None,
            ai_denoised: Arc::new(RwLock::new(None)),
            opposed_chroma_cache: Arc::clone(&self.opposed_chroma_cache),
            opposed_chroma_source_identity: Arc::clone(&self.opposed_chroma_source_identity),
            opposed_chroma_reference_source: false,
        }
    }

    pub fn from_scene_linear_rec2020(width: u32, height: u32, rgb: Vec<f32>) -> Result<Self> {
        let pixels = validate_raw_dimensions(width, height)?;
        let expected = pixels
            .checked_mul(3)
            .context("scene-linear raster element count overflow")?;
        anyhow::ensure!(
            rgb.len() == expected,
            "scene-linear raster has {} values, expected {expected} for {width}x{height}",
            rgb.len()
        );
        anyhow::ensure!(
            rgb.iter().all(|value| value.is_finite()),
            "scene-linear raster contains NaN or infinity"
        );
        Ok(Self {
            width,
            height,
            camera_make: String::new(),
            camera_model: String::new(),
            lens_make: String::new(),
            lens_model: String::new(),
            focal_length: 0.0,
            aperture: 0.0,
            focus_distance: 0.0,
            capture_metadata: CaptureMetadata::default(),
            cfa_kind: CfaKind::Bayer,
            raw_pixels: Vec::new(),
            scene_linear_raster: Some(rgb.into()),
            color_indices: CompactPixelMap::repeating(width, height, 1, 1, vec![1]),
            wb_coeffs: [1.0; 4],
            cam_to_srgb: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
            black_levels: [0.0; 4],
            black_levels_per_pixel: CompactPixelMap::repeating(width, height, 1, 1, vec![0.0]),
            white_levels: [1.0; 4],
            noise_profile: NoiseProfile::default(),
            camera_profile: CameraProfile::default(),
            camera_profile_source: None,
            available_camera_profiles: Vec::new(),
            white_balance_model: None,
            lens_geometry: None,
            ai_denoised: Arc::new(RwLock::new(None)),
            opposed_chroma_cache: Default::default(),
            opposed_chroma_source_identity: Default::default(),
            opposed_chroma_reference_source: true,
        })
    }

    pub fn is_pre_demosaiced_raster(&self) -> bool {
        self.raw_pixels.is_empty()
            && self.scene_linear_raster.as_ref().is_some_and(|rgb| {
                rgb.len()
                    == (self.width as usize)
                        .saturating_mul(self.height as usize)
                        .saturating_mul(3)
            })
    }

    /// A raster decoded from camera-space LinearRaw samples. Unlike normal
    /// scene rasters, these pixels still need the camera white balance and
    /// camera-to-working transform applied by the GPU.
    pub fn is_camera_linear_raster(&self) -> bool {
        self.is_pre_demosaiced_raster() && self.white_balance_model.is_some()
    }

    /// A raster decoded from a rendered photo (JPEG, PNG, HEIF or rendered
    /// TIFF) whose tones and colours were already developed.
    pub fn is_display_referred_raster(&self) -> bool {
        self.is_pre_demosaiced_raster() && !self.is_camera_linear_raster()
    }

    pub fn scene_linear_raster(&self) -> Option<&[f32]> {
        self.scene_linear_raster.as_deref()
    }

    pub fn ai_denoised_image(&self) -> Option<AiDenoisedImage> {
        self.ai_denoised
            .read()
            .ok()
            .and_then(|image| image.as_ref().cloned())
            .filter(|image| image.is_valid_for(self.width, self.height))
    }

    pub fn set_ai_denoised_image(&self, image: AiDenoisedImage) -> Result<()> {
        anyhow::ensure!(
            image.is_valid_for(self.width, self.height),
            "AI-denoise result {}x{} does not match RAW {}x{}",
            image.width,
            image.height,
            self.width,
            self.height
        );
        anyhow::ensure!(
            matches!(self.cfa_kind, CfaKind::Bayer) == image.bayer_cfa().is_some(),
            "AI-denoise payload type does not match the RAW CFA"
        );
        {
            let mut cached = self
                .ai_denoised
                .write()
                .map_err(|_| anyhow::anyhow!("AI-denoise cache lock was poisoned"))?;
            *cached = Some(image);
        }
        if let Ok(mut chroma) = self.opposed_chroma_cache.write() {
            chroma.retain(|key, _| !key.use_ai_cfa);
            chroma.prepared.retain(|key, _| !key.use_ai_cfa);
        }
        Ok(())
    }

    pub fn clear_ai_denoised_image(&self) {
        if let Ok(mut cached) = self.ai_denoised.write() {
            *cached = None;
        }
        if let Ok(mut chroma) = self.opposed_chroma_cache.write() {
            chroma.retain(|key, _| !key.use_ai_cfa);
            chroma.prepared.retain(|key, _| !key.use_ai_cfa);
        }
    }

    pub fn iso_speed(&self) -> f32 {
        self.capture_metadata.iso_speed
    }

    pub fn apply_adaptive_detail_defaults(&self, exposure: &mut ExposureParams) {
        if self.is_display_referred_raster() {
            // Rendered photos were already denoised and sharpened when they
            // were developed; start them untouched, as other editors do.
            exposure.luminance_denoise = 0.0;
            exposure.chroma_denoise = 0.0;
            exposure.sharpen_amount = 0.0;
            return;
        }
        let defaults = self.noise_profile.adaptive_detail_defaults(
            self.capture_metadata.iso_speed,
            self.white_levels,
            self.wb_coeffs,
        );
        exposure.luminance_denoise = defaults.luminance_denoise;
        exposure.chroma_denoise = defaults.chroma_denoise;
        exposure.denoise_detail = defaults.denoise_detail;
        exposure.denoise_quality = defaults.denoise_quality;
    }
}

/// Display-referred sources that bypass the sensor decoders and enter the
/// pipeline as scene-linear Rec.2020 rasters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RasterSource {
    Tiff,
    Rendered(RenderedImageFormat),
}

impl RasterSource {
    fn detect(path: &Path) -> Result<Option<Self>> {
        if let Some(format) = RenderedImageFormat::detect(path)? {
            return Ok(Some(Self::Rendered(format)));
        }
        let raster_tiff = super::tiff_loader::is_tiff_path(path)
            && super::tiff_loader::inspect_tiff_container(path)?
                == super::tiff_loader::TiffContainerKind::Raster;
        Ok(raster_tiff.then_some(Self::Tiff))
    }

    fn load(self, path: &Path) -> Result<LoadedRaw> {
        match self {
            Self::Tiff => super::tiff_loader::load_raster_tiff(path),
            Self::Rendered(format) => super::rendered_loader::load_rendered_image(path, format),
        }
    }

    fn thumbnail(self, path: &Path, maximum_edge: u32) -> Result<RawThumbnail> {
        match self {
            Self::Tiff => super::tiff_loader::load_raster_tiff_thumbnail(path, maximum_edge),
            Self::Rendered(format) => {
                super::rendered_loader::load_rendered_thumbnail(path, format, maximum_edge)
            }
        }
    }

    fn display_metadata(self, path: &Path) -> Result<RawDisplayMetadata> {
        match self {
            Self::Tiff => Ok(RawDisplayMetadata {
                dimensions: super::tiff_loader::load_raster_tiff_dimensions(path)?,
                ..Default::default()
            }),
            Self::Rendered(format) => {
                super::rendered_loader::load_rendered_display_metadata(path, format)
            }
        }
    }
}

/// Decodes any supported photo: camera RAW, TIFF, JPEG, PNG or HEIF.
#[cfg(not(libraw_available))]
pub fn load_raw_file(path: &Path) -> Result<LoadedRaw> {
    if let Some(raster) = RasterSource::detect(path)? {
        return raster.load(path);
    }
    Err(anyhow!(
        "this build was compiled without LibRaw. Install LibRaw and make libraw.pc visible through PKG_CONFIG_PATH, then rebuild CalibRaw."
    ))
}

#[cfg(not(libraw_available))]
pub fn load_raw_embedded_thumbnail(path: &Path, maximum_edge: u32) -> Result<RawThumbnail> {
    if let Some(raster) = RasterSource::detect(path)? {
        return raster.thumbnail(path, maximum_edge);
    }
    Err(anyhow!(
        "this build was compiled without LibRaw, so embedded RAW thumbnails are unavailable"
    ))
}

#[cfg(not(libraw_available))]
pub fn load_raw_thumbnail(path: &Path, maximum_edge: u32) -> Result<RawThumbnail> {
    if let Some(raster) = RasterSource::detect(path)? {
        return raster.thumbnail(path, maximum_edge);
    }
    Err(anyhow!(
        "this build was compiled without LibRaw, so RAW thumbnails are unavailable"
    ))
}

#[cfg(not(libraw_available))]
pub fn load_raw_display_dimensions(path: &Path) -> Result<[u32; 2]> {
    load_raw_display_metadata(path).map(|metadata| metadata.dimensions)
}

#[cfg(not(libraw_available))]
pub fn load_raw_display_metadata(path: &Path) -> Result<RawDisplayMetadata> {
    if let Some(raster) = RasterSource::detect(path)? {
        return raster.display_metadata(path);
    }
    Err(anyhow!(
        "this build was compiled without LibRaw, so RAW display metadata is unavailable"
    ))
}

#[cfg(not(libraw_available))]
pub fn load_raw_file_with_dcp(path: &Path, _profile_path: &Path) -> Result<LoadedRaw> {
    load_raw_file(path)
}

#[cfg(not(libraw_available))]
pub fn load_raw_file_with_profile_config(
    path: &Path,
    _mode: CameraProfileMode,
    _profile_folder: Option<&Path>,
) -> Result<LoadedRaw> {
    load_raw_file(path)
}

#[cfg(not(libraw_available))]
pub fn load_raw_file_with_profile_selection(
    path: &Path,
    _mode: CameraProfileMode,
    _profile_folder: Option<&Path>,
    _selected_profile: Option<&Path>,
) -> Result<LoadedRaw> {
    load_raw_file(path)
}

fn path_is_dng(path: &Path) -> bool {
    match path.extension() {
        Some(extension) => extension.eq_ignore_ascii_case("dng"),
        // Android opens documents as /proc/self/fd/<number>. Those paths lose
        // the display name, so identify DNGs from their container metadata.
        None => super::tiff_loader::has_dng_version(path).unwrap_or(false),
    }
}

#[cfg(libraw_available)]
fn try_rawler_then_libraw<T>(
    path: &Path,
    operation: &str,
    rawler: impl FnOnce() -> Result<T>,
    libraw: impl FnOnce() -> Result<T>,
) -> Result<T> {
    match rawler() {
        Ok(value) => Ok(value),
        Err(rawler_error) => {
            let rawler_detail = format!("{rawler_error:#}");
            log::warn!(
                "Rawler {operation} failed for {}; falling back to LibRaw: {rawler_detail}",
                path.display()
            );
            crate::diagnostics::record(format!(
                "Rawler {operation} failed; retrying through LibRaw: {rawler_detail}"
            ));
            libraw().with_context(|| {
                format!(
                    "Rawler {operation} failed first ({rawler_detail}); LibRaw fallback also failed"
                )
            })
        }
    }
}

/// Decodes any supported photo: camera RAW, TIFF, JPEG, PNG or HEIF.
#[cfg(libraw_available)]
pub fn load_raw_file(path: &Path) -> Result<LoadedRaw> {
    load_raw_file_with_profile_config(path, CameraProfileMode::Automatic, None)
}

#[cfg(libraw_available)]
pub fn load_raw_file_with_profile_config(
    path: &Path,
    mode: CameraProfileMode,
    profile_folder: Option<&Path>,
) -> Result<LoadedRaw> {
    load_raw_file_with_profile_selection(path, mode, profile_folder, None)
}

#[cfg(libraw_available)]
pub fn load_raw_file_with_profile_selection(
    path: &Path,
    mode: CameraProfileMode,
    profile_folder: Option<&Path>,
    selected_profile: Option<&Path>,
) -> Result<LoadedRaw> {
    if let Some(raster) = RasterSource::detect(path)? {
        raster.load(path)
    } else if path_is_dng(path) {
        try_rawler_then_libraw(
            path,
            "RAW decode",
            || {
                rawler_loader::load_raw_file_with_profile_selection(
                    path,
                    mode,
                    profile_folder,
                    selected_profile,
                )
            },
            || {
                libraw_loader::load_raw_file_with_profile_selection(
                    path,
                    mode,
                    profile_folder,
                    selected_profile,
                )
            },
        )
    } else {
        libraw_loader::load_raw_file_with_profile_selection(
            path,
            mode,
            profile_folder,
            selected_profile,
        )
    }
}

#[cfg(libraw_available)]
pub fn load_raw_file_with_dcp(path: &Path, profile_path: &Path) -> Result<LoadedRaw> {
    if let Some(raster) = RasterSource::detect(path)? {
        raster.load(path)
    } else if path_is_dng(path) {
        try_rawler_then_libraw(
            path,
            "DCP-backed RAW decode",
            || rawler_loader::load_raw_file_with_dcp(path, profile_path),
            || libraw_loader::load_raw_file_with_dcp(path, profile_path),
        )
    } else {
        libraw_loader::load_raw_file_with_dcp(path, profile_path)
    }
}

#[cfg(libraw_available)]
pub fn load_raw_embedded_thumbnail(path: &Path, maximum_edge: u32) -> Result<RawThumbnail> {
    if let Some(raster) = RasterSource::detect(path)? {
        raster.thumbnail(path, maximum_edge)
    } else if path_is_dng(path) {
        try_rawler_then_libraw(
            path,
            "embedded thumbnail decode",
            || rawler_loader::load_raw_embedded_thumbnail(path, maximum_edge),
            || libraw_loader::load_raw_embedded_thumbnail(path, maximum_edge),
        )
    } else {
        libraw_loader::load_raw_embedded_thumbnail(path, maximum_edge)
    }
}

#[cfg(libraw_available)]
pub fn load_raw_thumbnail(path: &Path, maximum_edge: u32) -> Result<RawThumbnail> {
    if let Some(raster) = RasterSource::detect(path)? {
        raster.thumbnail(path, maximum_edge)
    } else if path_is_dng(path) {
        try_rawler_then_libraw(
            path,
            "thumbnail decode",
            || rawler_loader::load_raw_thumbnail(path, maximum_edge),
            || libraw_loader::load_raw_thumbnail(path, maximum_edge),
        )
    } else {
        libraw_loader::load_raw_thumbnail(path, maximum_edge)
    }
}

#[cfg(libraw_available)]
pub fn load_raw_display_dimensions(path: &Path) -> Result<[u32; 2]> {
    load_raw_display_metadata(path).map(|metadata| metadata.dimensions)
}

#[cfg(libraw_available)]
pub fn load_raw_display_metadata(path: &Path) -> Result<RawDisplayMetadata> {
    if let Some(raster) = RasterSource::detect(path)? {
        raster.display_metadata(path)
    } else if path_is_dng(path) {
        try_rawler_then_libraw(
            path,
            "display metadata decode",
            || rawler_loader::load_raw_display_metadata(path),
            || libraw_loader::load_raw_display_metadata(path),
        )
    } else {
        libraw_loader::load_raw_display_metadata(path)
    }
}

#[cfg(libraw_available)]
pub fn invalidate_dcp_profile_index() {
    libraw_loader::invalidate_dcp_profile_index();
}

#[cfg(not(libraw_available))]
pub fn invalidate_dcp_profile_index() {}

#[cfg(libraw_available)]
pub fn prewarm_dcp_profile_index(folder: &Path) {
    libraw_loader::prewarm_dcp_profile_index(folder);
}

#[cfg(not(libraw_available))]
pub fn prewarm_dcp_profile_index(_folder: &Path) {}

#[cfg(libraw_available)]
mod libraw_loader;
#[cfg(libraw_available)]
mod rawler_loader;

#[cfg(test)]
mod routing_tests {
    use super::{is_unsupported_raw_error, path_is_dng, UnsupportedRawFormat};
    use std::path::Path;

    #[test]
    fn dng_extension_routes_case_insensitively() {
        assert!(path_is_dng(Path::new("phone.dng")));
        assert!(path_is_dng(Path::new("phone.DNG")));
        assert!(!path_is_dng(Path::new("camera.cr3")));
        assert!(!path_is_dng(Path::new("camera.nef")));
    }

    #[test]
    fn extensionless_non_dng_inputs_keep_libraw_routing() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"not a TIFF or DNG").unwrap();
        assert!(!path_is_dng(file.path()));
        // Valid TIFF container with no DNGVersion tag.
        let mut bytes = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
        bytes.extend_from_slice(&[0u8; 6]);
        std::fs::write(file.path(), bytes).unwrap();
        assert!(!path_is_dng(file.path()));
    }

    #[test]
    fn unsupported_format_classification_uses_typed_error_chain() {
        let error = anyhow::Error::new(UnsupportedRawFormat {
            detail: "unsupported test format".to_owned(),
        })
        .context("decoder fallback failed");
        assert!(is_unsupported_raw_error(&error));

        let untyped = anyhow::anyhow!("this message says unsupported but has no category");
        assert!(!is_unsupported_raw_error(&untyped));
    }

    #[cfg(libraw_available)]
    #[test]
    fn dng_rawler_failure_falls_back_to_libraw() {
        use std::cell::Cell;

        let fallback_called = Cell::new(false);
        let value = super::try_rawler_then_libraw(
            Path::new("fallback.dng"),
            "test decode",
            || Err::<u32, _>(anyhow::anyhow!("Rawler rejected this DNG")),
            || {
                fallback_called.set(true);
                Ok(42)
            },
        )
        .expect("LibRaw fallback should recover the decode");

        assert_eq!(value, 42);
        assert!(fallback_called.get());
    }

    #[cfg(libraw_available)]
    #[test]
    fn dng_rawler_success_does_not_call_libraw() {
        use std::cell::Cell;

        let fallback_called = Cell::new(false);
        let value = super::try_rawler_then_libraw(
            Path::new("rawler.dng"),
            "test decode",
            || Ok(7_u32),
            || {
                fallback_called.set(true);
                Ok(42)
            },
        )
        .expect("Rawler success should be returned directly");

        assert_eq!(value, 7);
        assert!(!fallback_called.get());
    }

    #[cfg(libraw_available)]
    #[test]
    fn dng_double_failure_preserves_rawler_context() {
        let error = super::try_rawler_then_libraw::<u32>(
            Path::new("broken.dng"),
            "test decode",
            || Err(anyhow::anyhow!("Rawler reason")),
            || Err(anyhow::anyhow!("LibRaw reason")),
        )
        .expect_err("both decoders should fail");
        let message = format!("{error:#}");

        assert!(message.contains("Rawler reason"));
        assert!(message.contains("LibRaw reason"));
    }
}

#[cfg(test)]
mod extension_tests {
    use super::is_supported_raw_path;
    use std::path::Path;

    #[test]
    fn tiff_extensions_are_supported_case_insensitively() {
        for name in ["photo.tif", "photo.TIF", "photo.tiff", "photo.TIFF"] {
            assert!(is_supported_raw_path(Path::new(name)), "{name}");
        }
    }
}

#[cfg(all(test, libraw_available))]
mod tests {
    use super::{
        temperature_offset_from_kelvin, AiDenoisedImage, CameraColorModel, CameraProfile,
        CameraProfileMode, CameraWhiteBalanceModel, CfaKind, CompactPixelMap, ExposureParams,
        LoadedRaw, GLOBAL_TEMPERATURE_LIMIT,
    };

    #[test]
    fn parallel_highlight_candidates_match_serial_for_dense_and_periodic_maps() {
        // Cover CFA tile sizes, partial 3-pixel cells, dense black maps, threshold
        // boundaries and both empty and heavily clipped scenes.
        for (width, height, tile) in [(2, 2, 2), (26, 24, 2), (97, 98, 2), (101, 103, 6)] {
            for dense in [false, true] {
                for clipped in [false, true] {
                    let mut raw = colored_opposed_test_raw();
                    raw.width = width;
                    raw.height = height;
                    let colors: Vec<u8> = (0..tile * tile)
                        .map(|i| ((i * 7 + i / tile) % 4) as u8)
                        .collect();
                    raw.color_indices =
                        CompactPixelMap::repeating(width, height, tile, tile, colors);
                    raw.black_levels_per_pixel = CompactPixelMap::repeating(
                        width,
                        height,
                        2,
                        2,
                        vec![0.0, 128.0, 256.0, 512.0],
                    );
                    if dense {
                        raw.color_indices = CompactPixelMap::dense(
                            width,
                            height,
                            raw.color_indices.iter().copied().collect(),
                        );
                        raw.black_levels_per_pixel = CompactPixelMap::dense(
                            width,
                            height,
                            raw.black_levels_per_pixel.iter().copied().collect(),
                        );
                    }
                    raw.raw_pixels = (0..width * height)
                        .map(|i| {
                            if clipped && i % 19 < 3 {
                                10000
                            } else {
                                (i * 31 % 9500) as u16
                            }
                        })
                        .collect();
                    for (black, clip) in [(0.0, 1.0), (0.025, 0.93), (-0.025, 0.8)] {
                        let expected = raw.prepare_opposed_chroma_candidates_serial(
                            black,
                            clip,
                            &raw.raw_pixels,
                        );
                        let actual =
                            raw.prepare_opposed_chroma_candidates(black, clip, &raw.raw_pixels);
                        assert_eq!(actual, expected, "{width}x{height}, dense={dense}, clipped={clipped}, black={black}, clip={clip}");
                    }
                }
            }
        }
    }

    #[test]
    fn compact_map_fast_paths_match_expanded_tiles() {
        for (width, height, tw, th) in [
            (17, 13, 1, 1),
            (17, 13, 2, 2),
            (17, 13, 6, 6),
            (17, 13, 17, 13),
        ] {
            let values: Vec<_> = (0..tw * th).collect();
            let map = CompactPixelMap::repeating(width, height, tw, th, values.clone());
            for y in 0..height {
                for x in 0..width {
                    assert_eq!(
                        map[(y * width + x) as usize],
                        values[((y % th) * tw + x % tw) as usize]
                    );
                }
            }
            assert_eq!(map.get((width * height) as usize), None);
        }
    }

    #[test]
    fn automatic_profile_mode_defaults_to_the_embedded_matrix() {
        assert!(!CameraProfileMode::Automatic.prefers_external_dcp());
        assert!(!CameraProfileMode::MatrixOnly.prefers_external_dcp());
        assert!(CameraProfileMode::DcpProfiles.prefers_external_dcp());
    }

    fn raw_with_white_balance_model() -> LoadedRaw {
        LoadedRaw {
            width: 1,
            height: 1,
            camera_make: "Test".to_owned(),
            camera_model: "Matrix".to_owned(),
            lens_make: String::new(),
            lens_model: String::new(),
            focal_length: 0.0,
            aperture: 0.0,
            focus_distance: 0.0,
            capture_metadata: Default::default(),
            cfa_kind: CfaKind::Bayer,
            raw_pixels: vec![0],
            scene_linear_raster: None,
            color_indices: CompactPixelMap::dense(1, 1, vec![0]),
            wb_coeffs: [2.0, 1.0, 1.5, 1.0],
            cam_to_srgb: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
            black_levels: [0.0; 4],
            black_levels_per_pixel: CompactPixelMap::dense(1, 1, vec![0.0]),
            white_levels: [1.0; 4],
            noise_profile: crate::pipeline::NoiseProfile::default(),
            camera_profile: CameraProfile::default(),
            camera_profile_source: None,
            available_camera_profiles: Vec::new(),
            white_balance_model: Some(CameraWhiteBalanceModel {
                base_wb: [2.0, 1.0, 1.5, 1.0],
                cdesc: *b"RGBG",
                base_cct: 5_000.0,
                color: CameraColorModel::Matrix {
                    xyz_to_camera: [
                        [1.0, 0.0, 0.0],
                        [0.0, 1.0, 0.0],
                        [0.0, 0.0, 1.0],
                        [0.0, 1.0, 0.0],
                    ],
                },
            }),
            lens_geometry: None,
            ai_denoised: std::sync::Arc::new(std::sync::RwLock::new(None)),
            opposed_chroma_cache: Default::default(),
            opposed_chroma_source_identity: Default::default(),
            opposed_chroma_reference_source: true,
        }
    }

    fn colored_opposed_test_raw() -> LoadedRaw {
        const WIDTH: u32 = 96;
        const HEIGHT: u32 = 96;
        const WHITE: f32 = 10_000.0;
        let mut raw = raw_with_white_balance_model();
        raw.width = WIDTH;
        raw.height = HEIGHT;
        raw.white_levels = [WHITE; 4];
        raw.black_levels_per_pixel = CompactPixelMap::repeating(WIDTH, HEIGHT, 1, 1, vec![0.0]);
        let mut colors = Vec::with_capacity((WIDTH * HEIGHT) as usize);
        let mut pixels = Vec::with_capacity((WIDTH * HEIGHT) as usize);
        for row in 0..HEIGHT {
            for col in 0..WIDTH {
                let physical = match (col % 2, row % 2) {
                    (0, 0) => 0,
                    (1, 0) => 1,
                    (0, 1) => 3,
                    _ => 2,
                };
                colors.push(physical);
                let logical = usize::from(if physical == 3 { 1 } else { physical });
                let mut value = [0.80_f32, 0.60, 0.40][logical];
                if (30..66).contains(&col)
                    && (30..66).contains(&row)
                    && (logical == 0 || logical == 2)
                {
                    value = 1.0;
                }
                pixels.push((value * WHITE).round() as u16);
            }
        }
        raw.raw_pixels = pixels;
        raw.color_indices = CompactPixelMap::dense(WIDTH, HEIGHT, colors);
        raw.opposed_chroma_cache = Default::default();
        raw.opposed_chroma_source_identity = Default::default();
        raw.opposed_chroma_reference_source = true;
        raw
    }

    #[test]
    fn opposed_chroma_cache_identity_covers_source_wb_black_clip_and_ai_selection() {
        let raw = colored_opposed_test_raw();
        let wb_a = [1.0, 1.0, 1.0, 1.0];
        let wb_b = [1.35, 1.0, 0.75, 1.0];

        raw.inpaint_opposed_chroma(0.0, 1.0, false, wb_a);
        {
            let cache = raw.opposed_chroma_cache.read().unwrap();
            assert_eq!(cache.len(), 1);
            assert_eq!(cache.prepared.len(), 1);
        }
        raw.inpaint_opposed_chroma(0.0, 1.0, false, wb_a);
        assert_eq!(raw.opposed_chroma_cache.read().unwrap().len(), 1);

        raw.inpaint_opposed_chroma(0.0, 1.0, false, wb_b);
        {
            let cache = raw.opposed_chroma_cache.read().unwrap();
            assert_eq!(cache.len(), 2);
            assert_eq!(
                cache.prepared.len(),
                1,
                "WB changes must reuse the full-image clipping analysis"
            );
        }
        raw.inpaint_opposed_chroma(0.025, 1.0, false, wb_b);
        raw.inpaint_opposed_chroma(0.025, 0.93, false, wb_b);
        {
            let cache = raw.opposed_chroma_cache.read().unwrap();
            assert_eq!(cache.len(), 4);
            assert_eq!(cache.prepared.len(), 3);
        }

        let mut ai_pixels = raw.raw_pixels.clone();
        for value in &mut ai_pixels {
            *value = value.saturating_sub(137);
        }
        raw.set_ai_denoised_image(
            AiDenoisedImage::new_bayer_cfa(raw.width, raw.height, ai_pixels).unwrap(),
        )
        .unwrap();
        raw.inpaint_opposed_chroma(0.025, 0.93, true, wb_b);
        assert_eq!(raw.opposed_chroma_cache.read().unwrap().len(), 5);

        let mut other = raw.clone();
        other.opposed_chroma_cache = std::sync::Arc::clone(&raw.opposed_chroma_cache);
        other.opposed_chroma_source_identity = Default::default();
        other.opposed_chroma_reference_source = true;
        other.inpaint_opposed_chroma(0.0, 1.0, false, wb_a);
        assert_eq!(raw.opposed_chroma_cache.read().unwrap().len(), 6);
    }

    #[test]
    fn opposed_chroma_for_exposure_uses_the_adjusted_white_balance() {
        let mut raw = colored_opposed_test_raw();
        let base_temperature = raw.as_shot_temperature_kelvin().unwrap();
        let exposure = ExposureParams {
            temperature: temperature_offset_from_kelvin(base_temperature, 8_000.0),
            tint: 0.2,
            ..Default::default()
        };

        let adjusted = raw
            .adjusted_white_balance_and_camera_transform(exposure.temperature, exposure.tint)
            .0;
        assert_ne!(adjusted, raw.wb_coeffs);

        raw.opposed_chroma_cache = Default::default();
        let expected = raw.inpaint_opposed_chroma(
            exposure.black_point,
            exposure.highlight_clip,
            exposure.ai_denoise_enabled,
            adjusted,
        );
        raw.opposed_chroma_cache = Default::default();
        let actual = raw.inpaint_opposed_chroma_for_exposure(&exposure);
        assert_eq!(actual, expected);
    }

    #[test]
    fn extended_temperature_range_reaches_beyond_the_old_hundred_mired_clamp() {
        let raw = raw_with_white_balance_model();
        let base = raw.as_shot_temperature_kelvin().unwrap();
        let at_8000 = temperature_offset_from_kelvin(base, 8_000.0);
        let at_25000 = temperature_offset_from_kelvin(base, 25_000.0);
        let at_3200 = temperature_offset_from_kelvin(base, 3_200.0);
        let at_1901 = temperature_offset_from_kelvin(base, 1_901.0);

        assert_ne!(
            raw.adjusted_white_balance_and_camera_transform(at_8000, 0.0)
                .0,
            raw.adjusted_white_balance_and_camera_transform(at_25000, 0.0)
                .0,
        );
        assert_ne!(
            raw.adjusted_white_balance_and_camera_transform(at_3200, 0.0)
                .0,
            raw.adjusted_white_balance_and_camera_transform(at_1901, 0.0)
                .0,
        );
        assert_eq!(
            raw.adjusted_white_balance_and_camera_transform(at_25000, 0.0)
                .0,
            raw.adjusted_white_balance_and_camera_transform(GLOBAL_TEMPERATURE_LIMIT + 50.0, 0.0,)
                .0
        );
    }

    #[test]
    fn white_balance_display_presets_and_renderer_share_camera_limits() {
        let mut raw = raw_with_white_balance_model();
        let model = raw.white_balance_model.as_mut().unwrap();
        model.color = CameraColorModel::Matrix {
            xyz_to_camera: [
                [0.65, -0.2, -0.1],
                [-0.2, 1.1, 0.1],
                [0.05, -0.3, 0.9],
                [0.0; 3],
            ],
        };
        model.base_wb =
            super::libraw_loader::temperature_tint_to_coefficients(model, 5_000.0, 1.0).unwrap();
        raw.wb_coeffs = model.base_wb;
        let (base_temperature, base_tint) = raw.as_shot_white_balance().unwrap();
        let temperature_offset = temperature_offset_from_kelvin(base_temperature, 5_000.0);
        let legacy_tint_offset = crate::pipeline::white_balance_tint_offset(base_tint, 0.135);
        let displayed = raw
            .white_balance_temperature_tint(temperature_offset, legacy_tint_offset)
            .unwrap();
        assert!(displayed.1 > 0.135);
        let saved = raw
            .white_balance_offsets_from_temperature_tint(5_000.0, 0.135)
            .unwrap();
        let from_legacy =
            raw.adjusted_white_balance_and_camera_transform(temperature_offset, legacy_tint_offset);
        let from_saved = raw.adjusted_white_balance_and_camera_transform(saved.0, saved.1);
        assert_eq!(from_legacy, from_saved);
        assert_ne!(from_legacy.0, raw.wb_coeffs);
        assert_eq!(from_legacy.1, raw.cam_to_srgb);
        assert_eq!(
            raw.adjusted_white_balance_and_camera_transform(0.0, 0.0).0,
            raw.wb_coeffs
        );
        let expected = super::libraw_loader::temperature_tint_to_coefficients(
            raw.white_balance_model.as_ref().unwrap(),
            displayed.0,
            displayed.1,
        )
        .unwrap();
        assert_eq!(from_legacy.0, expected);
    }

    #[test]
    fn image_area_picker_recovers_camera_coefficients_from_raw_cfa_samples() {
        const WIDTH: u32 = 24;
        const HEIGHT: u32 = 24;
        const WHITE: f32 = 10_000.0;
        let mut raw = raw_with_white_balance_model();
        let desired_offsets = raw
            .white_balance_offsets_from_temperature_tint(6_000.0, 1.0)
            .unwrap();
        let desired = raw
            .adjusted_white_balance_and_camera_transform(desired_offsets.0, desired_offsets.1)
            .0;

        raw.width = WIDTH;
        raw.height = HEIGHT;
        raw.white_levels = [WHITE; 4];
        raw.black_levels_per_pixel = CompactPixelMap::repeating(WIDTH, HEIGHT, 1, 1, vec![0.0]);
        let mut colors = Vec::with_capacity((WIDTH * HEIGHT) as usize);
        let mut pixels = Vec::with_capacity((WIDTH * HEIGHT) as usize);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let channel = match (x % 2, y % 2) {
                    (0, 0) => 0,
                    (1, 0) => 1,
                    (0, 1) => 3,
                    _ => 2,
                };
                colors.push(channel);
                pixels.push(((0.35 / desired[channel as usize]) * WHITE).round() as u16);
            }
        }
        raw.color_indices = CompactPixelMap::dense(WIDTH, HEIGHT, colors);
        raw.raw_pixels = pixels;

        let sampled_offsets = raw
            .white_balance_offsets_from_area([0.1, 0.1], [0.9, 0.9], 0.0)
            .unwrap();
        let sampled = raw
            .adjusted_white_balance_and_camera_transform(sampled_offsets.0, sampled_offsets.1)
            .0;
        for channel in [0, 1, 2] {
            assert!(
                (sampled[channel] - desired[channel]).abs() < 0.015,
                "sampled={sampled:?}, desired={desired:?}"
            );
        }
    }

    #[test]
    fn inpaint_opposed_chrominance_uses_darktable_cube_root_reference() {
        const WIDTH: u32 = 96;
        const HEIGHT: u32 = 96;
        const WHITE: f32 = 10_000.0;
        let mut raw = raw_with_white_balance_model();
        raw.width = WIDTH;
        raw.height = HEIGHT;
        raw.wb_coeffs = [1.0; 4];
        raw.white_levels = [WHITE; 4];
        raw.white_balance_model = None;

        let mut colors = Vec::with_capacity((WIDTH * HEIGHT) as usize);
        let mut pixels = Vec::with_capacity((WIDTH * HEIGHT) as usize);
        for row in 0..HEIGHT {
            for col in 0..WIDTH {
                let physical = match (col % 2, row % 2) {
                    (0, 0) => 0,
                    (1, 0) => 1,
                    (0, 1) => 3,
                    _ => 2,
                };
                colors.push(physical);
                let logical = if physical == 3 { 1 } else { physical };
                let mut value = [0.8, 0.6, 0.4][logical as usize];
                if logical == 0 && (42..54).contains(&col) && (42..54).contains(&row) {
                    value = 1.0;
                }
                pixels.push((value * WHITE).round() as u16);
            }
        }
        raw.raw_pixels = pixels;
        raw.color_indices = CompactPixelMap::dense(WIDTH, HEIGHT, colors);
        raw.black_levels_per_pixel = CompactPixelMap::repeating(WIDTH, HEIGHT, 1, 1, vec![0.0]);
        raw.opposed_chroma_cache = Default::default();

        let chroma = raw.inpaint_opposed_chroma(0.0, 1.0, false, raw.wb_coeffs);
        let opposed_root = 0.5 * (0.6f32.cbrt() + 0.4f32.cbrt());
        let expected_red = 0.8 - opposed_root * opposed_root * opposed_root;
        assert!((chroma[0] - expected_red).abs() < 0.005, "{chroma:?}");
        assert!(chroma.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn inpaint_opposed_chrominance_pads_partial_three_pixel_edge_blocks() {
        const WIDTH: u32 = 26;
        const HEIGHT: u32 = 24;
        let mut raw = raw_with_white_balance_model();
        raw.width = WIDTH;
        raw.height = HEIGHT;
        raw.raw_pixels = vec![1_000; (WIDTH * HEIGHT) as usize];
        raw.color_indices =
            CompactPixelMap::dense(WIDTH, HEIGHT, vec![2; (WIDTH * HEIGHT) as usize]);
        raw.black_levels_per_pixel = CompactPixelMap::repeating(WIDTH, HEIGHT, 1, 1, vec![0.0]);
        raw.white_levels = [10_000.0; 4];
        raw.wb_coeffs = [1.0; 4];
        raw.opposed_chroma_cache = Default::default();

        let chroma = raw.inpaint_opposed_chroma(0.0, 1.0, false, raw.wb_coeffs);
        assert!(chroma.iter().all(|value| value.is_finite()));
    }
}
