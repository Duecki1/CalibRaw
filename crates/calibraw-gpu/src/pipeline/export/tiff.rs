//! Tiled TIFF writer.

use super::*;

pub(super) fn export_tiled_tiff(
    context: ExportContext<'_>,
    request: ExportRequest<'_>,
) -> Result<()> {
    validate_export_dimensions(request.output_width, request.output_height)?;

    let file = open_export_destination(request.output)
        .with_context(|| format!("create TIFF {}", request.output.path.display()))?;
    let mut writer = BufWriter::new(file);
    let row_format = tiff_row_format(request.bit_depth);
    let profile = tiff_embedded_profile(request.color);
    write_tiff_header(&mut writer, request, row_format, &profile)?;
    render_export_output(context, request, &mut writer, row_format)?;
    writer.flush().context("flush TIFF export")?;
    Ok(())
}

fn tiff_row_format(bit_depth: ExportBitDepth) -> ExportRowFormat {
    match bit_depth {
        ExportBitDepth::Eight => ExportRowFormat::Rgb8,
        ExportBitDepth::Sixteen => ExportRowFormat::Rgb16Le,
        ExportBitDepth::Float32Linear => ExportRowFormat::RgbF32Le,
    }
}

fn tiff_embedded_profile(color: &ResolvedExportColor) -> Vec<u8> {
    color.embedded_icc.clone().unwrap_or_else(built_in_srgb_icc)
}

pub(super) const TIFF_TARGET_STRIP_BYTES: u64 = 1024 * 1024;

#[derive(Clone)]
struct TiffEntry {
    tag: u16,
    field_type: u16,
    count: u32,
    data: Vec<u8>,
}

fn tiff_short(value: u16) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

fn tiff_long(value: u32) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

fn tiff_long_values(values: &[u32]) -> Result<Vec<u8>> {
    let capacity = values
        .len()
        .checked_mul(std::mem::size_of::<u32>())
        .context("TIFF LONG array size overflow")?;
    let mut data = Vec::new();
    data.try_reserve_exact(capacity)
        .context("reserve TIFF LONG array")?;
    for value in values {
        data.extend_from_slice(&value.to_le_bytes());
    }
    Ok(data)
}

fn tiff_ascii(value: &str) -> Vec<u8> {
    let mut bytes = value.as_bytes().to_vec();
    if bytes.last().copied() != Some(0) {
        bytes.push(0);
    }
    bytes
}

fn tiff_ascii_entry(tag: u16, value: &str) -> Result<TiffEntry> {
    let data = tiff_ascii(value);
    Ok(TiffEntry {
        tag,
        field_type: 2,
        count: u32::try_from(data.len()).context("TIFF ASCII tag is too large")?,
        data,
    })
}

pub(super) fn tiff_strip_layout(
    width: u32,
    height: u32,
    bits_per_sample: u16,
) -> Result<(u32, Vec<u32>)> {
    anyhow::ensure!(width > 0 && height > 0, "TIFF dimensions must be non-zero");
    anyhow::ensure!(
        matches!(bits_per_sample, 8 | 16 | 32),
        "unsupported TIFF bit depth"
    );

    let bytes_per_pixel = u64::from(bits_per_sample / 8)
        .checked_mul(3)
        .context("TIFF bytes-per-pixel overflow")?;
    let bytes_per_row = u64::from(width)
        .checked_mul(bytes_per_pixel)
        .context("TIFF row byte count overflow")?;
    let rows_per_strip = (TIFF_TARGET_STRIP_BYTES / bytes_per_row)
        .max(1)
        .min(u64::from(height));
    let rows_per_strip = u32::try_from(rows_per_strip).context("TIFF rows-per-strip overflow")?;
    let strip_count = height.div_ceil(rows_per_strip);
    let mut byte_counts = Vec::new();
    byte_counts
        .try_reserve_exact(strip_count as usize)
        .context("reserve TIFF strip byte counts")?;

    for strip in 0..strip_count {
        let first_row = strip
            .checked_mul(rows_per_strip)
            .context("TIFF strip row offset overflow")?;
        let rows = rows_per_strip.min(height - first_row);
        let bytes = u64::from(rows)
            .checked_mul(bytes_per_row)
            .context("TIFF strip byte count overflow")?;
        byte_counts.push(
            u32::try_from(bytes).context("individual TIFF strip exceeds classic TIFF limits")?,
        );
    }

    Ok((rows_per_strip, byte_counts))
}

