//! EXIF fields CalibRaw displays and carries into exports, read from any
//! container kamadak-exif understands (TIFF-based RAWs, JPEG, PNG, HEIF).

use super::raw_loader::{CaptureMetadata, LoadedRaw, RawDisplayMetadata};
use anyhow::{Context, Result};
use exif::{Context as ExifContext, Exif, Field, In, Tag, Value};
use std::path::Path;

/// Files larger than this are not scanned for EXIF, so a damaged or hostile
/// file cannot force a huge metadata read.
const MAX_EXIF_SCAN_BYTES: u64 = 256_000_000;

/// Space reserved for source fields an export copies verbatim. A JPEG EXIF
/// APP1 segment holds 65,533 bytes, including the fields CalibRaw writes
/// itself, so keep enough headroom for those and the IFD directories.
const MAX_PRESERVED_EXIF_BYTES: usize = 48 * 1024;

/// Which IFD of an EXIF payload holds a preserved field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExifIfd {
    /// IFD0, the primary image directory.
    Primary,
    /// The Exif IFD reached through IFD0 tag 0x8769.
    Exif,
    /// The GPS IFD reached through IFD0 tag 0x8825.
    Gps,
    /// The interoperability IFD reached through Exif IFD tag 0xa005.
    Interop,
}

/// An EXIF field carried from the source into exports without interpretation.
///
/// `value` is in the little-endian TIFF encoding a CalibRaw EXIF payload uses,
/// and `value.len()` is `count` times the size of `field_type`. CalibRaw writes
/// the fields it owns itself and copies the rest, so metadata it does not
/// display (GPS, exposure program, user comments, copyright, ...) survives an
/// export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreservedExifField {
    pub ifd: ExifIfd,
    pub tag: u16,
    /// TIFF field type (EXIF 2.32, table 4-6).
    pub field_type: u16,
    /// Number of values, not bytes.
    pub count: u32,
    /// Little-endian value bytes.
    pub value: Vec<u8>,
}

/// Date/time, subsecond and UTC-offset tags preserved verbatim for exports.
fn is_exif_date_tag(tag: u16) -> bool {
    matches!(tag, 0x0132 | 0x9003 | 0x9004 | 0x9010..=0x9012 | 0x9290..=0x9292)
}

#[derive(Clone, Debug, Default)]
pub(super) struct ExifSummary {
    pub(super) camera_make: String,
    pub(super) camera_model: String,
    pub(super) lens_make: String,
    pub(super) lens_model: String,
    pub(super) focal_length: f32,
    pub(super) aperture: f32,
    pub(super) capture: CaptureMetadata,
}

impl ExifSummary {
    pub(super) fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::metadata(path)
            .with_context(|| format!("inspect EXIF source {}", path.display()))?
            .len();
        if bytes > MAX_EXIF_SCAN_BYTES {
            return Ok(Self::default());
        }
        let file = std::fs::File::open(path)
            .with_context(|| format!("open EXIF source {}", path.display()))?;
        let mut reader = exif::Reader::new();
        reader.continue_on_error(true);
        let exif = reader
            .read_from_container(&mut std::io::BufReader::new(file))
            .or_else(|error| error.distill_partial_result(|_| {}))
            .context("read EXIF metadata")?;
        Ok(Self::from_exif(&exif))
    }

    /// Like [`Self::read`], but a file without readable EXIF yields empty
    /// metadata. Missing EXIF is normal for screenshots and edited exports.
    pub(super) fn read_or_default(path: &Path) -> Self {
        Self::read(path).unwrap_or_else(|error| {
            log::debug!("no EXIF metadata in {}: {error:#}", path.display());
            Self::default()
        })
    }

    fn from_exif(exif: &Exif) -> Self {
        let primary = |tag| exif.get_field(tag, In::PRIMARY);
        let text = |tag| primary(tag).and_then(ascii).unwrap_or_default();
        let number = |tag| primary(tag).and_then(positive_number).unwrap_or(0.0);
        let iso_speed = [Tag::PhotographicSensitivity, Tag::ISOSpeed]
            .into_iter()
            .find_map(|tag| primary(tag).and_then(positive_number))
            .unwrap_or(0.0);
        let exif_dates = exif
            .fields()
            .filter(|field| field.ifd_num == In::PRIMARY && is_exif_date_tag(field.tag.number()))
            .filter_map(|field| ascii(field).map(|value| (field.tag.number(), value)))
            .collect();
        Self {
            camera_make: text(Tag::Make),
            camera_model: text(Tag::Model),
            lens_make: text(Tag::LensMake),
            lens_model: text(Tag::LensModel),
            focal_length: number(Tag::FocalLength),
            aperture: number(Tag::FNumber),
            capture: CaptureMetadata {
                exif_dates,
                iso_speed,
                shutter_seconds: number(Tag::ExposureTime),
                flash: exif
                    .fields()
                    .find(|field| field.tag == Tag::Flash)
                    .and_then(|field| field.value.get_uint(0))
                    .and_then(|value| u16::try_from(value).ok()),
                description: text(Tag::ImageDescription),
                artist: text(Tag::Artist),
                preserved_exif: preserved_fields(exif),
            },
        }
    }

    pub(super) fn display_metadata(&self, dimensions: [u32; 2]) -> RawDisplayMetadata {
        RawDisplayMetadata {
            aperture: self.aperture,
            dimensions,
            iso_speed: self.capture.iso_speed,
            shutter_seconds: self.capture.shutter_seconds,
            focal_length: self.focal_length,
        }
    }

    pub(super) fn apply_to(self, raw: &mut LoadedRaw) {
        raw.camera_make = self.camera_make;
        raw.camera_model = self.camera_model;
        raw.lens_make = self.lens_make;
        raw.lens_model = self.lens_model;
        raw.focal_length = self.focal_length;
        raw.aperture = self.aperture;
        raw.capture_metadata = self.capture;
    }
}

