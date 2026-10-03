//! Rendered photos (JPEG, PNG, HEIC/HEIF) decoded into the same scene-linear
//! Rec.2020 raster path that rendered TIFFs use. The pixels stay
//! display-referred, so an unedited photo renders as it was captured.

use super::display_raster::{
    encoded_rgb_to_scene_linear_rec2020, ensure_finite, scene_linear_thumbnail, EncodedColorSpace,
    Primaries,
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
    SrgbCurve(Primaries),
}

impl DecodedImage {
    fn into_scene_linear_rec2020(self, path: &Path) -> Result<Vec<f32>> {
        let mut rgb = self.pixels.into_rgb32f().into_raw();
        let space = match &self.color {
            SourceColor::Icc(profile) => EncodedColorSpace::Icc(profile),
            SourceColor::SrgbCurve(primaries) => EncodedColorSpace::SrgbCurve(*primaries),
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
        color: icc.map_or(SourceColor::SrgbCurve(Primaries::Srgb), SourceColor::Icc),
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
            return Ok(SourceColor::SrgbCurve(match nclx.primaries {
                9 => Primaries::Rec2020,
                12 => Primaries::DisplayP3,
                _ => Primaries::Srgb,
            }));
        }
    }
    Ok(SourceColor::SrgbCurve(Primaries::Srgb))
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    fn temp_image(suffix: &str) -> tempfile::NamedTempFile {
        tempfile::Builder::new()
            .prefix("calibraw-rendered-")
            .suffix(suffix)
            .tempfile()
            .unwrap()
    }

    /// Splices an EXIF APP1 segment with Orientation = 6 (rotate 90° CW)
    /// after the SOI marker of an encoded JPEG.
    fn with_rotate_90_orientation(jpeg: &[u8]) -> Vec<u8> {
        let mut tiff = b"II*\0\x08\0\0\0\x01\0".to_vec();
        tiff.extend_from_slice(&0x0112u16.to_le_bytes());
        tiff.extend_from_slice(&3u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&6u32.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());
        let mut segment = b"Exif\0\0".to_vec();
        segment.extend_from_slice(&tiff);
        let mut output = jpeg[..2].to_vec();
        output.extend_from_slice(&[0xFF, 0xE1]);
        output.extend_from_slice(&((segment.len() + 2) as u16).to_be_bytes());
        output.extend_from_slice(&segment);
        output.extend_from_slice(&jpeg[2..]);
        output
    }

    #[test]
    fn png_white_decodes_to_scene_linear_white() {
        let file = temp_image(".png");
        ImageBuffer::<Rgb<u16>, _>::from_raw(2, 1, vec![65535, 65535, 65535, 0, 0, 0])
            .unwrap()
            .save_with_format(file.path(), ImageFormat::Png)
            .unwrap();

        let raw = load_rendered_image(file.path(), RenderedImageFormat::Png).unwrap();
        assert!(raw.is_pre_demosaiced_raster());
        assert!(!raw.is_camera_linear_raster());
        let rgb = raw.scene_linear_raster().unwrap();
        assert!(
            rgb[..3].iter().all(|value| (value - 1.0).abs() < 1e-4),
            "{rgb:?}"
        );
        assert!(rgb[3..].iter().all(|value| value.abs() < 1e-6), "{rgb:?}");

        let mut exposure = super::super::ExposureParams::default();
        raw.apply_adaptive_detail_defaults(&mut exposure);
        assert_eq!(exposure.sharpen_amount, 0.0);
        assert_eq!(exposure.luminance_denoise, 0.0);
        assert_eq!(exposure.chroma_denoise, 0.0);
    }

    #[test]
    fn public_loaders_route_rendered_images_by_content() {
        let file = temp_image("");
        ImageBuffer::<Rgb<u8>, _>::from_raw(3, 2, vec![128; 18])
            .unwrap()
            .save_with_format(file.path(), ImageFormat::Png)
            .unwrap();
        // Android hands the decoder an extensionless /proc/self/fd path.
        assert!(file.path().extension().is_none());
        let raw = super::super::load_raw_file(file.path()).unwrap();
        assert_eq!([raw.width, raw.height], [3, 2]);
        let metadata = super::super::load_raw_display_metadata(file.path()).unwrap();
        assert_eq!(metadata.dimensions, [3, 2]);
        let thumbnail = super::super::load_raw_thumbnail(file.path(), 64).unwrap();
        assert_eq!([thumbnail.width, thumbnail.height], [3, 2]);
    }

