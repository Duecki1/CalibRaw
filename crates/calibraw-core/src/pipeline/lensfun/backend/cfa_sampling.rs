//! Resampling CFA data at corrected positions without mixing colour channels.

use super::*;

#[derive(Clone, Copy)]
pub(super) struct CfaCorrectionContext<'a> {
    pub(super) raw: &'a LoadedRaw,
    pub(super) vignette_enabled: bool,
    pub(super) vignette_gains: &'a [f32],
}

#[derive(Clone, Copy)]
pub(super) struct CfaSample {
    pub(super) position: [f32; 2],
    pub(super) channel: u8,
    pub(super) output: [usize; 2],
}

pub(super) fn sample_corrected_cfa_subpixel(
    context: CfaCorrectionContext<'_>,
    sample: CfaSample,
) -> (f32, f32) {
    let CfaCorrectionContext {
        raw,
        vignette_enabled,
        vignette_gains,
    } = context;
    let CfaSample {
        position: [x, y],
        channel,
        ..
    } = sample;
    if raw.cfa_kind == crate::pipeline::CfaKind::Bayer && x.is_finite() && y.is_finite() {
        if let Some(sample) = sample_bayer_phase_bilinear(context, sample) {
            return sample;
        }
    }

    if raw.cfa_kind == crate::pipeline::CfaKind::XTrans && x.is_finite() && y.is_finite() {
        if let Some(sample) =
            sample_xtrans_same_color_weighted(raw, x, y, channel, vignette_enabled, vignette_gains)
        {
            return sample;
        }
    }

    let source_index = nearest_matching_sample(raw, x, y, channel);
    corrected_sample_at(raw, source_index, vignette_enabled, vignette_gains)
}

fn sample_xtrans_same_color_weighted(
    raw: &LoadedRaw,
    x: f32,
    y: f32,
    channel: u8,
    vignette_enabled: bool,
    vignette_gains: &[f32],
) -> Option<(f32, f32)> {
    let max_x = raw.width.saturating_sub(1) as i32;
    let max_y = raw.height.saturating_sub(1) as i32;
    let center_x = x.floor() as i32;
    let center_y = y.floor() as i32;
    const NEIGHBORS: usize = 4;
    let mut nearest = [(f32::INFINITY, 0usize); NEIGHBORS];

    for sample_y in (center_y - 3)..=(center_y + 3) {
        if sample_y < 0 || sample_y > max_y {
            continue;
        }
        for sample_x in (center_x - 3)..=(center_x + 3) {
            if sample_x < 0 || sample_x > max_x {
                continue;
            }
            let index = sample_y as usize * raw.width as usize + sample_x as usize;
            if raw.color_indices[index] != channel {
                continue;
            }
            let dx = sample_x as f32 - x;
            let dy = sample_y as f32 - y;
            let distance_squared = dx * dx + dy * dy;
            if distance_squared > 12.25 {
                continue;
            }
            if distance_squared < 1e-8 {
                return Some(corrected_sample_at(
                    raw,
                    index,
                    vignette_enabled,
                    vignette_gains,
                ));
            }

            if distance_squared < nearest[NEIGHBORS - 1].0 {
                let mut slot = NEIGHBORS - 1;
                while slot > 0 && distance_squared < nearest[slot - 1].0 {
                    nearest[slot] = nearest[slot - 1];
                    slot -= 1;
                }
                nearest[slot] = (distance_squared, index);
            }
        }
    }

    let mut value_sum = 0.0f32;
    let mut black_sum = 0.0f32;
    let mut weight_sum = 0.0f32;
    for (distance_squared, index) in nearest {
        if !distance_squared.is_finite() {
            continue;
        }
        let weight = 1.0 / distance_squared.max(1e-4).powi(2);
        let sample = corrected_sample_at(raw, index, vignette_enabled, vignette_gains);
        value_sum += sample.0 * weight;
        black_sum += sample.1 * weight;
        weight_sum += weight;
    }
    (weight_sum > 1e-6).then(|| (value_sum / weight_sum, black_sum / weight_sum))
}