/// Copies every source field an export can write back unchanged, GPS first.
///
/// The thumbnail IFD is skipped because exports carry no embedded thumbnail.
fn preserved_fields(exif: &Exif) -> Vec<PreservedExifField> {
    let mut primary = Vec::new();
    let mut exif_ifd = Vec::new();
    let mut gps = Vec::new();
    let mut interop = Vec::new();
    for field in exif.fields() {
        let Some(field) = preserved_field(field) else {
            continue;
        };
        match field.ifd {
            ExifIfd::Primary => primary.push(field),
            ExifIfd::Exif => exif_ifd.push(field),
            ExifIfd::Gps => gps.push(field),
            ExifIfd::Interop => interop.push(field),
        }
    }

    let mut output = Vec::new();
    let mut bytes = 0usize;
    for fields in [gps, exif_ifd, interop, primary] {
        for field in fields {
            let value_bytes = if field.value.len() > 4 {
                field.value.len().saturating_add(field.value.len() & 1)
            } else {
                0
            };
            let entry_bytes = 12usize.saturating_add(value_bytes);
            if bytes.saturating_add(entry_bytes) > MAX_PRESERVED_EXIF_BYTES {
                continue;
            }
            bytes = bytes.saturating_add(entry_bytes);
            output.push(field);
        }
    }
    output
}

fn preserved_field(field: &Field) -> Option<PreservedExifField> {
    if field.ifd_num != In::PRIMARY {
        return None;
    }
    let context = field.tag.context();
    let ifd = match context {
        ExifContext::Tiff => ExifIfd::Primary,
        ExifContext::Exif => ExifIfd::Exif,
        ExifContext::Gps => ExifIfd::Gps,
        ExifContext::Interop => ExifIfd::Interop,
        _ => return None,
    };
    let tag = field.tag.number();
    if !is_preserved_tag(context, tag) {
        return None;
    }
    let (field_type, count, value) = encode_value(&field.value)?;
    Some(PreservedExifField {
        ifd,
        tag,
        field_type,
        count,
        value,
    })
}

