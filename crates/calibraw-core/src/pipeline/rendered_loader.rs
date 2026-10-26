//! Rendered photos (JPEG, PNG, HEIC/HEIF) decoded into the same scene-linear
//! Rec.2020 raster path that rendered TIFFs use. The pixels stay
//! display-referred, so an unedited photo renders as it was captured.

use super::display_raster::{
    encoded_rgb_to_scene_linear_rec2020, ensure_finite, scene_linear_thumbnail, EncodedColorSpace,
    Primaries, Transfer,
};
use super::exif_metadata::ExifSummary;
use super::raw_loader::{
    validate_raw_dimensions, LoadedRaw, RawDisplayMetadata, RawThumbnail, MAX_RAW_EDGE,
    MAX_RAW_PIXELS,
};
use super::source_format::RenderedImageFormat;
use anyhow::{anyhow, Context, Result};
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

#[cfg(target_os = "android")]
const MAX_DECODE_BYTES: u64 = 768 * 1024 * 1024;
#[cfg(not(target_os = "android"))]
const MAX_DECODE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_HEIF_FILE_BYTES: u64 = 512 * 1024 * 1024;
/// HEIF metadata (`ftyp` + `meta`) precedes the coded data in practice, so
/// probing a prefix avoids reading whole files while indexing a library.
const HEIF_PROBE_PREFIX_BYTES: u64 = 1024 * 1024;

pub(super) fn load_rendered_image(path: &Path, format: RenderedImageFormat) -> Result<LoadedRaw> {
    let decoded = decode(path, format)?;
    let (width, height) = (decoded.pixels.width(), decoded.pixels.height());
    let rgb = decoded.into_scene_linear_rec2020(path)?;
    let mut raw = LoadedRaw::from_scene_linear_rec2020(width, height, rgb)?;
    ExifSummary::read_or_default(path).apply_to(&mut raw);
    Ok(raw)
}

pub(super) fn load_rendered_thumbnail(
    path: &Path,
    format: RenderedImageFormat,
    maximum_edge: u32,
) -> Result<RawThumbnail> {
    anyhow::ensure!(maximum_edge > 0, "thumbnail edge must be non-zero");
    let DecodedImage { pixels, color } = decode(path, format)?;
    // Downscaling the encoded pixels first keeps colour conversion cheap; the
    // gamma-space resample is invisible at thumbnail size.
    let pixels = crate::thumbnail_cache::downscale_to_fit(pixels, maximum_edge);
    let (width, height) = (pixels.width(), pixels.height());
    let rgb = DecodedImage { pixels, color }.into_scene_linear_rec2020(path)?;
    Ok(scene_linear_thumbnail(width, height, &rgb))
}

pub(super) fn load_rendered_display_metadata(
    path: &Path,
    format: RenderedImageFormat,
) -> Result<RawDisplayMetadata> {
    let dimensions = match format {
        RenderedImageFormat::Heif => heif_dimensions(path)?,
        RenderedImageFormat::Jpeg | RenderedImageFormat::Png => {
            let mut decoder = open_decoder(path, format)?;
            let (width, height) = decoder.dimensions();
            match decoder.orientation().unwrap_or(Orientation::NoTransforms) {
                Orientation::Rotate90
                | Orientation::Rotate270
                | Orientation::Rotate90FlipH
                | Orientation::Rotate270FlipH => [height, width],
                _ => [width, height],
            }
        }
    };
    validate_raw_dimensions(dimensions[0], dimensions[1])?;
    Ok(ExifSummary::read_or_default(path).display_metadata(dimensions))
}

/// Upright pixels, still in their encoded form, and how to read their colour.
struct DecodedImage {
    pixels: DynamicImage,
    color: SourceColor,
}

enum SourceColor {
    Icc(Vec<u8>),
    Coded {
        primaries: Primaries,
        transfer: Transfer,
    },
}

impl SourceColor {
    const SRGB: Self = Self::Coded {
        primaries: Primaries::Srgb,
        transfer: Transfer::Srgb,
    };
}

impl DecodedImage {
    fn into_scene_linear_rec2020(self, path: &Path) -> Result<Vec<f32>> {
        let mut rgb = self.pixels.into_rgb32f().into_raw();
        let space = match &self.color {
            SourceColor::Icc(profile) => EncodedColorSpace::Icc(profile),
            SourceColor::Coded {
                primaries,
                transfer,
            } => EncodedColorSpace::Coded {
                primaries: *primaries,
                transfer: *transfer,
            },
        };
        if let Err(error) = encoded_rgb_to_scene_linear_rec2020(&mut rgb, space) {
            // Photos commonly carry LUT or CMYK profiles CalibRaw cannot
            // evaluate. Showing them as sRGB beats refusing to open them.
            let message = format!(
                "embedded ICC profile in {} is not supported; assuming sRGB: {error:#}",
                path.display()
            );
            log::warn!("{message}");
            crate::diagnostics::record(message);
            encoded_rgb_to_scene_linear_rec2020(&mut rgb, EncodedColorSpace::SRGB)?;
        }
        ensure_finite(&rgb)?;
        Ok(rgb)
    }
}

