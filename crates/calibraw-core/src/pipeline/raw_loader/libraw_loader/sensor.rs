//! Sensor data from LibRaw: active-area pixel copy, CFA layout, black and white levels.

use super::*;

pub(super) fn normalize_libraw_linear_max<T>(value: T) -> u32
where
    u32: TryFrom<T>,
{
    u32::try_from(value).unwrap_or(0)
}

type ActivePixelData = (
    u32,
    u32,
    Vec<u16>,
    CompactPixelMap<u8>,
    CompactPixelMap<f32>,
);

pub(super) struct ActivePixelCopy<'a> {
    pub(super) raw: *mut ffi::libraw_data_t,
    pub(super) raw_image: *const u16,
    pub(super) raw_dimensions: [u32; 2],
    pub(super) crop_origin: [u32; 2],
    pub(super) dimensions: [u32; 2],
    pub(super) raw_pitch: usize,
    pub(super) flip: i32,
    pub(super) cfa_kind: CfaKind,
    pub(super) cdesc: [u8; 4],
    pub(super) cfa_map: [u8; 4],
    pub(super) shared_black: u32,
    pub(super) cblack: &'a [u32],
}

pub(super) unsafe fn copy_active_pixels(request: ActivePixelCopy<'_>) -> Result<ActivePixelData> {
    let ActivePixelCopy {
        raw,
        raw_image,
        raw_dimensions: [raw_width, raw_height],
        crop_origin: [crop_x, crop_y],
        dimensions: [width, height],
        raw_pitch,
        flip,
        cfa_kind,
        cdesc,
        cfa_map,
        shared_black,
        cblack,
    } = request;
    let raw_width = raw_width as usize;
    let raw_height = raw_height as usize;
    let crop_x = crop_x as usize;
    let crop_y = crop_y as usize;
    let width = width as usize;
    let height = height as usize;
    let row_bytes = raw_width
        .checked_mul(std::mem::size_of::<u16>())
        .ok_or_else(|| anyhow!("RAW row size overflow"))?;
    let pitch = if raw_pitch == 0 { row_bytes } else { raw_pitch };
    if pitch < row_bytes {
        return Err(anyhow!(
            "LibRaw raw_pitch ({pitch}) is smaller than one decoded row ({row_bytes})"
        ));
    }
    if pitch % std::mem::align_of::<u16>() != 0 {
        return Err(anyhow!("LibRaw raw_pitch ({pitch}) is not u16-aligned"));
    }

    let crop_right = crop_x
        .checked_add(width)
        .ok_or_else(|| anyhow!("active RAW horizontal crop overflow"))?;
    let crop_bottom = crop_y
        .checked_add(height)
        .ok_or_else(|| anyhow!("active RAW vertical crop overflow"))?;
    if crop_bottom > raw_height || crop_right > raw_width {
        return Err(anyhow!("active RAW crop exceeds decoded RAW buffer"));
    }

    let (out_width, out_height) = match flip {
        5 | 6 => (height, width),
        _ => (width, height),
    };
    let output_len = out_width
        .checked_mul(out_height)
        .ok_or_else(|| anyhow!("oriented RAW dimensions overflow"))?;
    validate_raw_dimensions(out_width as u32, out_height as u32)?;
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(output_len)
        .context("reserve oriented RAW pixel buffer")?;
    pixels.resize(output_len, 0);

    if flip == 0 {
        for y in 0..height {
            let raw_y = crop_y + y;
            let row_offset = raw_y
                .checked_mul(pitch)
                .ok_or_else(|| anyhow!("RAW row pointer offset overflow"))?;
            let row_ptr = (raw_image as *const u8).add(row_offset) as *const u16;
            let source = std::slice::from_raw_parts(row_ptr.add(crop_x), width);
            let destination = &mut pixels[y * out_width..(y + 1) * out_width];
            destination.copy_from_slice(source);
        }
    } else {
        let raw_image_addr = raw_image as usize;
        pixels.par_chunks_mut(out_width).enumerate().try_for_each(
            |(y, destination)| -> Result<()> {
                for (x, output) in destination.iter_mut().enumerate() {
                    let (src_x, src_y) = oriented_source_pos(x, y, width, height, flip);
                    let raw_x = crop_x + src_x;
                    let raw_y = crop_y + src_y;
                    let row_offset = raw_y
                        .checked_mul(pitch)
                        .ok_or_else(|| anyhow!("RAW row pointer offset overflow"))?;
                    let row_ptr =
                        unsafe { (raw_image_addr as *const u8).add(row_offset) as *const u16 };
                    *output = unsafe { *row_ptr.add(raw_x) };
                }
                Ok(())
            },
        )?;
    }

    let cfa_period = match cfa_kind {
        CfaKind::Bayer => 2usize,
        CfaKind::XTrans => 6usize,
    };
    let (black_rows, black_cols) = black_pattern_dimensions(cblack).unwrap_or((1, 1));
    let source_period_x = lcm_usize(cfa_period, black_cols).max(1);
    let source_period_y = lcm_usize(cfa_period, black_rows).max(1);
    let (period_width, period_height) = if matches!(flip, 5 | 6) {
        (
            source_period_y.min(out_width),
            source_period_x.min(out_height),
        )
    } else {
        (
            source_period_x.min(out_width),
            source_period_y.min(out_height),
        )
    };
    let pattern_len = period_width
        .checked_mul(period_height)
        .ok_or_else(|| anyhow!("RAW metadata pattern dimensions overflow"))?;
    let mut colors = Vec::new();
    colors
        .try_reserve_exact(pattern_len)
        .context("reserve compact oriented CFA pattern")?;
    let mut black_map = Vec::new();
    black_map
        .try_reserve_exact(pattern_len)
        .context("reserve compact oriented black-level pattern")?;

    for y in 0..period_height {
        for x in 0..period_width {
            let (src_x, src_y) = oriented_source_pos(x, y, width, height, flip);
            let raw_x = crop_x + src_x;
            let raw_y = crop_y + src_y;
            let libraw_color = ffi::libraw_COLOR(raw, raw_y as i32, raw_x as i32);
            if !(0..=3).contains(&libraw_color) {
                return Err(anyhow!(
                    "LibRaw returned invalid CFA channel {libraw_color} at {raw_x},{raw_y}"
                ));
            }
            if cdesc[libraw_color as usize] == 0 {
                return Err(anyhow!(
                    "LibRaw used undescribed CFA channel {libraw_color} at {raw_x},{raw_y}"
                ));
            }
            colors.push(cfa_map[libraw_color as usize]);
            black_map.push(effective_black_level(
                shared_black,
                cblack,
                libraw_color as usize,
                src_x,
                src_y,
            ));
        }
    }

    let colors = CompactPixelMap::repeating(
        out_width as u32,
        out_height as u32,
        period_width as u32,
        period_height as u32,
        colors,
    );
    let black_map = CompactPixelMap::repeating(
        out_width as u32,
        out_height as u32,
        period_width as u32,
        period_height as u32,
        black_map,
    );

    Ok((
        out_width as u32,
        out_height as u32,
        pixels,
        colors,
        black_map,
    ))
}

