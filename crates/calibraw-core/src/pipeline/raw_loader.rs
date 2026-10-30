// SPDX-License-Identifier: GPL-3.0-or-later
// Highlight reconstruction includes adaptations from darktable 5.6.0.
// Copyright (C) 2010-2026 darktable developers.
// Copyright (C) 2026 CalibRaw contributors (Rust adaptation).

use super::basicadj::{
    temperature_kelvin_from_offset, temperature_offset_from_kelvin, white_balance_tint_from_offset,
    white_balance_tint_offset, ExposureParams, HighlightReconstructionMethod, WhiteBalance,
    GLOBAL_TEMPERATURE_LIMIT, GLOBAL_TINT_OFFSET_LIMIT,
};
use super::chromatic_aberration::LateralChromaticAberration;
use super::color_profile::CameraProfile;
use super::exif_metadata::PreservedExifField;
use super::geometry::LensGeometryMap;
use super::lens_shading::LensShadingMap;
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
use std::ops::{Deref, DerefMut, Index, RangeInclusive};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

mod binned_sensor;
mod compact_pixel_map;
mod opposed_chroma;
mod profile_color;
mod sensor_samples;
mod sensor_saturation;
mod white_balance;
pub use binned_sensor::BinnedSensor;
pub use compact_pixel_map::*;
pub use opposed_chroma::*;
pub use profile_color::{ProfileColor, ProfileColorSource};
pub use sensor_samples::SensorSamples;
pub use sensor_saturation::SensorSaturation;

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
    "mos", "mrw", "nef", "nefx", "nrw", "obm", "orf", "pef", "ptx", "pxn", "qtk", "raf", "raw",
    "rdc", "rw2", "rwl", "rwz", "sr2", "srf", "srw", "sti", "tif", "tiff",
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
/// Largest decoded image of any kind. The 210 MP cap covers high-resolution
/// phone captures; a scene-linear raster costs 12 bytes per pixel in memory.
#[cfg(target_os = "android")]
pub const MAX_RAW_PIXELS: u64 = 210_000_000;
#[cfg(not(target_os = "android"))]
pub const MAX_RAW_PIXELS: u64 = 200_000_000;
#[cfg(all(libraw_available, target_os = "android"))]
const MAX_RAW_FILE_BYTES: u64 = 2_000_000_000;
#[cfg(all(libraw_available, not(target_os = "android")))]
const MAX_RAW_FILE_BYTES: u64 = 8_000_000_000;
#[cfg(all(libraw_available, target_os = "android"))]
const MAX_SENSOR_PIXELS: u64 = 210_000_000;
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DngColorEndpoint {
    pub cct: Option<f32>,
    pub color_matrix: [[f32; 3]; 4],
    pub calibration: [[f32; 4]; 4],
    pub forward_matrix: Option<[[f32; 4]; 3]>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CameraColorModel {
    Dng {
        endpoints: Box<[DngColorEndpoint; 2]>,
        analog_balance: [[f32; 4]; 4],
    },
    Matrix {
        xyz_to_camera: [[f32; 3]; 4],
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct CameraWhiteBalanceModel {
    pub base_wb: [f32; 4],
    pub cdesc: [u8; 4],
    pub base_cct: f32,
    pub color: CameraColorModel,
}

/// Fixed ranges of the white-balance sliders: temperature in Kelvin and the
/// multiplicative tint of the Y coordinate. Neither depends on the other
/// slider's value.
#[derive(Clone, Debug, PartialEq)]
pub struct WhiteBalanceRanges {
    pub temperature: RangeInclusive<f32>,
    pub tint: RangeInclusive<f32>,
}

impl WhiteBalanceRanges {
    /// Limits each value to its own range, independently of the other.
    pub fn clamp(&self, temperature: f32, tint: f32) -> (f32, f32) {
        (
            temperature.clamp(*self.temperature.start(), *self.temperature.end()),
            tint.clamp(*self.tint.start(), *self.tint.end()),
        )
    }
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
    /// EXIF fields the source carries that CalibRaw does not interpret,
    /// preserved so exports can write them back unchanged.
    pub preserved_exif: Vec<PreservedExifField>,
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

/// How a [`LoadedRaw`] holds its pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelLayout {
    /// One sample per photosite in `raw_pixels`, demosaiced on the GPU.
    Mosaic,
    /// Co-sited sensor RGB in `binned_sensor`: a reduced view of a mosaic,
    /// processed like one except that it skips demosaicing.
    Binned,
    /// Demosaiced RGB in `scene_linear_raster`.
    Raster,
}

/// A decoded source photo ready for the develop pipeline. Despite the name it
/// holds sensor data (`raw_pixels` + CFA metadata), a reduced sensor view
/// binned to RGB (`binned_sensor`) or, for rendered TIFF/JPEG/PNG/HEIF photos,
/// a scene-linear Rec.2020 raster; see [`LoadedRaw::pixel_layout`] and
/// [`LoadedRaw::is_display_referred_raster`].
#[derive(Clone, Debug)]
pub struct LoadedRaw {
    pub width: u32,
    pub height: u32,
    /// Width and height of the whole photo at its native resolution. Crops,
    /// proxies and tiles keep their source's value, so effects whose radius
    /// follows the photo's size (`resolution_scale` in common.wgsl) cover the
    /// same part of the photo in a reduced preview as in a full-size export.
    pub source_extent: [u32; 2],
    pub camera_make: String,
    pub camera_model: String,
    pub lens_make: String,
    pub lens_model: String,
    pub focal_length: f32,
    pub aperture: f32,
    pub focus_distance: f32,
    pub capture_metadata: CaptureMetadata,
    pub cfa_kind: CfaKind,
    pub raw_pixels: SensorSamples,
    /// The pixels of a reduced sensor view. Such a view keeps the mosaic's
    /// metadata and `cfa_kind`, but its `raw_pixels` are empty and its CFA and
    /// black-level maps are uniform placeholders.
    pub binned_sensor: Option<BinnedSensor>,
    pub scene_linear_raster: Option<Arc<[f32]>>,
    /// The raster holds scene radiance whose tones were never rendered, such as a
    /// floating-point HDR or focus-stack master, rather than a developed photo. It may
    /// exceed display white, so it always takes the scene view transform.
    pub scene_referred_raster: bool,
    pub color_indices: CompactPixelMap<u8>,
    /// The as-shot white balance multipliers.
    pub wb_coeffs: [f32; 4],
    pub black_levels: [f32; 4],
    pub black_levels_per_pixel: CompactPixelMap<f32>,
    pub white_levels: [f32; 4],
    /// Where photosites saturate relative to `white_levels`; highlight reconstruction clips
    /// against it. Crops, proxies and tiles keep their source's.
    pub sensor_saturation: SensorSaturation,
    pub noise_profile: NoiseProfile,
    pub profile_color: Arc<ProfileColor>,
    /// What the decode read `profile_color` from; crops, proxies and tiles
    /// keep their source's.
    pub color_source: Option<Arc<ProfileColorSource>>,
    pub available_camera_profiles: Vec<CameraProfileCandidate>,
    pub lens_geometry: Option<Arc<LensGeometryMap>>,
    /// Vignetting gains the GPU applies after highlight reconstruction; only
    /// sensor mosaics carry them. Placed over the whole photo, so crops,
    /// proxies and tiles keep them.
    pub lens_shading: Option<LensShadingMap>,
    /// Where red and blue sit relative to green, measured over the whole
    /// photo when it is decoded. Normalized to the photo, so crops, proxies
    /// and tiles keep their source's.
    pub lateral_chromatic_aberration: LateralChromaticAberration,
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
    /// from `self`, and so is the lens shading, which is placed over the whole photo. The view
    /// has no scene-linear raster, no lens geometry and a fresh, empty AI-denoise slot. It shares `self`'s opposed-chroma cache and source identity as a
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
            source_extent: self.source_extent,
            camera_make: self.camera_make.clone(),
            camera_model: self.camera_model.clone(),
            lens_make: self.lens_make.clone(),
            lens_model: self.lens_model.clone(),
            focal_length: self.focal_length,
            aperture: self.aperture,
            focus_distance: self.focus_distance,
            capture_metadata: self.capture_metadata.clone(),
            cfa_kind: self.cfa_kind,
            raw_pixels: raw_pixels.into(),
            binned_sensor: None,
            scene_linear_raster: None,
            scene_referred_raster: self.scene_referred_raster,
            color_indices,
            wb_coeffs: self.wb_coeffs,
            black_levels: self.black_levels,
            black_levels_per_pixel,
            white_levels: self.white_levels,
            sensor_saturation: self.sensor_saturation,
            noise_profile: self.noise_profile,
            profile_color: Arc::clone(&self.profile_color),
            color_source: self.color_source.clone(),
            available_camera_profiles: self.available_camera_profiles.clone(),
            lens_geometry: None,
            lens_shading: self.lens_shading.clone(),
            lateral_chromatic_aberration: self.lateral_chromatic_aberration,
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
            source_extent: [width, height],
            camera_make: String::new(),
            camera_model: String::new(),
            lens_make: String::new(),
            lens_model: String::new(),
            focal_length: 0.0,
            aperture: 0.0,
            focus_distance: 0.0,
            capture_metadata: CaptureMetadata::default(),
            cfa_kind: CfaKind::Bayer,
            raw_pixels: SensorSamples::default(),
            binned_sensor: None,
            scene_linear_raster: Some(rgb.into()),
            scene_referred_raster: false,
            color_indices: CompactPixelMap::repeating(width, height, 1, 1, vec![1]),
            wb_coeffs: [1.0; 4],
            black_levels: [0.0; 4],
            black_levels_per_pixel: CompactPixelMap::repeating(width, height, 1, 1, vec![0.0]),
            white_levels: [1.0; 4],
            sensor_saturation: SensorSaturation::AT_WHITE,
            noise_profile: NoiseProfile::default(),
            profile_color: Arc::default(),
            color_source: None,
            available_camera_profiles: Vec::new(),
            lens_geometry: None,
            lens_shading: None,
            lateral_chromatic_aberration: LateralChromaticAberration::NONE,
            ai_denoised: Arc::new(RwLock::new(None)),
            opposed_chroma_cache: Default::default(),
            opposed_chroma_source_identity: Default::default(),
            opposed_chroma_reference_source: true,
        })
    }

    pub fn pixel_layout(&self) -> PixelLayout {
        if self.binned_sensor.is_some() {
            PixelLayout::Binned
        } else if self.is_pre_demosaiced_raster() {
            PixelLayout::Raster
        } else {
            PixelLayout::Mosaic
        }
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

    /// Whether temperature and tint apply to this photo. Rendered photos and
    /// raster TIFFs have no camera white-balance model, nor do RAWs without
    /// usable colour metadata.
    pub fn has_white_balance_model(&self) -> bool {
        self.profile_color.white_balance_model.is_some()
    }

    /// A raster decoded from camera-space LinearRaw samples. Unlike normal
    /// scene rasters, these pixels still need the camera white balance and
    /// camera-to-working transform applied by the GPU.
    pub fn is_camera_linear_raster(&self) -> bool {
        self.is_pre_demosaiced_raster() && self.profile_color.white_balance_model.is_some()
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
        Ok(super::tiff_loader::is_raster_tiff(path)?.then_some(Self::Tiff))
    }

    fn load(self, path: &Path) -> Result<LoadedRaw> {
        let mut raw = match self {
            Self::Tiff => super::tiff_loader::load_raster_tiff(path),
            Self::Rendered(format) => super::rendered_loader::load_rendered_image(path, format),
        }?;
        // These sources carry no sensor data to profile, so the noise model
        // that Detail's denoise and sharpening read is measured from pixels.
        if let Some(rgb) = raw.scene_linear_raster() {
            raw.noise_profile = NoiseProfile::estimate_raster(raw.width, raw.height, rgb);
        }
        raw.lateral_chromatic_aberration = LateralChromaticAberration::estimate(&raw);
        Ok(raw)
    }

    fn thumbnail(self, path: &Path, maximum_edge: u32) -> Result<RawThumbnail> {
        // These decoders interleave small reads with decoding, so in HDD mode
        // the file is read once up front and decoded from the page cache.
        crate::serialized_reads::prefetch_file(path);
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

/// Whether `path` decodes as a display-referred raster (a rendered photo or a
/// raster TIFF), which never has a camera white-balance model. Reads the
/// file header.
pub fn decodes_as_raster(path: &Path) -> Result<bool> {
    Ok(RasterSource::detect(path)?.is_some())
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

/// Loads a thumbnail like [`load_raw_thumbnail`] together with the photo's
/// display metadata, which is `None` when it cannot be read.
#[cfg(not(libraw_available))]
pub fn load_raw_thumbnail_with_display_metadata(
    path: &Path,
    maximum_edge: u32,
) -> Result<(RawThumbnail, Option<RawDisplayMetadata>)> {
    let thumbnail = load_raw_thumbnail(path, maximum_edge)?;
    let display_metadata = crate::serialized_reads::run(|| load_raw_display_metadata(path)).ok();
    Ok((thumbnail, display_metadata))
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

/// Runs `primary`, and `fallback` only when it fails. Both failures are kept
/// in the returned error so the user sees why each backend refused the file.
#[cfg(libraw_available)]
fn try_with_fallback<T>(
    path: &Path,
    operation: &str,
    (primary_name, primary): (&str, impl FnOnce() -> Result<T>),
    (fallback_name, fallback): (&str, impl FnOnce() -> Result<T>),
) -> Result<T> {
    match primary() {
        Ok(value) => Ok(value),
        Err(primary_error) => {
            let primary_detail = format!("{primary_error:#}");
            log::warn!(
                "{primary_name} {operation} failed for {}; falling back to {fallback_name}: {primary_detail}",
                path.display()
            );
            crate::diagnostics::record(format!(
                "{primary_name} {operation} failed; retrying through {fallback_name}: {primary_detail}"
            ));
            fallback().with_context(|| {
                format!(
                    "{primary_name} {operation} failed first ({primary_detail}); {fallback_name} fallback also failed"
                )
            })
        }
    }
}

#[cfg(libraw_available)]
fn try_rawler_then_libraw<T>(
    path: &Path,
    operation: &str,
    rawler: impl FnOnce() -> Result<T>,
    libraw: impl FnOnce() -> Result<T>,
) -> Result<T> {
    try_with_fallback(path, operation, ("Rawler", rawler), ("LibRaw", libraw))
}

/// Proprietary RAWs stay on LibRaw; Rawler only covers files LibRaw rejects,
/// such as cameras newer than its colour tables.
#[cfg(libraw_available)]
fn try_libraw_then_rawler<T>(
    path: &Path,
    operation: &str,
    libraw: impl FnOnce() -> Result<T>,
    rawler: impl FnOnce() -> Result<T>,
) -> Result<T> {
    try_with_fallback(path, operation, ("LibRaw", libraw), ("Rawler", rawler))
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
        try_libraw_then_rawler(
            path,
            "RAW decode",
            || {
                libraw_loader::load_raw_file_with_profile_selection(
                    path,
                    mode,
                    profile_folder,
                    selected_profile,
                )
            },
            || {
                rawler_loader::load_proprietary_raw_file_with_profile_selection(
                    path,
                    mode,
                    profile_folder,
                    selected_profile,
                )
            },
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
        try_libraw_then_rawler(
            path,
            "DCP-backed RAW decode",
            || libraw_loader::load_raw_file_with_dcp(path, profile_path),
            || rawler_loader::load_proprietary_raw_file_with_dcp(path, profile_path),
        )
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

/// Loads a thumbnail like [`load_raw_thumbnail`] together with the photo's
/// display metadata, which is `None` when it cannot be read. LibRaw-decoded
/// RAWs take both from one open of the file; DNG and raster files read the
/// metadata separately, as [`load_raw_display_metadata`] does.
#[cfg(libraw_available)]
pub fn load_raw_thumbnail_with_display_metadata(
    path: &Path,
    maximum_edge: u32,
) -> Result<(RawThumbnail, Option<RawDisplayMetadata>)> {
    if RasterSource::detect(path)?.is_some() || path_is_dng(path) {
        let thumbnail = load_raw_thumbnail(path, maximum_edge)?;
        let display_metadata =
            crate::serialized_reads::run(|| load_raw_display_metadata(path)).ok();
        Ok((thumbnail, display_metadata))
    } else {
        libraw_loader::load_raw_thumbnail_with_display_metadata(path, maximum_edge)
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

/// The profile limit when the index of `folder` stopped at it, returned once per built
/// index so the caller can tell the user that some profiles are missing.
#[cfg(libraw_available)]
pub fn take_dcp_profile_limit_notice(folder: &Path) -> Option<usize> {
    libraw_loader::take_dcp_profile_limit_notice(folder)
}

#[cfg(not(libraw_available))]
pub fn take_dcp_profile_limit_notice(_folder: &Path) -> Option<usize> {
    None
}

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
