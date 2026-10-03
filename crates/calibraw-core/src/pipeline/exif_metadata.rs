//! EXIF fields CalibRaw displays and carries into exports, read from any
//! container kamadak-exif understands (TIFF-based RAWs, JPEG, PNG, HEIF).

use super::raw_loader::{CaptureMetadata, LoadedRaw, RawDisplayMetadata};
use anyhow::{Context, Result};
use exif::{Exif, Field, In, Tag, Value};
use std::path::Path;

/// Files larger than this are not scanned for EXIF, so a damaged or hostile
/// file cannot force a huge metadata read.
const MAX_EXIF_SCAN_BYTES: u64 = 256_000_000;

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A little-endian TIFF/EXIF block with Make, an EXIF IFD holding ISO,
    /// exposure time, f-number and DateTimeOriginal.
    fn exif_block() -> Vec<u8> {
        fn entry(bytes: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: u32) {
            bytes.extend_from_slice(&tag.to_le_bytes());
            bytes.extend_from_slice(&kind.to_le_bytes());
            bytes.extend_from_slice(&count.to_le_bytes());
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let mut bytes = b"II*\0\x08\0\0\0".to_vec();
        // IFD0 at 8: Make (inline "Cam\0"), ExifIFD pointer -> 38.
        bytes.extend_from_slice(&2u16.to_le_bytes());
        entry(&mut bytes, 0x010f, 2, 4, u32::from_le_bytes(*b"Cam\0"));
        entry(&mut bytes, 0x8769, 4, 1, 38);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(bytes.len(), 38);
        // EXIF IFD at 38 with four entries; data follows at 38 + 2 + 48 + 4 = 92.
        bytes.extend_from_slice(&4u16.to_le_bytes());
        entry(&mut bytes, 0x829a, 5, 1, 92);
        entry(&mut bytes, 0x829d, 5, 1, 100);
        entry(&mut bytes, 0x8827, 3, 1, 800);
        entry(&mut bytes, 0x9003, 2, 20, 108);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(bytes.len(), 92);
        for value in [1u32, 250, 28, 10] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(b"2026:10:03 12:34:56\0");
        bytes
    }

    #[test]
    fn summary_reads_camera_capture_and_date_fields() {
        let exif = exif::Reader::new().read_raw(exif_block()).unwrap();
        let summary = ExifSummary::from_exif(&exif);
        assert_eq!(summary.camera_make, "Cam");
        assert_eq!(summary.capture.iso_speed, 800.0);
        assert!((summary.capture.shutter_seconds - 1.0 / 250.0).abs() < 1e-6);
        assert!((summary.aperture - 2.8).abs() < 1e-6);
        assert_eq!(
            summary.capture.exif_dates,
            vec![(0x9003, "2026:10:03 12:34:56".to_owned())]
        );
        let display = summary.display_metadata([40, 30]);
        assert_eq!(display.dimensions, [40, 30]);
        assert_eq!(display.iso_speed, 800.0);
    }

    #[test]
    fn missing_exif_reads_as_empty_metadata() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"not an image").unwrap();
        assert!(ExifSummary::read(file.path()).is_err());
        let summary = ExifSummary::read_or_default(file.path());
        assert!(summary.camera_model.is_empty());
        assert_eq!(summary.capture.iso_speed, 0.0);
    }
}
