//! Row formats: raster validation, output encoding and tile stitching.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExportRowFormat {
    Rgb8,
    Rgba8,
    Rgba16Be,
    Rgb16Le,
    RgbF32Le,
}

impl ExportRowFormat {
    pub(super) const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb8 => 3,
            Self::Rgba8 => 4,
            Self::Rgb16Le => 6,
            Self::Rgba16Be => 8,
            Self::RgbF32Le => 12,
        }
    }
}

/// Writes encoded output rows to one writer per slide, cutting each row into equal slides from
/// left to right. One writer receives the rows unchanged.
pub(super) struct SlideWriter<'a, W> {
    slides: &'a mut [W],
    /// Bytes of one slide's part of a row.
    slide_row_bytes: usize,
    /// The slide being written and how much of its row part it already has.
    current: usize,
    written: usize,
}

impl<'a, W> SlideWriter<'a, W> {
    pub(super) fn new(slides: &'a mut [W], slide_width: u32, row_format: ExportRowFormat) -> Self {
        Self {
            slides,
            slide_row_bytes: slide_width as usize * row_format.bytes_per_pixel(),
            current: 0,
            written: 0,
        }
    }
}

impl<W: Write> Write for SlideWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let slide_count = self.slides.len();
        let remaining = self.slide_row_bytes - self.written;
        let slide = self
            .slides
            .get_mut(self.current)
            .ok_or_else(|| std::io::Error::other("an export needs an output"))?;
        let written = slide.write(&bytes[..bytes.len().min(remaining)])?;
        self.written += written;
        if self.written == self.slide_row_bytes {
            self.written = 0;
            self.current = (self.current + 1) % slide_count;
        }
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.slides.iter_mut().try_for_each(Write::flush)
    }
}

pub(super) fn validate_linear_rgb_raster(bytes: &[u8], width: u32, height: u32) -> Result<&[f32]> {
    let expected_values = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(3))
        .context("linear RGB raster size overflow")?;
    let expected_bytes = expected_values
        .checked_mul(std::mem::size_of::<f32>() as u64)
        .context("linear RGB raster byte size overflow")?;
    anyhow::ensure!(
        u64::try_from(bytes.len()).unwrap_or(u64::MAX) == expected_bytes,
        "linear RGB raster length does not match its dimensions"
    );
    bytemuck::try_cast_slice(bytes)
        .map_err(|error| anyhow::anyhow!("map linear RGB raster: {error}"))
}

pub(super) fn validate_rgb_raster_len(bytes: &[u8], width: u32, height: u32) -> Result<()> {
    let expected = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(3))
        .context("RGB raster size overflow")?;
    anyhow::ensure!(
        u64::try_from(bytes.len()).unwrap_or(u64::MAX) == expected,
        "RGB raster length does not match its dimensions"
    );
    Ok(())
}

pub(super) fn encode_output_row(
    row: &[f32],
    transform: Option<&SrgbOutputTransform>,
    row_format: ExportRowFormat,
) -> Result<Vec<u8>> {
    anyhow::ensure!(
        row.len().is_multiple_of(3),
        "linear RGB row has an invalid length"
    );
    let pixels = row.len() / 3;
    let bytes = pixels
        .checked_mul(row_format.bytes_per_pixel())
        .context("encoded row overflow")?;
    let mut encoded = Vec::new();
    encoded
        .try_reserve_exact(bytes)
        .context("reserve encoded export row")?;

    for rgb in row.chunks_exact(3) {
        anyhow::ensure!(
            rgb.iter().all(|value| value.is_finite()),
            "export contains NaN or infinity"
        );
        if row_format == ExportRowFormat::RgbF32Le {
            for value in rgb {
                encoded.extend_from_slice(&value.to_le_bytes());
            }
            continue;
        }

        let transform = transform.context("integer export requires an output color transform")?;
        let device = transform.transform_rgb([rgb[0], rgb[1], rgb[2]]);
        match row_format {
            ExportRowFormat::Rgb8 | ExportRowFormat::Rgba8 => {
                for value in device {
                    encoded.push((value.clamp(0.0, 1.0) * 255.0).round() as u8);
                }
                if row_format == ExportRowFormat::Rgba8 {
                    encoded.push(255);
                }
            }
            ExportRowFormat::Rgba16Be | ExportRowFormat::Rgb16Le => {
                for value in device {
                    let sample = (value.clamp(0.0, 1.0) * 65_535.0).round() as u16;
                    if row_format == ExportRowFormat::Rgb16Le {
                        encoded.extend_from_slice(&sample.to_le_bytes());
                    } else {
                        encoded.extend_from_slice(&sample.to_be_bytes());
                    }
                }
                if row_format == ExportRowFormat::Rgba16Be {
                    encoded.extend_from_slice(&u16::MAX.to_be_bytes());
                }
            }
            ExportRowFormat::RgbF32Le => unreachable!(),
        }
    }
    Ok(encoded)
}

pub(super) fn stitch_linear_tile_into_band(
    band: &mut [f32],
    source_width: u32,
    band_y: u32,
    tile: crate::pipeline::ExportTile,
    rgb: &[f32],
) -> Result<()> {
    anyhow::ensure!(tile.core_y == band_y, "tile is in the wrong export band");
    let source_row_values = checked_rgb_len(tile.core_width, 1)?;
    anyhow::ensure!(
        rgb.len() == checked_rgb_len(tile.core_width, tile.core_height)?,
        "GPU tile readback length does not match tile dimensions"
    );
    for row in 0..tile.core_height as usize {
        let source_start = row
            .checked_mul(source_row_values)
            .context("tile source row overflow")?;
        let destination_pixel = row
            .checked_mul(source_width as usize)
            .and_then(|value| value.checked_add(tile.core_x as usize))
            .context("tile destination row overflow")?;
        let destination_start = destination_pixel
            .checked_mul(3)
            .context("tile destination channel overflow")?;
        let destination_end = destination_start
            .checked_add(source_row_values)
            .context("tile destination end overflow")?;
        band.get_mut(destination_start..destination_end)
            .context("tile destination is outside its export band")?
            .copy_from_slice(&rgb[source_start..source_start + source_row_values]);
    }
    Ok(())
}

pub(super) fn checked_rgb_len(width: u32, height: u32) -> Result<usize> {
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .context("image pixel count overflow")?;
    let values = pixels.checked_mul(3).context("RGB value count overflow")?;
    usize::try_from(values).context("RGB allocation does not fit this platform")
}