fn decode(path: &Path, format: RenderedImageFormat) -> Result<DecodedImage> {
    match format {
        RenderedImageFormat::Jpeg | RenderedImageFormat::Png => decode_with_image(path, format),
        RenderedImageFormat::Heif => decode_heif(path),
    }
    .with_context(|| format!("decode {} {}", format.label(), path.display()))
}

fn decode_with_image(path: &Path, format: RenderedImageFormat) -> Result<DecodedImage> {
    let mut decoder = open_decoder(path, format)?;
    let (width, height) = decoder.dimensions();
    validate_raw_dimensions(width, height)?;
    let icc = decoder.icc_profile().ok().flatten();
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut pixels = DynamicImage::from_decoder(decoder)?;
    pixels.apply_orientation(orientation);
    Ok(DecodedImage {
        pixels,
        color: icc.map_or(SourceColor::SRGB, SourceColor::Icc),
    })
}

fn open_decoder(path: &Path, format: RenderedImageFormat) -> Result<impl ImageDecoder> {
    let image_format = match format {
        RenderedImageFormat::Jpeg => ImageFormat::Jpeg,
        RenderedImageFormat::Png => ImageFormat::Png,
        RenderedImageFormat::Heif => return Err(anyhow!("HEIF is not decoded by the image crate")),
    };
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = image::ImageReader::with_format(BufReader::new(file), image_format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_RAW_EDGE);
    limits.max_image_height = Some(MAX_RAW_EDGE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    Ok(reader.into_decoder()?)
}

fn decode_heif(path: &Path) -> Result<DecodedImage> {
    let bytes = read_heif(path, None)?;
    heif_guarded(|| {
        let info = heic_rs::probe(&bytes)?;
        validate_raw_dimensions(info.width, info.height)?;
        let color = heif_color(&bytes)?;
        let options = heic_rs::DecodeOptions::default()
            .with_layout(heic_rs::PixelLayout::Rgb16)
            .with_max_pixels(Some(MAX_RAW_PIXELS))
            .with_alpha(false);
        let image = heic_rs::decode(&bytes, &options)?;
        let samples = image
            .data
            .chunks_exact(2)
            .map(|sample| u16::from_ne_bytes([sample[0], sample[1]]))
            .collect();
        let pixels = image::ImageBuffer::from_raw(image.width, image.height, samples)
            .context("HEIF decoder returned a buffer that does not match its dimensions")?;
        Ok(DecodedImage {
            pixels: DynamicImage::ImageRgb16(pixels),
            color,
        })
    })
}

/// heic-rs is young; a panic on a malformed or unusual file must surface as
/// an ordinary decode error instead of taking down the worker thread.
fn heif_guarded<T>(operation: impl FnOnce() -> Result<T>) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation))
        .map_err(|_| anyhow!("the HEIF decoder rejected malformed or unsupported data"))?
}

/// Colour comes from the primary item, or from its first tile when a grid
/// image only describes colour on the coded tiles.
fn heif_color(bytes: &[u8]) -> Result<SourceColor> {
    let context = heic_rs::context::Context::open(bytes)?;
    let primary = context.meta.primary;
    let mut items = vec![primary];
    if let Some((_, tiles)) = context.grid(primary)? {
        items.extend(tiles.first());
    }
    for item in items {
        let props = context.props(item)?;
        if let Some(profile) = context.icc(&props) {
            return Ok(SourceColor::Icc(profile.to_vec()));
        }
        if let Some(nclx) = props.nclx {
            return Ok(SourceColor::Coded {
                primaries: Primaries::from_h273(nclx.primaries),
                transfer: Transfer::from_h273(nclx.transfer),
            });
        }
    }
    Ok(SourceColor::SRGB)
}

fn heif_dimensions(path: &Path) -> Result<[u32; 2]> {
    let prefix = read_heif(path, Some(HEIF_PROBE_PREFIX_BYTES))?;
    let info = match heif_guarded(|| Ok(heic_rs::probe(&prefix)?)) {
        Ok(info) => info,
        Err(_) => {
            let bytes = read_heif(path, None)?;
            heif_guarded(|| Ok(heic_rs::probe(&bytes)?))?
        }
    };
    Ok([info.width, info.height])
}

fn read_heif(path: &Path, limit: Option<u64>) -> Result<Vec<u8>> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let length = file
        .metadata()
        .with_context(|| format!("inspect {}", path.display()))?
        .len();
    anyhow::ensure!(
        length <= MAX_HEIF_FILE_BYTES,
        "HEIF file is {length} bytes; the limit is {MAX_HEIF_FILE_BYTES}"
    );
    let take = limit.map_or(length, |limit| limit.min(length));
    let mut bytes = Vec::with_capacity(usize::try_from(take).unwrap_or(0));
    file.take(take)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {}", path.display()))?;
    Ok(bytes)
}