pub(super) fn oriented_source_pos(
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    flip: i32,
) -> (usize, usize) {
    match flip {
        3 => (width - 1 - x, height - 1 - y),
        5 => (width - 1 - y, x),
        6 => (y, height - 1 - x),
        _ => (x, y),
    }
}

pub(super) fn cdesc4(iparams: &ffi::libraw_iparams_t) -> [u8; 4] {
    [
        c_char_as_u8(iparams.cdesc[0]),
        c_char_as_u8(iparams.cdesc[1]),
        c_char_as_u8(iparams.cdesc[2]),
        c_char_as_u8(iparams.cdesc[3]),
    ]
}

pub(super) fn cfa_kind_from_filters(filters: u32) -> Result<CfaKind> {
    match filters {
        9 => Ok(CfaKind::XTrans),
        value if value >= 1000 => Ok(CfaKind::Bayer),
        0 => Err(anyhow!(
            "full-colour/linear RAW input is not supported by the CFA GPU pipeline"
        )),
        1 => Err(anyhow!(
            "Leaf CatchLight 16x16 CFA is not supported by the current demosaic paths"
        )),
        value => Err(anyhow!(
            "unsupported LibRaw CFA filter code {value}; expected Bayer or Fuji X-Trans"
        )),
    }
}

pub(super) fn canonical_cfa_map(cdesc: [u8; 4]) -> Result<[u8; 4]> {
    let mut map = [3u8; 4];
    let mut red_count = 0u8;
    let mut green_count = 0u8;
    let mut blue_count = 0u8;

    for index in 0..4 {
        map[index] = match cdesc[index] as char {
            'R' | 'r' => {
                red_count = red_count.saturating_add(1);
                0
            }
            'B' | 'b' => {
                blue_count = blue_count.saturating_add(1);
                2
            }
            'G' | 'g' => {
                let canonical = if green_count == 0 { 1 } else { 3 };
                green_count = green_count.saturating_add(1);
                canonical
            }
            '\0' => 3,
            other => {
                return Err(anyhow!(
                    "unsupported non-RGB CFA descriptor {other:?} in {:?}",
                    cdesc.map(char::from)
                ));
            }
        };
    }

    if red_count != 1 || blue_count != 1 || !(1..=2).contains(&green_count) {
        return Err(anyhow!(
                "unsupported RGB CFA descriptor {:?}; expected one red, one blue, and one or two green planes",
                cdesc.map(char::from)
            ));
    }

    Ok(map)
}