/// Tags that describe the source container or are rebuilt by CalibRaw.
///
/// The primary IFD of a RAW also describes its sensor image: strip and tile
/// offsets, compression and sample layout point into the source file and mean
/// nothing inside an export. DNG camera profiles and GeoTIFF georeferencing
/// describe the source file too. MakerNote offsets are relative to the source
/// container, the IFD pointers are rebuilt from the preserved IFDs, and XMP,
/// IPTC, Photoshop resources and ICC profiles are separate metadata blocks
/// that an EXIF payload must not carry.
fn is_preserved_tag(context: ExifContext, tag: u16) -> bool {
    match context {
        ExifContext::Tiff => !matches!(
            tag,
            0x0100 // ImageWidth, written from the export dimensions
            | 0x0101 // ImageLength
            | 0x0102 // BitsPerSample
            | 0x0103 // Compression
            | 0x0106 // PhotometricInterpretation
            | 0x0111 // StripOffsets
            | 0x0112 // Orientation, exports are upright
            | 0x0115 // SamplesPerPixel
            | 0x0116 // RowsPerStrip
            | 0x0117 // StripByteCounts
            | 0x0118 // MinSampleValue
            | 0x0119 // MaxSampleValue
            | 0x011c // PlanarConfiguration
            | 0x013d // Predictor
            | 0x0140 // ColorMap
            | 0x0142 // TileWidth
            | 0x0143 // TileLength
            | 0x0144 // TileOffsets
            | 0x0145 // TileByteCounts
            | 0x014a // SubIFDs
            | 0x0152 // ExtraSamples
            | 0x0153 // SampleFormat
            | 0x015b // JPEGTables
            | 0x0200..=0x0209 /* Old-style JPEG tables and offsets */
            | 0x0212 // YCbCrSubSampling
            | 0x02bc // XMP
            | 0x828d // CFARepeatPatternDim
            | 0x828e // CFAPattern
            | 0x8290 // KodakIFD pointer
            | 0x830e // GeoTIFF ModelPixelScale
            | 0x83bb // IPTC/NAA
            | 0x8482 // GeoTIFF ModelTiepoint
            | 0x85d8 // GeoTIFF ModelTransformation
            | 0x8649 // Photoshop ImageResources
            | 0x8769 // ExifIFDPointer
            | 0x8773 // ICC profile, embedded separately
            | 0x87af..=0x87b1 /* GeoTIFF keys and parameters */
            | 0x8825 // GPSInfoIFDPointer
            | 0xa480..=0xa481 /* GDAL metadata and nodata */
            | 0xc4a5 // PrintImageMatching
            | 0xc612..=0xc7ff /* DNG camera, profile and opcode tags */
            | 0xea1c // Microsoft padding
            | 0xea1d // Microsoft OffsetSchema
        ),
        ExifContext::Exif => !matches!(
            tag,
            0x927c // MakerNote, offsets relative to the source container
            | 0xa005 // InteropIFDPointer, rebuilt from the preserved IFD
            | 0xea1c // Microsoft padding
            | 0xea1d // Microsoft OffsetSchema
        ),
        ExifContext::Gps | ExifContext::Interop => true,
        _ => false,
    }
}

/// Re-encodes a kamadak-exif value as a little-endian TIFF field.
fn encode_value(value: &Value) -> Option<(u16, u32, Vec<u8>)> {
    let count = |len: usize| u32::try_from(len).ok();
    let encoded = match value {
        Value::Byte(values) => (1, count(values.len())?, values.clone()),
        Value::Ascii(values) => {
            let mut bytes = Vec::new();
            for value in values {
                bytes.extend_from_slice(value);
                bytes.push(0);
            }
            if values.is_empty() {
                bytes.push(0);
            }
            (2, count(bytes.len())?, bytes)
        }
        Value::Short(values) => (
            3,
            count(values.len())?,
            values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect(),
        ),
        Value::Long(values) => (
            4,
            count(values.len())?,
            values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect(),
        ),
        Value::Rational(values) => {
            let mut bytes = Vec::with_capacity(values.len().saturating_mul(8));
            for value in values {
                bytes.extend_from_slice(&value.num.to_le_bytes());
                bytes.extend_from_slice(&value.denom.to_le_bytes());
            }
            (5, count(values.len())?, bytes)
        }
        Value::SByte(values) => (
            6,
            count(values.len())?,
            values.iter().map(|value| *value as u8).collect(),
        ),
        Value::Undefined(values, _) => (7, count(values.len())?, values.clone()),
        Value::SShort(values) => (
            8,
            count(values.len())?,
            values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect(),
        ),
        Value::SLong(values) => (
            9,
            count(values.len())?,
            values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect(),
        ),
        Value::SRational(values) => {
            let mut bytes = Vec::with_capacity(values.len().saturating_mul(8));
            for value in values {
                bytes.extend_from_slice(&value.num.to_le_bytes());
                bytes.extend_from_slice(&value.denom.to_le_bytes());
            }
            (10, count(values.len())?, bytes)
        }
        Value::Float(values) => (
            11,
            count(values.len())?,
            values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect(),
        ),
        Value::Double(values) => (
            12,
            count(values.len())?,
            values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect(),
        ),
        Value::Unknown(..) => return None,
    };
    Some(encoded)
}

fn ascii(field: &Field) -> Option<String> {
    let Value::Ascii(values) = &field.value else {
        return None;
    };
    let value = std::str::from_utf8(values.first()?).ok()?.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn positive_number(field: &Field) -> Option<f32> {
    let value = match &field.value {
        Value::Rational(values) => values.first()?.to_f64(),
        Value::SRational(values) => values.first()?.to_f64(),
        _ => f64::from(field.value.get_uint(0)?),
    } as f32;
    (value.is_finite() && value > 0.0).then_some(value)
}
