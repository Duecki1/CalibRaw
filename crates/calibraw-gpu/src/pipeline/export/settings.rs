//! Export settings, limits, metadata, targets and progress events.

use super::*;

impl ExportFormat {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Tiff => "TIFF",
            Self::JpegXl => "JPEG XL",
        }
    }

    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Tiff => "tif",
            Self::JpegXl => "jxl",
        }
    }

    pub const fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Png => &["png"],
            Self::Jpeg => &["jpg", "jpeg"],
            Self::Tiff => &["tif", "tiff"],
            Self::JpegXl => &["jxl"],
        }
    }

    pub fn matches_extension(self, extension: &str) -> bool {
        self.extensions()
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
    }

    pub const fn mime_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Tiff => "image/tiff",
            Self::JpegXl => "image/jxl",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExportBitDepth {
    Eight,
    #[default]
    Sixteen,
    Float32Linear,
}

impl ExportBitDepth {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Eight => "8-bit integer",
            Self::Sixteen => "16-bit integer",
            Self::Float32Linear => "32-bit float / linear master",
        }
    }

    pub const fn is_float(self) -> bool {
        matches!(self, Self::Float32Linear)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExportResizeMode {
    #[default]
    Original,
    LongEdge,
    ShortEdge,
    Width,
    Height,
    Percentage,
}

impl ExportResizeMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Original => "Original",
            Self::LongEdge => "Long Edge",
            Self::ShortEdge => "Short Edge",
            Self::Width => "Width",
            Self::Height => "Height",
            Self::Percentage => "Percentage",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExportSettings {
    pub resize_mode: ExportResizeMode,
    pub edge_or_dimension: u32,
    pub percentage: f32,
    pub allow_upscale: bool,
    pub keep_metadata: bool,
    pub jpeg_quality: u8,
    pub bit_depth: ExportBitDepth,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            resize_mode: ExportResizeMode::Original,
            edge_or_dimension: 3000,
            percentage: 100.0,
            allow_upscale: false,
            keep_metadata: true,
            jpeg_quality: 90,
            bit_depth: ExportBitDepth::Sixteen,
        }
    }
}

pub const MAX_EXPORT_EDGE: u32 = 32_768;
#[cfg(target_os = "android")]
pub const MAX_EXPORT_PIXELS: u64 = 50_000_000;
#[cfg(not(target_os = "android"))]
pub const MAX_EXPORT_PIXELS: u64 = 120_000_000;
#[cfg(target_os = "android")]
pub(super) const MAX_EXPORT_BAND_BYTES: u64 = 64 * 1024 * 1024;
#[cfg(not(target_os = "android"))]
pub(super) const MAX_EXPORT_BAND_BYTES: u64 = 192 * 1024 * 1024;
impl ExportSettings {
    pub fn output_dimensions(&self, source_width: u32, source_height: u32) -> (u32, u32) {
        let source_width = source_width.max(1);
        let source_height = source_height.max(1);
        if self.resize_mode == ExportResizeMode::Original {
            return (source_width, source_height);
        }

        let width = source_width as f64;
        let height = source_height as f64;
        let requested = self.edge_or_dimension.max(1) as f64;
        let mut scale = match self.resize_mode {
            ExportResizeMode::Original => 1.0,
            ExportResizeMode::LongEdge => requested / width.max(height),
            ExportResizeMode::ShortEdge => requested / width.min(height),
            ExportResizeMode::Width => requested / width,
            ExportResizeMode::Height => requested / height,
            ExportResizeMode::Percentage => f64::from(self.percentage.clamp(1.0, 400.0)) / 100.0,
        };
        if !self.allow_upscale {
            scale = scale.min(1.0);
        }
        scale = scale.max(1.0 / width.max(height));

        let output_width = (width * scale).round().clamp(1.0, u32::MAX as f64) as u32;
        let output_height = (height * scale).round().clamp(1.0, u32::MAX as f64) as u32;
        (output_width, output_height)
    }