pub(super) fn write_tiff_header<W: Write>(
    output: &mut W,
    request: ExportRequest<'_>,
    row_format: ExportRowFormat,
    profile: &[u8],
) -> Result<()> {
    let bits = match row_format {
        ExportRowFormat::Rgb8 => 8u16,
        ExportRowFormat::Rgb16Le => 16u16,
        ExportRowFormat::RgbF32Le => 32u16,
        _ => return Err(anyhow::anyhow!("unsupported TIFF row encoding")),
    };
    let sample_format = if row_format == ExportRowFormat::RgbF32Le {
        3u16
    } else {
        1u16
    };
    let bytes_per_pixel = u64::from(bits / 8) * 3;
    let pixel_bytes = u64::from(request.output_width)
        .checked_mul(u64::from(request.output_height))
        .and_then(|pixels| pixels.checked_mul(bytes_per_pixel))
        .context("TIFF pixel byte count overflow")?;
    anyhow::ensure!(
        pixel_bytes <= u64::from(u32::MAX),
        "TIFF pixel data exceeds classic TIFF's 4 GiB limit"
    );

    let (rows_per_strip, strip_byte_counts) =
        tiff_strip_layout(request.output_width, request.output_height, bits)?;
    let strip_count = u32::try_from(strip_byte_counts.len()).context("too many TIFF strips")?;
    let strip_offsets_placeholder = vec![
        0u8;
        strip_byte_counts
            .len()
            .checked_mul(std::mem::size_of::<u32>())
            .context("TIFF strip-offset array size overflow")?
    ];
    let software = tiff_ascii_entry(305, "CalibRaw 2.0")?;

    let mut entries = vec![
        TiffEntry {
            tag: 256,
            field_type: 4,
            count: 1,
            data: tiff_long(request.output_width),
        },
        TiffEntry {
            tag: 257,
            field_type: 4,
            count: 1,
            data: tiff_long(request.output_height),
        },
        TiffEntry {
            tag: 258,
            field_type: 3,
            count: 3,
            data: [bits.to_le_bytes(), bits.to_le_bytes(), bits.to_le_bytes()].concat(),
        },
        TiffEntry {
            tag: 259,
            field_type: 3,
            count: 1,
            data: tiff_short(1),
        },
        TiffEntry {
            tag: 262,
            field_type: 3,
            count: 1,
            data: tiff_short(2),
        },
        TiffEntry {
            tag: 273,
            field_type: 4,
            count: strip_count,
            data: strip_offsets_placeholder,
        },
        TiffEntry {
            tag: 274,
            field_type: 3,
            count: 1,
            data: tiff_short(1),
        },
        TiffEntry {
            tag: 277,
            field_type: 3,
            count: 1,
            data: tiff_short(3),
        },
        TiffEntry {
            tag: 278,
            field_type: 4,
            count: 1,
            data: tiff_long(rows_per_strip),
        },
        TiffEntry {
            tag: 279,
            field_type: 4,
            count: strip_count,
            data: tiff_long_values(&strip_byte_counts)?,
        },
        TiffEntry {
            tag: 284,
            field_type: 3,
            count: 1,
            data: tiff_short(1),
        },
        software,
        TiffEntry {
            tag: 339,
            field_type: 3,
            count: 3,
            data: [
                sample_format.to_le_bytes(),
                sample_format.to_le_bytes(),
                sample_format.to_le_bytes(),
            ]
            .concat(),
        },
        TiffEntry {
            tag: 34675,
            field_type: 7,
            count: u32::try_from(profile.len()).context("ICC profile is too large for TIFF")?,
            data: profile.to_vec(),
        },
    ];

    if request.keep_metadata {
        let description = request.metadata.description.trim();
        if !description.is_empty() {
            entries.push(tiff_ascii_entry(270, description)?);
        }
        for (tag, value) in &request.metadata.exif_dates {
            if *tag == 0x0132 {
                entries.push(tiff_ascii_entry(*tag, value)?);
            }
        }
        let dates = metadata::encode_date_ifd(request.metadata, 0);
        if dates.len() > 6 {
            entries.push(TiffEntry {
                tag: 0x8769,
                field_type: 4,
                count: 1,
                data: dates,
            });
        }
        if !request.metadata.camera_make.trim().is_empty() {
            entries.push(tiff_ascii_entry(271, request.metadata.camera_make.trim())?);
        }
        if !request.metadata.camera_model.trim().is_empty() {
            entries.push(tiff_ascii_entry(272, request.metadata.camera_model.trim())?);
        }
        if !request.metadata.artist.trim().is_empty() {
            entries.push(tiff_ascii_entry(315, request.metadata.artist.trim())?);
        }
    }

    entries.sort_by_key(|entry| entry.tag);
    let entry_count = u16::try_from(entries.len()).context("too many TIFF IFD entries")?;
    let ifd_size = 2usize
        .checked_add(
            entries
                .len()
                .checked_mul(12)
                .context("TIFF IFD size overflow")?,
        )
        .and_then(|value| value.checked_add(4))
        .context("TIFF IFD size overflow")?;
    let mut cursor = 8usize
        .checked_add(ifd_size)
        .context("TIFF header size overflow")?;
    let mut external_offsets = Vec::with_capacity(entries.len());
    for entry in &entries {
        if entry.data.len() > 4 {
            cursor = cursor
                .checked_add(3)
                .map(|value| value & !3)
                .context("TIFF metadata alignment overflow")?;
            external_offsets.push(Some(cursor));
            cursor = cursor
                .checked_add(entry.data.len())
                .context("TIFF metadata size overflow")?;
        } else {
            external_offsets.push(None);
        }
    }
    for (entry, offset) in entries.iter_mut().zip(&external_offsets) {
        if entry.tag == 0x8769 {
            entry.data = metadata::encode_date_ifd(
                request.metadata,
                u32::try_from(offset.context("missing TIFF EXIF offset")?)
                    .context("TIFF EXIF offset overflow")?,
            );
        }
    }
    cursor = cursor
        .checked_add(3)
        .map(|value| value & !3)
        .context("TIFF pixel alignment overflow")?;
    let pixel_offset =
        u32::try_from(cursor).context("TIFF header exceeds classic TIFF offset range")?;
    let total_len = u64::from(pixel_offset)
        .checked_add(pixel_bytes)
        .context("TIFF file size overflow")?;
    anyhow::ensure!(
        total_len <= u64::from(u32::MAX),
        "TIFF export exceeds classic TIFF's 4 GiB limit"
    );

    let mut strip_offsets = Vec::new();
    strip_offsets
        .try_reserve_exact(strip_byte_counts.len())
        .context("reserve TIFF strip offsets")?;
    let mut next_strip_offset = u64::from(pixel_offset);
    for byte_count in &strip_byte_counts {
        strip_offsets.push(
            u32::try_from(next_strip_offset).context("TIFF strip offset exceeds classic limits")?,
        );
        next_strip_offset = next_strip_offset
            .checked_add(u64::from(*byte_count))
            .context("TIFF strip offset overflow")?;
    }
    anyhow::ensure!(
        next_strip_offset == total_len,
        "TIFF strip layout does not cover the complete raster"
    );
    if let Some(strip_offsets_entry) = entries.iter_mut().find(|entry| entry.tag == 273) {
        strip_offsets_entry.data = tiff_long_values(&strip_offsets)?;
    }

    output.write_all(b"II").context("write TIFF byte order")?;
    output
        .write_all(&42u16.to_le_bytes())
        .context("write TIFF magic")?;
    output
        .write_all(&8u32.to_le_bytes())
        .context("write TIFF IFD offset")?;
    output
        .write_all(&entry_count.to_le_bytes())
        .context("write TIFF entry count")?;

    for (entry, external_offset) in entries.iter().zip(&external_offsets) {
        output
            .write_all(&entry.tag.to_le_bytes())
            .context("write TIFF tag")?;
        output
            .write_all(&entry.field_type.to_le_bytes())
            .context("write TIFF field type")?;
        output
            .write_all(&entry.count.to_le_bytes())
            .context("write TIFF field count")?;
        if let Some(offset) = external_offset {
            output
                .write_all(
                    &u32::try_from(*offset)
                        .context("TIFF metadata offset overflow")?
                        .to_le_bytes(),
                )
                .context("write TIFF value offset")?;
        } else {
            let mut inline = [0u8; 4];
            inline[..entry.data.len()].copy_from_slice(&entry.data);
            output
                .write_all(&inline)
                .context("write TIFF inline value")?;
        }
    }
    output
        .write_all(&0u32.to_le_bytes())
        .context("write TIFF next IFD")?;

    let mut written = 8usize
        .checked_add(ifd_size)
        .context("TIFF header write count overflow")?;
    for (entry, external_offset) in entries.iter().zip(external_offsets) {
        if let Some(offset) = external_offset {
            while written < offset {
                output.write_all(&[0]).context("pad TIFF metadata")?;
                written += 1;
            }
            output
                .write_all(&entry.data)
                .context("write TIFF metadata payload")?;
            written = written
                .checked_add(entry.data.len())
                .context("TIFF metadata write count overflow")?;
        }
    }
    while written < pixel_offset as usize {
        output.write_all(&[0]).context("pad TIFF pixel offset")?;
        written += 1;
    }
    Ok(())
}
