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