fn sample_bayer_phase_bilinear(
    context: CfaCorrectionContext<'_>,
    sample: CfaSample,
) -> Option<(f32, f32)> {
    let CfaCorrectionContext {
        raw,
        vignette_enabled,
        vignette_gains,
    } = context;
    let CfaSample {
        position: [x, y],
        channel,
        output: [output_x, output_y],
    } = sample;
    let (x0, x1, tx) = bayer_axis_samples(x, raw.width, (output_x as u32) & 1)?;
    let (y0, y1, ty) = bayer_axis_samples(y, raw.height, (output_y as u32) & 1)?;
    let indices = [
        (y0 * raw.width + x0) as usize,
        (y0 * raw.width + x1) as usize,
        (y1 * raw.width + x0) as usize,
        (y1 * raw.width + x1) as usize,
    ];
    if indices
        .iter()
        .any(|&index| raw.color_indices.get(index).copied() != Some(channel))
    {
        return None;
    }

    let a = corrected_sample_at(raw, indices[0], vignette_enabled, vignette_gains);
    let b = corrected_sample_at(raw, indices[1], vignette_enabled, vignette_gains);
    let c = corrected_sample_at(raw, indices[2], vignette_enabled, vignette_gains);
    let d = corrected_sample_at(raw, indices[3], vignette_enabled, vignette_gains);
    let top = (lerp(a.0, b.0, tx), lerp(a.1, b.1, tx));
    let bottom = (lerp(c.0, d.0, tx), lerp(c.1, d.1, tx));
    Some((lerp(top.0, bottom.0, ty), lerp(top.1, bottom.1, ty)))
}

fn bayer_axis_samples(coordinate: f32, extent: u32, phase: u32) -> Option<(u32, u32, f32)> {
    if extent == 0 {
        return None;
    }
    let maximum = extent - 1;
    let first = phase.min(maximum);
    if first > maximum {
        return None;
    }
    let last = first + ((maximum - first) / 2) * 2;
    if first == last {
        return Some((first, first, 0.0));
    }

    let lattice = ((coordinate - first as f32) * 0.5).clamp(0.0, ((last - first) / 2) as f32);
    let lower_step = lattice.floor() as u32;
    let upper_step = (lower_step + 1).min((last - first) / 2);
    let lower = first + lower_step * 2;
    let upper = first + upper_step * 2;
    let mix = if lower == upper {
        0.0
    } else {
        ((coordinate - lower as f32) / (upper - lower) as f32).clamp(0.0, 1.0)
    };
    Some((lower, upper, mix))
}

fn corrected_sample_at(
    raw: &LoadedRaw,
    index: usize,
    vignette_enabled: bool,
    vignette_gains: &[f32],
) -> (f32, f32) {
    let black = raw.black_levels_per_pixel[index];
    let sample = f32::from(raw.raw_pixels[index]);
    let gain = if vignette_enabled {
        vignette_gains.get(index).copied().unwrap_or(1.0).max(0.0)
    } else {
        1.0
    };
    (black + (sample - black).max(0.0) * gain, black)
}

fn lerp(left: f32, right: f32, amount: f32) -> f32 {
    left + (right - left) * amount
}

fn nearest_matching_sample(raw: &LoadedRaw, x: f32, y: f32, channel: u8) -> usize {
    let max_x = raw.width.saturating_sub(1) as i32;
    let max_y = raw.height.saturating_sub(1) as i32;
    let center_x = x.round().clamp(0.0, max_x as f32) as i32;
    let center_y = y.round().clamp(0.0, max_y as f32) as i32;
    let center = center_y as usize * raw.width as usize + center_x as usize;
    if raw.color_indices[center] == channel {
        return center;
    }

    let radius_limit: i32 = match raw.cfa_kind {
        crate::pipeline::CfaKind::Bayer => 3,
        crate::pipeline::CfaKind::XTrans => 6,
    };
    let mut best = center;
    let mut best_distance = f32::INFINITY;
    for radius in 1..=radius_limit {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx.abs() != radius && dy.abs() != radius {
                    continue;
                }
                let sample_x = (center_x + dx).clamp(0, max_x);
                let sample_y = (center_y + dy).clamp(0, max_y);
                let index = sample_y as usize * raw.width as usize + sample_x as usize;
                if raw.color_indices[index] != channel {
                    continue;
                }
                let distance = (sample_x as f32 - x).powi(2) + (sample_y as f32 - y).powi(2);
                if distance < best_distance {
                    best = index;
                    best_distance = distance;
                }
            }
        }
        if best_distance.is_finite() {
            return best;
        }
    }
    best
}
