use super::*;
use calibraw_core::pipeline::{ExifIfd, PreservedExifField};

pub(super) fn add_png_text_metadata<W: Write>(
    encoder: &mut png::Encoder<'_, W>,
    metadata: &ExportMetadata,
    output_width: u32,
    output_height: u32,
) -> Result<()> {
    encoder
        .add_itxt_chunk("Software".to_owned(), "CalibRaw 2.0".to_owned())
        .context("write PNG software metadata")?;
    if let Some(source) = metadata
        .source_file_name
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        encoder
            .add_itxt_chunk("Source".to_owned(), source.to_owned())
            .context("write PNG source metadata")?;
    }
    let camera = joined_metadata_label(&metadata.camera_make, &metadata.camera_model);
    if !camera.is_empty() {
        encoder
            .add_itxt_chunk("Camera".to_owned(), camera)
            .context("write PNG camera metadata")?;
    }
    let lens = joined_metadata_label(&metadata.lens_make, &metadata.lens_model);
    if !lens.is_empty() {
        encoder
            .add_itxt_chunk("Lens".to_owned(), lens)
            .context("write PNG lens metadata")?;
    }
    if metadata.focal_length.is_finite() && metadata.focal_length > 0.0 {
        encoder
            .add_itxt_chunk(
                "Focal length".to_owned(),
                format!("{:.1} mm", metadata.focal_length),
            )
            .context("write PNG focal-length metadata")?;
    }
    if metadata.aperture.is_finite() && metadata.aperture > 0.0 {
        encoder
            .add_itxt_chunk("Aperture".to_owned(), format!("f/{:.1}", metadata.aperture))
            .context("write PNG aperture metadata")?;
    }
    if metadata.focus_distance.is_finite() && metadata.focus_distance > 0.0 {
        encoder
            .add_itxt_chunk(
                "Focus distance".to_owned(),
                format!("{:.2} m", metadata.focus_distance),
            )
            .context("write PNG focus-distance metadata")?;
    }
    if metadata.iso_speed.is_finite() && metadata.iso_speed > 0.0 {
        encoder
            .add_itxt_chunk("ISO speed".to_owned(), format!("{:.0}", metadata.iso_speed))
            .context("write PNG ISO metadata")?;
    }
    if metadata.shutter_seconds.is_finite() && metadata.shutter_seconds > 0.0 {
        encoder
            .add_itxt_chunk(
                "Exposure time".to_owned(),
                format_exposure_time(metadata.shutter_seconds),
            )
            .context("write PNG exposure-time metadata")?;
    }
    if !metadata.artist.trim().is_empty() {
        encoder
            .add_itxt_chunk("Artist".to_owned(), metadata.artist.trim().to_owned())
            .context("write PNG artist metadata")?;
    }
    if !metadata.description.trim().is_empty() {
        encoder
            .add_itxt_chunk(
                "Image description".to_owned(),
                metadata.description.trim().to_owned(),
            )
            .context("write PNG image-description metadata")?;
    }
    encoder
        .add_itxt_chunk(
            "Original dimensions".to_owned(),
            format!("{}x{}", metadata.source_width, metadata.source_height),
        )
        .context("write original dimensions metadata")?;
    encoder
        .add_itxt_chunk(
            "Export dimensions".to_owned(),
            format!("{output_width}x{output_height}"),
        )
        .context("write export dimensions metadata")?;
    encoder
        .add_itxt_chunk("Orientation".to_owned(), "1 (normal)".to_owned())
        .context("write PNG orientation metadata")?;
    Ok(())
}

fn format_exposure_time(seconds: f32) -> String {
    if seconds > 0.0 && seconds < 1.0 {
        let reciprocal = (1.0 / seconds).round().max(1.0);
        if ((1.0 / reciprocal) - seconds).abs() <= seconds * 0.02 {
            return format!("1/{reciprocal:.0} s");
        }
    }
    format!("{seconds:.4} s")
}

fn joined_metadata_label(make: &str, model: &str) -> String {
    match (make.trim(), model.trim()) {
        ("", "") => String::new(),
        ("", model) => model.to_owned(),
        (make, "") => make.to_owned(),
        (make, model) if model.starts_with(make) => model.to_owned(),
        (make, model) => format!("{make} {model}"),
    }
}

#[derive(Clone)]
enum ExifValue {
    Short(u16),
    Long(u32),
    Ascii(Vec<u8>),
    Rational(u32, u32),
    Undefined(Vec<u8>),
    /// A source field, already in little-endian TIFF encoding.
    Preserved {
        field_type: u16,
        count: u32,
        value: Vec<u8>,
    },
}

#[derive(Clone)]
struct ExifEntry {
    tag: u16,
    value: ExifValue,
}