    #[test]
    fn jpeg_orientation_is_applied_to_pixels_and_display_dimensions() {
        let file = temp_image(".jpg");
        let source = ImageBuffer::<Rgb<u8>, _>::from_fn(16, 8, |x, _| {
            if x < 8 {
                Rgb([255, 255, 255])
            } else {
                Rgb([0, 0, 0])
            }
        });
        let mut jpeg = Vec::new();
        source
            .write_to(&mut std::io::Cursor::new(&mut jpeg), ImageFormat::Jpeg)
            .unwrap();
        std::fs::write(file.path(), with_rotate_90_orientation(&jpeg)).unwrap();

        let metadata =
            load_rendered_display_metadata(file.path(), RenderedImageFormat::Jpeg).unwrap();
        assert_eq!(metadata.dimensions, [8, 16]);

        let raw = load_rendered_image(file.path(), RenderedImageFormat::Jpeg).unwrap();
        assert_eq!([raw.width, raw.height], [8, 16]);
        // Rotating 90° clockwise moves the white left half to the top.
        let rgb = raw.scene_linear_raster().unwrap();
        let top = rgb[(8 + 4) * 3];
        let bottom = rgb[(15 * 8 + 4) * 3];
        assert!(top > 0.9 && bottom < 0.1, "top={top} bottom={bottom}");

        let thumbnail = load_rendered_thumbnail(file.path(), RenderedImageFormat::Jpeg, 4).unwrap();
        assert_eq!([thumbnail.width, thumbnail.height], [2, 4]);
    }

    #[test]
    fn heif_decoder_panics_become_errors() {
        let error = heif_guarded::<()>(|| panic!("decoder bug")).unwrap_err();
        assert!(error.to_string().contains("malformed or unsupported"));
    }

    #[test]
    fn truncated_heif_is_an_error_not_a_crash() {
        let bytes = include_bytes!("../../tests/fixtures/display-p3-32x16.heic");
        for length in [0, 16, 64, bytes.len() / 2, bytes.len() - 1] {
            let file = temp_image(".heic");
            std::fs::write(file.path(), &bytes[..length]).unwrap();
            assert!(load_rendered_image(file.path(), RenderedImageFormat::Heif).is_err());
        }
    }

    #[test]
    fn display_p3_heif_decodes_with_wide_gamut_primaries() {
        // heif-enc -q 95 --colour_primaries 12 --transfer_characteristic 13
        // of a 32x16 PNG: white left half, red top-right quarter, black rest.
        let file = temp_image(".heic");
        std::fs::write(
            file.path(),
            include_bytes!("../../tests/fixtures/display-p3-32x16.heic"),
        )
        .unwrap();

        let metadata =
            load_rendered_display_metadata(file.path(), RenderedImageFormat::Heif).unwrap();
        assert_eq!(metadata.dimensions, [32, 16]);

        let raw = load_rendered_image(file.path(), RenderedImageFormat::Heif).unwrap();
        assert_eq!([raw.width, raw.height], [32, 16]);
        let rgb = raw.scene_linear_raster().unwrap();
        let pixel = |x: usize, y: usize| &rgb[(y * 32 + x) * 3..][..3];
        assert!(pixel(4, 8).iter().all(|value| (value - 1.0).abs() < 0.02));
        assert!(pixel(28, 12).iter().all(|value| value.abs() < 0.02));
        // P3 red lands well outside sRGB red in Rec.2020 (0.627, 0.069, 0.016).
        let red = pixel(24, 3);
        assert!(red[0] > 0.70 && red[1] < 0.06, "{red:?}");

        let thumbnail = load_rendered_thumbnail(file.path(), RenderedImageFormat::Heif, 8).unwrap();
        assert_eq!([thumbnail.width, thumbnail.height], [8, 4]);
    }
}