pub(super) fn canonicalize_f32x4(values: [f32; 4], cfa_map: [u8; 4]) -> [f32; 4] {
    let mut out = [0.0; 4];
    for physical in 0..4 {
        out[cfa_map[physical] as usize] = values[physical];
    }
    out
}

pub(super) fn logical_rgb_channel(cdesc: [u8; 4], cfa_channel: usize) -> Option<usize> {
    match cdesc[cfa_channel.min(3)] as char {
        'R' | 'r' => Some(0),
        'G' | 'g' => Some(1),
        'B' | 'b' => Some(2),
        _ => None,
    }
}

pub(in crate::pipeline::raw_loader) fn white_balance(mut wb: [f32; 4], cdesc: [u8; 4]) -> [f32; 4] {
    let mut green_sum = 0.0;
    let mut green_count = 0.0;

    for index in 0..4 {
        let is_green = matches!(cdesc[index] as char, 'G' | 'g');
        if is_green && wb[index].is_finite() && wb[index] > 0.0 {
            green_sum += wb[index];
            green_count += 1.0;
        }
    }

    let green_reference = if green_count > 0.0 {
        green_sum / green_count
    } else if wb[1].is_finite() && wb[1] > 0.0 {
        wb[1]
    } else {
        1.0
    };

    for value in &mut wb {
        *value = if value.is_finite() && *value > 0.0 {
            *value / green_reference
        } else {
            1.0
        };
    }

    wb
}

pub(super) fn black_levels(black: u32, cblack: &[u32]) -> [f32; 4] {
    let mut out = [black as f32; 4];
    for (index, value) in out.iter_mut().enumerate() {
        *value += cblack.get(index).copied().unwrap_or(0) as f32;
    }
    out
}

fn gcd_usize(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a.max(1)
}

fn lcm_usize(a: usize, b: usize) -> usize {
    a.checked_div(gcd_usize(a.max(1), b.max(1)))
        .and_then(|value| value.checked_mul(b.max(1)))
        .unwrap_or(usize::MAX)
}

pub(super) fn effective_black_level(
    black: u32,
    cblack: &[u32],
    channel: usize,
    active_x: usize,
    active_y: usize,
) -> f32 {
    let channel_offset = cblack.get(channel.min(3)).copied().unwrap_or(0);
    let pattern_offset = black_pattern_dimensions(cblack)
        .and_then(|(rows, cols)| {
            let pattern_index = (active_y % rows)
                .checked_mul(cols)?
                .checked_add(active_x % cols)?
                .checked_add(6)?;
            cblack.get(pattern_index).copied()
        })
        .unwrap_or(0);

    black
        .saturating_add(channel_offset)
        .saturating_add(pattern_offset) as f32
}

fn black_pattern_dimensions(cblack: &[u32]) -> Option<(usize, usize)> {
    let rows = usize::try_from(*cblack.get(4)?).ok()?;
    let cols = usize::try_from(*cblack.get(5)?).ok()?;
    if rows == 0 || cols == 0 {
        return None;
    }
    let values = rows.checked_mul(cols)?;
    let end = 6usize.checked_add(values)?;
    (end <= cblack.len()).then_some((rows, cols))
}