fn nul_terminated_exif_ascii(value: &str) -> Vec<u8> {
    let mut output = value
        .chars()
        .map(|character| {
            if character.is_ascii() && character != '\0' {
                character as u8
            } else {
                b'?'
            }
        })
        .collect::<Vec<_>>();
    output.push(0);
    output
}

fn exif_rational(value: f32) -> Option<(u32, u32)> {
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    let denominator = 10_000u32;
    let numerator = (f64::from(value) * f64::from(denominator))
        .round()
        .clamp(1.0, f64::from(u32::MAX)) as u32;
    let divisor = greatest_common_divisor(numerator, denominator);
    Some((numerator / divisor, denominator / divisor))
}

fn greatest_common_divisor(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left.max(1)
}

fn exif_value_parts(value: &ExifValue) -> (u16, u32, Vec<u8>) {
    match value {
        ExifValue::Short(value) => (3, 1, value.to_le_bytes().to_vec()),
        ExifValue::Long(value) => (4, 1, value.to_le_bytes().to_vec()),
        ExifValue::Ascii(value) => (2, value.len() as u32, value.clone()),
        ExifValue::Rational(numerator, denominator) => {
            let mut bytes = Vec::with_capacity(8);
            bytes.extend_from_slice(&numerator.to_le_bytes());
            bytes.extend_from_slice(&(*denominator).max(1).to_le_bytes());
            (5, 1, bytes)
        }
        ExifValue::Undefined(value) => (7, value.len() as u32, value.clone()),
        ExifValue::Preserved {
            field_type,
            count,
            value,
        } => (*field_type, *count, value.clone()),
    }
}

fn encoded_ifd_block_len(entries: &[ExifEntry]) -> u32 {
    let directory_len = 2usize
        .saturating_add(entries.len().saturating_mul(12))
        .saturating_add(4);
    let data_len = entries
        .iter()
        .map(|entry| {
            let (_, _, bytes) = exif_value_parts(&entry.value);
            if bytes.len() <= 4 {
                0
            } else {
                bytes.len() + (bytes.len() & 1)
            }
        })
        .sum::<usize>();
    u32::try_from(directory_len.saturating_add(data_len)).unwrap_or(u32::MAX)
}

fn encode_ifd_block(entries: &[ExifEntry], ifd_offset: u32) -> Vec<u8> {
    let directory_len = 2usize + entries.len() * 12 + 4;
    let data_offset = ifd_offset.saturating_add(directory_len as u32);
    let mut directory = Vec::with_capacity(directory_len);
    let mut data = Vec::new();
    directory.extend_from_slice(&(entries.len() as u16).to_le_bytes());

    for entry in entries {
        directory.extend_from_slice(&entry.tag.to_le_bytes());
        let (field_type, count, bytes) = exif_value_parts(&entry.value);
        directory.extend_from_slice(&field_type.to_le_bytes());
        directory.extend_from_slice(&count.to_le_bytes());
        if bytes.len() <= 4 {
            directory.extend_from_slice(&bytes);
            directory.resize(directory.len() + 4 - bytes.len(), 0);
        } else {
            let value_offset = data_offset.saturating_add(data.len() as u32);
            directory.extend_from_slice(&value_offset.to_le_bytes());
            data.extend_from_slice(&bytes);
            if data.len() & 1 != 0 {
                data.push(0);
            }
        }
    }
    directory.extend_from_slice(&0u32.to_le_bytes());
    directory.extend_from_slice(&data);
    directory
}

fn is_ifd_pointer(tag: u16) -> bool {
    matches!(tag, 0x8769 | 0x8825 | 0xa005)
}

fn preserved_entry(field: &PreservedExifField) -> ExifEntry {
    ExifEntry {
        tag: field.tag,
        value: ExifValue::Preserved {
            field_type: field.field_type,
            count: field.count,
            value: field.value.clone(),
        },
    }
}

/// Adds the source fields for `ifd` unless CalibRaw writes that tag itself.
fn add_preserved(entries: &mut Vec<ExifEntry>, metadata: &ExportMetadata, ifd: ExifIfd) {
    for field in &metadata.preserved_exif {
        if field.ifd != ifd || is_ifd_pointer(field.tag) {
            continue;
        }
        if entries.iter().any(|entry| entry.tag == field.tag) {
            continue;
        }
        entries.push(preserved_entry(field));
    }
}

fn set_pointer(entries: &mut [ExifEntry], tag: u16, offset: u32) {
    if let Some(pointer) = entries.iter_mut().find(|entry| entry.tag == tag) {
        pointer.value = ExifValue::Long(offset);
    }
}