    pub fn checked_output_dimensions(
        &self,
        source_width: u32,
        source_height: u32,
    ) -> Result<(u32, u32)> {
        let dimensions = self.output_dimensions(source_width, source_height);
        validate_export_dimensions(dimensions.0, dimensions.1)?;
        Ok(dimensions)
    }
}

pub(super) fn validate_export_dimensions(width: u32, height: u32) -> Result<()> {
    anyhow::ensure!(
        width > 0 && height > 0,
        "export dimensions must be non-zero"
    );
    anyhow::ensure!(
        width <= MAX_EXPORT_EDGE && height <= MAX_EXPORT_EDGE,
        "export dimensions {width}x{height} exceed the {MAX_EXPORT_EDGE}-pixel edge limit"
    );
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .context("export pixel count overflow")?;
    anyhow::ensure!(
        pixels <= MAX_EXPORT_PIXELS,
        "export dimensions {width}x{height} contain {pixels} pixels; the limit is {MAX_EXPORT_PIXELS}"
    );
    Ok(())
}

#[derive(Clone, Debug, Default)]
pub struct ExportMetadata {
    pub exif_dates: Vec<(u16, String)>,
    pub source_file_name: Option<String>,
    pub camera_make: String,
    pub camera_model: String,
    pub lens_make: String,
    pub lens_model: String,
    pub focal_length: f32,
    pub aperture: f32,
    pub focus_distance: f32,
    pub iso_speed: f32,
    pub shutter_seconds: f32,
    pub description: String,
    pub artist: String,
    pub source_width: u32,
    pub source_height: u32,
}

impl ExportMetadata {
    pub fn from_raw(raw: &LoadedRaw, source_file_name: Option<String>) -> Self {
        Self {
            exif_dates: raw.capture_metadata.exif_dates.clone(),
            source_file_name,
            camera_make: raw.camera_make.clone(),
            camera_model: raw.camera_model.clone(),
            lens_make: raw.lens_make.clone(),
            lens_model: raw.lens_model.clone(),
            focal_length: raw.focal_length,
            aperture: raw.aperture,
            focus_distance: raw.focus_distance,
            iso_speed: raw.capture_metadata.iso_speed,
            shutter_seconds: raw.capture_metadata.shutter_seconds,
            description: raw.capture_metadata.description.clone(),
            artist: raw.capture_metadata.artist.clone(),
            source_width: raw.width,
            source_height: raw.height,
        }
    }
}

#[derive(Debug)]
pub enum ExportEvent {
    Progress {
        completed_tiles: usize,
        total_tiles: usize,
    },
    Finished(Result<PathBuf, String>),
}

/// Where an export is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportTarget {
    /// A file path. The export is written to a temporary file in the same
    /// directory and renamed into place on success, so a failed or cancelled
    /// export never leaves a partial file at the path.
    File(PathBuf),
    /// A writable descriptor path the platform handed out (an Android
    /// MediaStore `/proc/self/fd/N`), which cannot be renamed into. The export
    /// writes into it directly and stages intermediate files in `staging_dir`;
    /// the caller publishes or cancels the descriptor afterwards.
    Descriptor { path: PathBuf, staging_dir: PathBuf },
}

impl ExportTarget {
    /// The final output path.
    pub fn path(&self) -> &Path {
        match self {
            Self::File(path) | Self::Descriptor { path, .. } => path,
        }
    }
}

impl ExportFormat {
    pub(super) fn worker_name(self) -> &'static str {
        match self {
            Self::Png => "calibraw-tiled-export",
            Self::Jpeg => "calibraw-tiled-jpeg-export",
            Self::Tiff => "calibraw-tiled-tiff-export",
            Self::JpegXl => "calibraw-tiled-jxl-export",
        }
    }

    pub(super) fn worker_spawn_error(self) -> &'static str {
        match self {
            Self::Png => "could not start export worker",
            Self::Jpeg => "could not start JPEG export worker",
            Self::Tiff => "could not start TIFF export worker",
            Self::JpegXl => "could not start JPEG XL export worker",
        }
    }
}
