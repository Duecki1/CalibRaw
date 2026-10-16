//! Tile geometry for denoising: reflected borders, seam blending and gain matching.

use super::*;

pub(super) fn match_gain_tile(input: &[f32], output: &mut [f32]) -> Result<f32> {
    anyhow::ensure!(
        !input.is_empty() && !output.is_empty(),
        "RawNIND gain matching received no pixels"
    );
    let input_mean = input.iter().map(|value| f64::from(*value)).sum::<f64>() / input.len() as f64;
    let output_mean =
        output.iter().map(|value| f64::from(*value)).sum::<f64>() / output.len() as f64;
    let threshold = 1e-6 * input_mean.abs();
    anyhow::ensure!(
        input_mean.is_finite() && output_mean.is_finite() && output_mean.abs() > threshold,
        "RawNIND returned a degenerate mean"
    );
    let gain = (input_mean / output_mean) as f32;
    anyhow::ensure!(
        gain.is_finite() && gain.abs() <= 10_000.0,
        "RawNIND returned an implausible gain {gain}"
    );
    let mut maximum_abs = 0.0f32;
    for value in output {
        *value *= gain;
        maximum_abs = maximum_abs.max(value.abs());
    }
    anyhow::ensure!(
        maximum_abs.is_finite() && maximum_abs <= MAX_MODEL_ABS,
        "RawNIND gain-matched output is divergent (max |value| {maximum_abs})"
    );
    Ok(gain)
}

pub(super) fn bayer_rggb_origin(raw: &LoadedRaw) -> Result<(u32, u32)> {
    anyhow::ensure!(
        raw.width >= 2 && raw.height >= 2,
        "Bayer RAW dimensions are too small"
    );
    for y in 0..2 {
        for x in 0..2 {
            if raw.color_indices[(y * raw.width + x) as usize] != 0 {
                continue;
            }
            let color = |dx: u32, dy: u32| {
                raw.color_indices[(((y + dy) % 2) * raw.width + (x + dx) % 2) as usize]
            };
            if matches!(color(1, 0), 1 | 3) && matches!(color(0, 1), 1 | 3) && color(1, 1) == 2 {
                return Ok((x, y));
            }
        }
    }
    Err(anyhow::anyhow!(
        "RawNIND Bayer path requires a canonical R/G/B 2x2 CFA"
    ))
}

pub(super) fn normalized_sensor_site(raw: &LoadedRaw, x: u32, y: u32) -> f32 {
    let index = (y * raw.width + x) as usize;
    let channel = usize::from(raw.color_indices[index]).min(3);
    let black = raw.black_levels_per_pixel[index];
    let white = raw.white_levels[channel].max(black + 1.0);
    ((f32::from(raw.raw_pixels[index]) - black) / (white - black)).clamp(0.0, 1.0)
}

pub(super) fn reflect_index(index: i64, length: usize) -> usize {
    if length <= 1 {
        return 0;
    }
    let period = (length as i64 - 1) * 2;
    let folded = index.rem_euclid(period);
    if folded < length as i64 {
        folded as usize
    } else {
        (period - folded) as usize
    }
}

fn seam_ramp(distance: usize, overlap: usize) -> f32 {
    (distance as f32 + 0.5) / (2 * overlap) as f32
}

pub(super) fn seam_weight(
    coordinate: usize,
    core_start: usize,
    core_end: usize,
    overlap: usize,
    has_before: bool,
    has_after: bool,
) -> f32 {
    if has_before && coordinate < core_start + overlap {
        return seam_ramp(coordinate - (core_start - overlap), overlap);
    }
    if has_after && coordinate >= core_end - overlap {
        return 1.0 - seam_ramp(coordinate - (core_end - overlap), overlap);
    }
    1.0
}

pub(super) fn accumulate_half_rgb(
    stored: &mut [u16],
    destination: usize,
    rgb: [f32; 3],
    weight: f32,
    phase: &str,
) -> Result<()> {
    anyhow::ensure!(
        weight.is_finite() && (0.0..=1.0).contains(&weight),
        "{phase} produced an invalid overlap weight"
    );
    for channel in 0..3 {
        let contribution = rgb[channel] * weight;
        let previous = half::f16::from_bits(stored[destination + channel]).to_f32();
        let value = previous + contribution;
        anyhow::ensure!(
            value.is_finite() && value.abs() <= half::f16::MAX.to_f32(),
            "{phase} produced a divergent value"
        );
        stored[destination + channel] = half::f16::from_f32(value).to_bits();
    }
    Ok(())
}

pub(super) fn fill_bayer_crop_edges(
    values: &mut [f32],
    width: u32,
    height: u32,
    origin_x: u32,
    origin_y: u32,
    interior_width: u32,
    interior_height: u32,
) {
    let max_x = origin_x + interior_width - 1;
    let max_y = origin_y + interior_height - 1;
    for y in 0..height {
        for x in 0..width {
            if x >= origin_x && x <= max_x && y >= origin_y && y <= max_y {
                continue;
            }
            let source_x = x.clamp(origin_x, max_x);
            let source_y = y.clamp(origin_y, max_y);
            let source = (source_y * width + source_x) as usize;
            let destination = (y * width + x) as usize;
            values[destination] = values[source];
        }
    }
}

pub(super) fn reflected_raw_tile(
    raw: &LoadedRaw,
    origin_x: i32,
    origin_y: i32,
) -> Result<LoadedRaw> {
    let pixels = TILE_EDGE * TILE_EDGE;
    let mut raw_pixels = vec![0u16; pixels];
    let mut colors = vec![0u8; pixels];
    let mut blacks = vec![0.0f32; pixels];
    for y in 0..TILE_EDGE {
        let source_y = reflect_index(i64::from(origin_y) + y as i64, raw.height as usize);
        for x in 0..TILE_EDGE {
            let source_x = reflect_index(i64::from(origin_x) + x as i64, raw.width as usize);
            let source = source_y * raw.width as usize + source_x;
            let destination = y * TILE_EDGE + x;
            raw_pixels[destination] = raw.raw_pixels[source];
            colors[destination] = raw.color_indices[source];
            blacks[destination] = raw.black_levels_per_pixel[source];
        }
    }
    Ok(raw.derive_with(
        TILE_EDGE as u32,
        TILE_EDGE as u32,
        raw_pixels,
        CompactPixelMap::compact_from_dense(TILE_EDGE as u32, TILE_EDGE as u32, colors, 64),
        CompactPixelMap::compact_from_dense(TILE_EDGE as u32, TILE_EDGE as u32, blacks, 64),
    ))
}

pub(super) fn rows3(rows: [[f32; 4]; 3]) -> Matrix3 {
    rows.map(|row| [row[0], row[1], row[2]])
}