fn ifd0_entries(
    metadata: &ExportMetadata,
    output_width: u32,
    output_height: u32,
    has_gps: bool,
) -> Vec<ExifEntry> {
    let mut entries = vec![
        ExifEntry {
            tag: 0x0100,
            value: ExifValue::Long(output_width),
        },
        ExifEntry {
            tag: 0x0101,
            value: ExifValue::Long(output_height),
        },
        ExifEntry {
            tag: 0x0112,
            value: ExifValue::Short(1),
        },
        ExifEntry {
            tag: 0x0131,
            value: ExifValue::Ascii(nul_terminated_exif_ascii("CalibRaw 2.0")),
        },
    ];
    if !metadata.description.trim().is_empty() {
        entries.push(ExifEntry {
            tag: 0x010e,
            value: ExifValue::Ascii(nul_terminated_exif_ascii(metadata.description.trim())),
        });
    }
    for (tag, value) in &metadata.exif_dates {
        if *tag == 0x0132 {
            entries.push(ExifEntry {
                tag: *tag,
                value: ExifValue::Ascii(nul_terminated_exif_ascii(value)),
            });
        }
    }
    if !metadata.camera_make.trim().is_empty() {
        entries.push(ExifEntry {
            tag: 0x010f,
            value: ExifValue::Ascii(nul_terminated_exif_ascii(&metadata.camera_make)),
        });
    }
    if !metadata.camera_model.trim().is_empty() {
        entries.push(ExifEntry {
            tag: 0x0110,
            value: ExifValue::Ascii(nul_terminated_exif_ascii(&metadata.camera_model)),
        });
    }
    if !metadata.artist.trim().is_empty() {
        entries.push(ExifEntry {
            tag: 0x013b,
            value: ExifValue::Ascii(nul_terminated_exif_ascii(&metadata.artist)),
        });
    }
    add_preserved(&mut entries, metadata, ExifIfd::Primary);
    entries.push(ExifEntry {
        tag: 0x8769,
        value: ExifValue::Long(0),
    });
    if has_gps {
        entries.push(ExifEntry {
            tag: 0x8825,
            value: ExifValue::Long(0),
        });
    }
    entries.sort_by_key(|entry| entry.tag);
    entries
}

fn exif_ifd_entries(
    metadata: &ExportMetadata,
    output_width: u32,
    output_height: u32,
    has_interop: bool,
) -> Vec<ExifEntry> {
    let mut entries = vec![
        ExifEntry {
            tag: 0x9000,
            value: ExifValue::Undefined(b"0232".to_vec()),
        },
        ExifEntry {
            tag: 0xa002,
            value: ExifValue::Long(output_width),
        },
        ExifEntry {
            tag: 0xa003,
            value: ExifValue::Long(output_height),
        },
    ];
    if let Some((numerator, denominator)) = exif_rational(metadata.shutter_seconds) {
        entries.push(ExifEntry {
            tag: 0x829a,
            value: ExifValue::Rational(numerator, denominator),
        });
    }
    if metadata.iso_speed.is_finite() && metadata.iso_speed > 0.0 {
        let iso = metadata.iso_speed.round().clamp(1.0, u32::MAX as f32) as u32;
        entries.push(ExifEntry {
            tag: 0x8827,
            value: if iso <= u32::from(u16::MAX) {
                ExifValue::Short(iso as u16)
            } else {
                ExifValue::Long(iso)
            },
        });
    }
    if let Some((numerator, denominator)) = exif_rational(metadata.aperture) {
        entries.push(ExifEntry {
            tag: 0x829d,
            value: ExifValue::Rational(numerator, denominator),
        });
    }
    if let Some((numerator, denominator)) = exif_rational(metadata.focal_length) {
        entries.push(ExifEntry {
            tag: 0x920a,
            value: ExifValue::Rational(numerator, denominator),
        });
    }
    if let Some((numerator, denominator)) = exif_rational(metadata.focus_distance) {
        entries.push(ExifEntry {
            tag: 0x9206,
            value: ExifValue::Rational(numerator, denominator),
        });
    }
    if !metadata.lens_make.trim().is_empty() {
        entries.push(ExifEntry {
            tag: 0xa433,
            value: ExifValue::Ascii(nul_terminated_exif_ascii(&metadata.lens_make)),
        });
    }
    if !metadata.lens_model.trim().is_empty() {
        entries.push(ExifEntry {
            tag: 0xa434,
            value: ExifValue::Ascii(nul_terminated_exif_ascii(&metadata.lens_model)),
        });
    }
    entries.extend(date_entries(metadata));
    add_preserved(&mut entries, metadata, ExifIfd::Exif);
    if has_interop {
        entries.push(ExifEntry {
            tag: 0xa005,
            value: ExifValue::Long(0),
        });
    }
    entries.sort_by_key(|entry| entry.tag);
    entries
}

fn gps_ifd_entries(metadata: &ExportMetadata) -> Vec<ExifEntry> {
    let mut entries = Vec::new();
    add_preserved(&mut entries, metadata, ExifIfd::Gps);
    entries.sort_by_key(|entry| entry.tag);
    entries
}

