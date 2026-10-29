//! Matching AI-denoised images to crops, proxies and padded tiles.

use super::*;

pub(super) fn crop_ai_denoised(
    raw: &LoadedRaw,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Option<crate::pipeline::AiDenoisedImage> {
    let source = raw.ai_denoised_image()?;
    if let Some(source_cfa) = source.bayer_cfa() {
        let elements = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|count| usize::try_from(count).ok())?;
        let mut raw_cfa16 = Vec::new();
        raw_cfa16.try_reserve_exact(elements).ok()?;
        for row in y..y + height {
            let start = (row * raw.width + x) as usize;
            let end = start + width as usize;
            raw_cfa16.extend_from_slice(&source_cfa[start..end]);
        }
        return crate::pipeline::AiDenoisedImage::new_bayer_cfa(width, height, raw_cfa16).ok();
    }
    let source_rgb = source.camera_rgb16f()?;
    let elements = u64::from(width)
        .checked_mul(u64::from(height))?
        .checked_mul(3)
        .and_then(|count| usize::try_from(count).ok())?;
    let mut rgb16f = Vec::new();
    rgb16f.try_reserve_exact(elements).ok()?;
    for row in y..y + height {
        let start = ((row * raw.width + x) * 3) as usize;
        let end = start + width as usize * 3;
        rgb16f.extend_from_slice(&source_rgb[start..end]);
    }
    crate::pipeline::AiDenoisedImage::new(width, height, rgb16f).ok()
}

pub(super) fn proportional_partition(
    index: u32,
    source_count: u32,
    output_count: u32,
) -> (u32, u32) {
    let start = (u64::from(index) * u64::from(source_count) / u64::from(output_count)) as u32;
    let end = (u64::from(index + 1) * u64::from(source_count) / u64::from(output_count)) as u32;
    (start, end.max(start + 1).min(source_count))
}

/// The demosaiced AI-denoised scene (X-Trans) of `raw`'s region, box-averaged
/// to the proxy's size.
pub(super) fn proxy_ai_denoised(
    raw: &LoadedRaw,
    x: u32,
    y: u32,
    region_width: u32,
    region_height: u32,
    output_width: u32,
    output_height: u32,
) -> Option<crate::pipeline::AiDenoisedImage> {
    // A Bayer result is binned with its mosaic (`bin_mosaic`).
    let source = raw.ai_denoised_image()?;
    let source_rgb = source.camera_rgb16f()?;
    let elements = u64::from(output_width)
        .checked_mul(u64::from(output_height))?
        .checked_mul(3)
        .and_then(|count| usize::try_from(count).ok())?;
    let mut rgb16f = vec![0u16; elements];
    use half::f16;
    rgb16f
        .par_chunks_mut(output_width as usize * 3)
        .enumerate()
        .for_each(|(output_y, row)| {
            let source_y0 = y
                + ((output_y as u64 * u64::from(region_height)) / u64::from(output_height)) as u32;
            let source_y1 = y
                + (((output_y as u64 + 1) * u64::from(region_height))
                    .div_ceil(u64::from(output_height))) as u32;
            let source_y1 = source_y1.min(y + region_height).max(source_y0 + 1);
            for output_x in 0..output_width {
                let source_x0 = x
                    + ((u64::from(output_x) * u64::from(region_width)) / u64::from(output_width))
                        as u32;
                let source_x1 = x
                    + ((u64::from(output_x + 1) * u64::from(region_width))
                        .div_ceil(u64::from(output_width))) as u32;
                let source_x1 = source_x1.min(x + region_width).max(source_x0 + 1);
                let mut sum = [0.0f64; 3];
                let mut count = 0u32;
                for source_y in source_y0..source_y1 {
                    for source_x in source_x0..source_x1 {
                        let index = ((source_y * raw.width + source_x) * 3) as usize;
                        for channel in 0..3 {
                            sum[channel] +=
                                f64::from(f16::from_bits(source_rgb[index + channel]).to_f32());
                        }
                        count += 1;
                    }
                }
                let destination = output_x as usize * 3;
                for channel in 0..3 {
                    row[destination + channel] =
                        f16::from_f32((sum[channel] / f64::from(count.max(1))) as f32).to_bits();
                }
            }
        });
    crate::pipeline::AiDenoisedImage::new(output_width, output_height, rgb16f).ok()
}

pub(super) fn padded_tile_ai_denoised(
    raw: &LoadedRaw,
    tile: ExportTile,
) -> Option<crate::pipeline::AiDenoisedImage> {
    let source = raw.ai_denoised_image()?;
    if let Some(source_cfa) = source.bayer_cfa() {
        let elements = u64::from(tile.padded_width)
            .checked_mul(u64::from(tile.padded_height))
            .and_then(|count| usize::try_from(count).ok())?;
        let mut raw_cfa16 = vec![0u16; elements];
        let max_x = i64::from(raw.width.saturating_sub(1));
        let max_y = i64::from(raw.height.saturating_sub(1));
        for local_y in 0..tile.padded_height {
            let source_y =
                (i64::from(tile.global_origin_y) + i64::from(local_y)).clamp(0, max_y) as u32;
            for local_x in 0..tile.padded_width {
                let source_x =
                    (i64::from(tile.global_origin_x) + i64::from(local_x)).clamp(0, max_x) as u32;
                raw_cfa16[(local_y * tile.padded_width + local_x) as usize] =
                    source_cfa[(source_y * raw.width + source_x) as usize];
            }
        }
        return crate::pipeline::AiDenoisedImage::new_bayer_cfa(
            tile.padded_width,
            tile.padded_height,
            raw_cfa16,
        )
        .ok();
    }
    let source_rgb = source.camera_rgb16f()?;
    let elements = u64::from(tile.padded_width)
        .checked_mul(u64::from(tile.padded_height))?
        .checked_mul(3)
        .and_then(|count| usize::try_from(count).ok())?;
    let mut rgb16f = vec![0u16; elements];
    let max_x = i64::from(raw.width.saturating_sub(1));
    let max_y = i64::from(raw.height.saturating_sub(1));
    for local_y in 0..tile.padded_height {
        let source_y =
            (i64::from(tile.global_origin_y) + i64::from(local_y)).clamp(0, max_y) as u32;
        for local_x in 0..tile.padded_width {
            let source_x =
                (i64::from(tile.global_origin_x) + i64::from(local_x)).clamp(0, max_x) as u32;
            let source_index = ((source_y * raw.width + source_x) * 3) as usize;
            let destination_index = ((local_y * tile.padded_width + local_x) * 3) as usize;
            rgb16f[destination_index..destination_index + 3]
                .copy_from_slice(&source_rgb[source_index..source_index + 3]);
        }
    }
    crate::pipeline::AiDenoisedImage::new(tile.padded_width, tile.padded_height, rgb16f).ok()
}

pub(super) const HIGHLIGHT_RECONSTRUCTION_SUPPORT: u32 = 1;