pub(super) fn white_levels(maximum: u32, linear_max: [u32; 4], black_levels: [f32; 4]) -> [f32; 4] {
    let shared_fallback = (maximum != 0)
        .then_some(maximum)
        .or_else(|| linear_max.iter().copied().find(|value| *value != 0))
        .unwrap_or(65535);

    let mut out = [shared_fallback as f32; 4];
    for index in 0..4 {
        let candidate = linear_max[index];
        let candidate_is_sane = candidate != 0
            && candidate as f32 > black_levels[index] + 1.0
            && (maximum == 0 || candidate <= maximum);
        if candidate_is_sane {
            out[index] = candidate as f32;
        }
    }
    out
}

/// Lowers nominal white levels to the sensor saturation observed in the mosaic.
///
/// LibRaw's `maximum` is often the container bit-depth ceiling (for example 4095 for 12-bit
/// Olympus ORFs) while the sensor saturates measurably lower (3972 on the XZ-1). Saturated
/// photosites then normalise below the highlight-reconstruction threshold, so clipped greens are
/// never repaired and the WB multipliers push red and blue above them, which renders as magenta.
///
/// Saturation is recognised as a pile-up: the top `window` codes hold far more pixels per code
/// than the band directly below, which a natural highlight tail never does. Like LibRaw's
/// `adjust_maximum` (threshold 0.75), the detected level must still lie in the top quarter of the
/// nominal range. Each channel's level is only ever lowered, to at most the detected code, so
/// channels sharing the nominal white keep equal levels and white balance is unchanged.
pub(super) fn saturation_adjusted_white_levels(
    white_levels: [f32; 4],
    black_levels: [f32; 4],
    raw_pixels: &[u16],
) -> [f32; 4] {
    const MIN_RANGE_FRACTION: f32 = 0.75;
    /// Width of the band below the candidate, in multiples of the top window.
    const BELOW_WINDOWS: usize = 16;
    /// Minimum per-code density ratio between the top window and the band below it.
    const PILE_UP_RATIO: u64 = 8;

    let nominal_white = white_levels.iter().copied().fold(0.0f32, f32::max);
    let black = black_levels.iter().copied().fold(0.0f32, f32::max);
    if raw_pixels.is_empty() || !nominal_white.is_finite() || nominal_white <= black + 1.0 {
        return white_levels;
    }
    let top_code = (nominal_white as usize).min(usize::from(u16::MAX));
    let range = nominal_white - black;
    let window = ((range / 1024.0) as usize).max(2);

    // Values above the nominal white are counted in the top bin; they never become candidates.
    let histogram = raw_pixels
        .par_chunks(1 << 20)
        .map(|chunk| {
            let mut bins = vec![0u64; top_code + 1];
            for &value in chunk {
                bins[usize::from(value).min(top_code)] += 1;
            }
            bins
        })
        .reduce_with(|mut sum, part| {
            sum.iter_mut().zip(part).for_each(|(a, b)| *a += b);
            sum
        })
        .unwrap_or_default();

    // Skip isolated hot pixels: the candidate is the highest code holding a meaningful count.
    // Data that reaches the nominal white already agrees with the metadata.
    let total = raw_pixels.len() as u64;
    let min_bin = (total / 1_000_000).max(16);
    if histogram[top_code] >= min_bin {
        return white_levels;
    }
    let Some(candidate) = (0..top_code).rev().find(|&code| histogram[code] >= min_bin) else {
        return white_levels;
    };
    if (candidate as f32) < black + MIN_RANGE_FRACTION * range
        || candidate < window * (BELOW_WINDOWS + 1)
    {
        return white_levels;
    }

    let top_start = candidate + 1 - window;
    let top: u64 = histogram[top_start..=candidate].iter().sum();
    let below: u64 = histogram[top_start - window * BELOW_WINDOWS..top_start]
        .iter()
        .sum();
    let below_per_window = below.div_ceil(BELOW_WINDOWS as u64).max(1);
    let min_pile_up = (total / 100_000).max(64);
    if top < min_pile_up || top < PILE_UP_RATIO * below_per_window {
        return white_levels;
    }

    let saturation = candidate as f32;
    white_levels.map(|white| white.min(saturation))
}