fn interop_ifd_entries(metadata: &ExportMetadata) -> Vec<ExifEntry> {
    let mut entries = Vec::new();
    add_preserved(&mut entries, metadata, ExifIfd::Interop);
    entries.sort_by_key(|entry| entry.tag);
    entries
}

/// The Exif and Interoperability IFDs, laid out back to back.
struct ExifIfds {
    exif: Vec<ExifEntry>,
    interop: Vec<ExifEntry>,
}

impl ExifIfds {
    fn from_metadata(metadata: &ExportMetadata, output_width: u32, output_height: u32) -> Self {
        let interop = interop_ifd_entries(metadata);
        let exif = exif_ifd_entries(metadata, output_width, output_height, !interop.is_empty());
        Self { exif, interop }
    }

    fn encoded_len(&self) -> u32 {
        let interop_len = if self.interop.is_empty() {
            0
        } else {
            encoded_ifd_block_len(&self.interop)
        };
        encoded_ifd_block_len(&self.exif).saturating_add(interop_len)
    }

    fn encode(&self, offset: u32) -> Vec<u8> {
        let mut exif = self.exif.clone();
        let exif_len = encoded_ifd_block_len(&exif);
        if !self.interop.is_empty() {
            set_pointer(&mut exif, 0xa005, offset.saturating_add(exif_len));
        }
        let mut output = encode_ifd_block(&exif, offset);
        if !self.interop.is_empty() {
            output.extend_from_slice(&encode_ifd_block(
                &self.interop,
                offset.saturating_add(exif_len),
            ));
        }
        output
    }
}

pub(super) fn build_exif_payload(
    metadata: &ExportMetadata,
    output_width: u32,
    output_height: u32,
) -> Vec<u8> {
    let exif_ifds = ExifIfds::from_metadata(metadata, output_width, output_height);
    let gps = gps_ifd_entries(metadata);
    let has_gps = !gps.is_empty();
    let mut ifd0 = ifd0_entries(metadata, output_width, output_height, has_gps);

    let ifd0_offset = 8u32;
    let exif_offset = ifd0_offset.saturating_add(encoded_ifd_block_len(&ifd0));
    let gps_offset = exif_offset.saturating_add(exif_ifds.encoded_len());
    set_pointer(&mut ifd0, 0x8769, exif_offset);
    if has_gps {
        set_pointer(&mut ifd0, 0x8825, gps_offset);
    }

    let gps_len = if has_gps {
        encoded_ifd_block_len(&gps)
    } else {
        0
    };
    let capacity = 8usize
        .saturating_add(encoded_ifd_block_len(&ifd0) as usize)
        .saturating_add(exif_ifds.encoded_len() as usize)
        .saturating_add(gps_len as usize);
    let mut output = Vec::with_capacity(capacity);
    output.extend_from_slice(b"II");
    output.extend_from_slice(&42u16.to_le_bytes());
    output.extend_from_slice(&ifd0_offset.to_le_bytes());
    output.extend_from_slice(&encode_ifd_block(&ifd0, ifd0_offset));
    output.extend_from_slice(&exif_ifds.encode(exif_offset));
    if has_gps {
        output.extend_from_slice(&encode_ifd_block(&gps, gps_offset));
    }
    output
}

pub(super) fn exif_ifd_encoded_len(
    metadata: &ExportMetadata,
    output_width: u32,
    output_height: u32,
) -> u32 {
    ExifIfds::from_metadata(metadata, output_width, output_height).encoded_len()
}

pub(super) fn encode_exif_ifd(
    metadata: &ExportMetadata,
    offset: u32,
    output_width: u32,
    output_height: u32,
) -> Vec<u8> {
    ExifIfds::from_metadata(metadata, output_width, output_height).encode(offset)
}

pub(super) fn gps_ifd_encoded_len(metadata: &ExportMetadata) -> u32 {
    let entries = gps_ifd_entries(metadata);
    if entries.is_empty() {
        0
    } else {
        encoded_ifd_block_len(&entries)
    }
}

pub(super) fn encode_gps_ifd(metadata: &ExportMetadata, offset: u32) -> Vec<u8> {
    let entries = gps_ifd_entries(metadata);
    if entries.is_empty() {
        Vec::new()
    } else {
        encode_ifd_block(&entries, offset)
    }
}

fn date_entries(metadata: &ExportMetadata) -> Vec<ExifEntry> {
    metadata
        .exif_dates
        .iter()
        .filter(|(tag, _)| matches!(tag, 0x9003 | 0x9004 | 0x9010..=0x9012 | 0x9290..=0x9292))
        .map(|(tag, value)| ExifEntry {
            tag: *tag,
            value: ExifValue::Ascii(nul_terminated_exif_ascii(value)),
        })
        .collect()
}
