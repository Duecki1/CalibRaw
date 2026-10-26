use super::raw_loader::CompactPixelMap;
use crate::color_math::srgb_encode;
use serde::{Deserialize, Serialize};
use std::ops::{Range, RangeInclusive};

const FLAT_MIN_SECOND_DIFFERENCE_MEDIAN_SQUARED: f32 = 0.163;
/// Sampling positions per image; the stride between them follows from it.
const TARGET_ANCHORS: u64 = 48_000;

/// Lower-half mean of `min(|a - 2c + b|, |d - 2c + e|)` over independent
/// unit Gaussian samples (Monte Carlo, 2·10⁷ draws). Averaging the lower half
/// keeps the estimate continuous on 8-bit codes and leaves out most edges and
/// texture.
const UNIT_NOISE_LOWER_HALF_SECOND_DIFFERENCE: f32 = 0.447;
/// Encoded noise of standard deviation `σ` appears in linear light as about
/// `4.79 σ² · signal + (σ / 12.92)²`: the least-squares fit of the squared
/// sRGB decoding slope over encoded [0.02, 0.98], with the intercept held at
/// the linear toe's.
const SRGB_SHOT_PER_ENCODED_VARIANCE: f32 = 4.79;
const SRGB_TOE_SLOPE: f32 = 1.0 / 12.92;
/// Encoded levels whose noise a raster estimate measures. Crushed blacks and
/// clipped highlights are flat and would read as clean.
const RASTER_MEASURED_LEVELS: Range<f32> = 0.02..0.985;
/// Plausible encoded noise: from 8-bit quantization (1/√12 of a code) to
/// twenty codes.
const RASTER_ENCODED_SIGMA_RANGE: RangeInclusive<f32> = (0.288_675 / 255.0)..=(20.0 / 255.0);
/// The encoded noise assumed for a raster too small to measure: one code.
const RASTER_NOMINAL_ENCODED_SIGMA: f32 = 1.0 / 255.0;
/// Samples a channel needs before its measurement replaces the nominal one,
/// and the count at which it is fully trusted.
const RASTER_MIN_SAMPLES: usize = 64;
const RASTER_FULL_CONFIDENCE_SAMPLES: f32 = 2_000.0;
/// Normalised sensor levels at or above this are clipped highlights, which a RAW
/// estimate leaves out of its fit.
const RAW_MEASURED_LEVEL_LIMIT: f32 = 0.985;
/// Summed sample weight at which a RAW channel's fit is fully trusted over the ISO-based
/// fallback.
const RAW_FULL_CONFIDENCE_WEIGHT: f32 = 2_000.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdaptiveDetailDefaults {
    pub luminance_denoise: f32,
    pub chroma_denoise: f32,
    pub denoise_detail: f32,
    pub denoise_quality: DenoiseQuality,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoiseProfile {
    pub shot: [f32; 4],
    pub read: [f32; 4],
    pub confidence: f32,
    pub green2_present: bool,
}

impl Default for NoiseProfile {
    fn default() -> Self {
        let quantization = (0.5f32 / 65_535.0).powi(2);
        Self {
            shot: [0.0; 4],
            read: [quantization; 4],
            confidence: 0.0,
            green2_present: false,
        }
    }
}

impl NoiseProfile {
    #[allow(clippy::too_many_arguments)]
    pub fn estimate(
        width: u32,
        height: u32,
        raw_pixels: &[u16],
        color_indices: &CompactPixelMap<u8>,
        black_levels_per_pixel: &CompactPixelMap<f32>,
        white_levels: [f32; 4],
        iso_speed: f32,
        cfa_period: u32,
    ) -> Self {
        const BIN_COUNT: usize = 20;
        const MIN_BIN_SAMPLES: usize = 24;

        let green2_present = cfa_plane_present(width, height, color_indices, cfa_period, 3);
        let expected_len = (width as usize).saturating_mul(height as usize);

        if width < cfa_period.saturating_mul(3) + 1
            || height < cfa_period.saturating_mul(3) + 1
            || raw_pixels.len() < expected_len
            || color_indices.len() < expected_len
            || black_levels_per_pixel.len() < expected_len
        {
            return Self::iso_fallback(iso_speed, white_levels, green2_present);
        }

        let radius = cfa_period.max(1);
        let stride = coprime_stride(anchor_spacing(width, height), radius);
        let mut bins: [[Vec<f32>; BIN_COUNT]; 4] =
            std::array::from_fn(|_| std::array::from_fn(|_| Vec::new()));

        let normalized = |x: u32, y: u32, channel: usize| -> Option<f32> {
            let index = (y * width + x) as usize;
            let raw = f32::from(*raw_pixels.get(index)?);
            let black = *black_levels_per_pixel.get(index)?;
            let white = white_levels[channel].max(black + 1.0);
            Some(((raw - black) / (white - black)).clamp(0.0, 1.25))
        };

        let y_start = radius;
        let y_end = height.saturating_sub(radius);
        let x_start = radius;
        let x_end = width.saturating_sub(radius);
        let mut y = y_start;
        while y < y_end {
            let mut x = x_start;
            while x < x_end {
                let index = (y * width + x) as usize;
                let channel = usize::from(color_indices[index]).min(3);
                let center = match normalized(x, y, channel) {
                    Some(v) if v < RAW_MEASURED_LEVEL_LIMIT => v,
                    _ => {
                        x = x.saturating_add(stride);
                        continue;
                    }
                };

                let h = normalized(x - radius, y, channel)
                    .zip(normalized(x + radius, y, channel))
                    .map(|(a, b)| (a - 2.0 * center + b).powi(2) / 6.0);
                let v = normalized(x, y - radius, channel)
                    .zip(normalized(x, y + radius, channel))
                    .map(|(a, b)| (a - 2.0 * center + b).powi(2) / 6.0);
                if let (Some(h), Some(v)) = (h, v) {
                    let variance_sample = h.min(v);
                    if variance_sample.is_finite() {
                        let bin = ((center.clamp(0.0, 0.999_999) * BIN_COUNT as f32) as usize)
                            .min(BIN_COUNT - 1);
                        bins[channel][bin].push(variance_sample);
                    }
                }
                x = x.saturating_add(stride);
            }
            y = y.saturating_add(stride);
        }

        let fallback = Self::iso_fallback(iso_speed, white_levels, green2_present);
        let mut shot = fallback.shot;
        let mut read = fallback.read;
        let mut channel_confidence = [0.0f32; 4];

        for channel in 0..4 {
            let mut points = Vec::<(f32, f32, f32)>::new();
            for (bin_index, values) in bins[channel].iter_mut().enumerate() {
                if values.len() < MIN_BIN_SAMPLES {
                    continue;
                }
                values.sort_unstable_by(|a, b| a.total_cmp(b));
                let median_sq = values[values.len() / 2];
                let variance = (median_sq / FLAT_MIN_SECOND_DIFFERENCE_MEDIAN_SQUARED).max(0.0);
                let signal = (bin_index as f32 + 0.5) / BIN_COUNT as f32;
                let weight = values.len().min(256) as f32;
                points.push((signal, variance, weight));
            }
            if points.len() < 3 {
                continue;
            }

            let (mut sw, mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for &(x, y, w) in &points {
                sw += w;
                sx += w * x;
                sy += w * y;
                sxx += w * x * x;
                sxy += w * x * y;
            }
            let denom: f32 = sw * sxx - sx * sx;
            if denom.abs() <= 1e-12 {
                continue;
            }
            let min_shot = fallback.shot[channel] * 0.15;
            let min_read = fallback.read[channel] * 0.15;
            let max_shot = (fallback.shot[channel] * 8.0).clamp(0.001, 0.08);
            let max_read = (fallback.read[channel] * 64.0).clamp(1e-5, 0.02);
            let fitted_a = ((sw * sxy - sx * sy) / denom).clamp(min_shot, max_shot);
            let fitted_b = ((sy - fitted_a * sx) / sw).clamp(min_read, max_read);

            let confidence = (points.iter().map(|point| point.2).sum::<f32>()
                / RAW_FULL_CONFIDENCE_WEIGHT)
                .clamp(0.0, 1.0);
            shot[channel] = fallback.shot[channel] * (1.0 - confidence) + fitted_a * confidence;
            read[channel] = fallback.read[channel] * (1.0 - confidence) + fitted_b * confidence;
            channel_confidence[channel] = confidence;
        }

        if !green2_present {
            shot[3] = shot[1];
            read[3] = read[1];
            channel_confidence[3] = channel_confidence[1];
        }

        Self {
            shot,
            read,
            confidence: (channel_confidence[0] + channel_confidence[1] + channel_confidence[2])
                / 3.0,
            green2_present,
        }
    }

    /// Estimates the noise of a photo decoded into a scene-linear RGB raster
    /// (`width * height * 3` values, display white at 1.0), which carries no
    /// sensor data to profile. The result describes the raster's own values,
    /// which the GPU denoisers read unscaled.
    ///
    /// Rendered photos carry noise of roughly constant amplitude in their
    /// encoded values, so each channel's standard deviation is measured in
    /// sRGB-encoded space and converted to the linear model. Second
    /// differences span two pixels, past the correlation that demosaicing,
    /// in-camera noise reduction and 4:2:0 chroma subsampling leave between
    /// neighbours, and the sampling stride is coprime with 8 so anchors visit
    /// every JPEG block phase.
    pub fn estimate_raster(width: u32, height: u32, rgb: &[f32]) -> Self {
        const RADIUS: u32 = 2;
        const JPEG_BLOCK_EDGE: u32 = 8;

        let expected_len = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(3);
        if width < 2 * RADIUS + 1 || height < 2 * RADIUS + 1 || rgb.len() < expected_len {
            return Self::from_encoded_sigma([RASTER_NOMINAL_ENCODED_SIGMA; 3], 0.0);
        }

        let stride = coprime_stride(anchor_spacing(width, height), JPEG_BLOCK_EDGE) as usize;
        let encoded = |x: u32, y: u32, channel: usize| {
            srgb_encode(rgb[(y as usize * width as usize + x as usize) * 3 + channel])
        };
        let mut differences: [Vec<f32>; 3] = std::array::from_fn(|_| Vec::new());
        for y in (RADIUS..height - RADIUS).step_by(stride) {
            for x in (RADIUS..width - RADIUS).step_by(stride) {
                for (channel, samples) in differences.iter_mut().enumerate() {
                    let center = encoded(x, y, channel);
                    if !RASTER_MEASURED_LEVELS.contains(&center) {
                        continue;
                    }
                    let horizontal = encoded(x - RADIUS, y, channel) - 2.0 * center
                        + encoded(x + RADIUS, y, channel);
                    let vertical = encoded(x, y - RADIUS, channel) - 2.0 * center
                        + encoded(x, y + RADIUS, channel);
                    samples.push(horizontal.abs().min(vertical.abs()));
                }
            }
        }

        let mut sigma = [RASTER_NOMINAL_ENCODED_SIGMA; 3];
        let mut channel_confidence = [0.0f32; 3];
        for (channel, samples) in differences.iter_mut().enumerate() {
            if samples.len() < RASTER_MIN_SAMPLES {
                continue;
            }
            sigma[channel] = (lower_half_mean(samples) / UNIT_NOISE_LOWER_HALF_SECOND_DIFFERENCE)
                .clamp(
                    *RASTER_ENCODED_SIGMA_RANGE.start(),
                    *RASTER_ENCODED_SIGMA_RANGE.end(),
                );
            channel_confidence[channel] =
                (samples.len() as f32 / RASTER_FULL_CONFIDENCE_SAMPLES).min(1.0);
        }
        Self::from_encoded_sigma(sigma, channel_confidence.iter().sum::<f32>() / 3.0)
    }

    /// The linear-light model of noise with standard deviation `sigma` per
    /// RGB channel in sRGB-encoded values.
    fn from_encoded_sigma(sigma: [f32; 3], confidence: f32) -> Self {
        let rgbg = [sigma[0], sigma[1], sigma[2], sigma[1]];
        Self {
            shot: rgbg.map(|sigma| SRGB_SHOT_PER_ENCODED_VARIANCE * sigma * sigma),
            read: rgbg.map(|sigma| (sigma * SRGB_TOE_SLOPE).powi(2)),
            confidence,
            green2_present: false,
        }
    }

    /// The profile with each CFA channel's variance multiplied by its factor in
    /// `factors`, for example by the mean of 1/n over samples that each
    /// average n independent sensor samples.
    pub fn scaled_variance(self, factors: [f32; 4]) -> Self {
        let factors = factors.map(|factor| factor.clamp(1e-4, 1.0));
        Self {
            shot: std::array::from_fn(|channel| self.shot[channel] * factors[channel]),
            read: std::array::from_fn(|channel| self.read[channel] * factors[channel]),
            confidence: self.confidence,
            green2_present: self.green2_present,
        }
    }

    pub fn adaptive_detail_defaults(
        self,
        iso_speed: f32,
        white_levels: [f32; 4],
        wb_coeffs: [f32; 4],
    ) -> AdaptiveDetailDefaults {
        const REFERENCE_SIGNAL: f32 = 0.18;
        const METADATA_VARIANCE_FLOOR: f32 = 1.0;
        // Noise relative to `REFERENCE_SIGNAL` that fades the automatic denoise in: nothing
        // at the low edge, the full amount at the high edge (smoothstep in between).
        const LUMINANCE_SIGMA_FADE: (f32, f32) = (0.018, 0.080);
        const OPPONENT_SIGMA_FADE: (f32, f32) = (0.035, 0.160);
        // Largest automatic amounts: luminance on its 0-100 slider scale, chroma on its
        // 0-1 scale, with the exponents shaping how quickly they rise.
        const MAX_LUMINANCE_DENOISE: f32 = 34.0;
        const LUMINANCE_DENOISE_EXPONENT: f32 = 1.15;
        const MAX_CHROMA_DENOISE: f32 = 0.38;
        const CHROMA_DENOISE_EXPONENT: f32 = 0.8;
        // Detail rises from the untouched default by this much at full luminance need.
        const DETAIL_BOOST: f32 = 8.0;
        // Needs above which the High denoise quality is selected.
        const HIGH_QUALITY_LUMINANCE_NEED: f32 = 0.72;
        const HIGH_QUALITY_CHROMA_NEED: f32 = 0.40;
        let fallback = Self::iso_fallback(iso_speed, white_levels, self.green2_present);
        let mut variance = [0.0f32; 3];
        for channel in 0..3 {
            let wb = wb_coeffs[channel].max(0.0);
            let measured =
                self.read[channel] * wb * wb + self.shot[channel] * wb * REFERENCE_SIGNAL;
            let metadata =
                fallback.read[channel] * wb * wb + fallback.shot[channel] * wb * REFERENCE_SIGNAL;
            variance[channel] = measured.max(metadata * METADATA_VARIANCE_FLOOR);
        }

        let signal_variance = 0.25f32.powi(2) * variance[0]
            + 0.50f32.powi(2) * variance[1]
            + 0.25f32.powi(2) * variance[2];
        let opponent_variance = 0.5 * ((variance[0] + variance[1]) + (variance[2] + variance[1]));
        let relative_signal_sigma = signal_variance.max(0.0).sqrt() / REFERENCE_SIGNAL;
        let relative_opponent_sigma = opponent_variance.max(0.0).sqrt() / REFERENCE_SIGNAL;

        let luminance_need = smoothstep(
            LUMINANCE_SIGMA_FADE.0,
            LUMINANCE_SIGMA_FADE.1,
            relative_signal_sigma,
        );
        let chroma_need = smoothstep(
            OPPONENT_SIGMA_FADE.0,
            OPPONENT_SIGMA_FADE.1,
            relative_opponent_sigma,
        );
        let luminance_denoise =
            MAX_LUMINANCE_DENOISE * luminance_need.powf(LUMINANCE_DENOISE_EXPONENT);
        let chroma_denoise = MAX_CHROMA_DENOISE * chroma_need.powf(CHROMA_DENOISE_EXPONENT);

        AdaptiveDetailDefaults {
            luminance_denoise,
            chroma_denoise,
            denoise_detail: super::basicadj::default_denoise_detail()
                + DETAIL_BOOST * luminance_need,
            denoise_quality: if luminance_need > HIGH_QUALITY_LUMINANCE_NEED
                || chroma_need > HIGH_QUALITY_CHROMA_NEED
            {
                DenoiseQuality::High
            } else {
                DenoiseQuality::Balanced
            },
        }
    }

    fn iso_fallback(iso_speed: f32, white_levels: [f32; 4], green2_present: bool) -> Self {
        let iso_gain = (iso_speed.max(100.0) / 100.0).clamp(1.0, 1024.0);
        let mut shot = [0.0; 4];
        let mut read = [0.0; 4];
        for channel in 0..4 {
            let range = white_levels[channel].max(1_024.0);
            shot[channel] = (iso_gain / range).clamp(0.0, 0.02);
            let sigma_codes = 0.75 * iso_gain.sqrt();
            read[channel] = (sigma_codes / range).powi(2).clamp(1e-12, 0.005);
        }
        if !green2_present {
            shot[3] = shot[1];
            read[3] = read[1];
        }
        Self {
            shot,
            read,
            confidence: 0.0,
            green2_present,
        }
    }
}

fn smoothstep(low: f32, high: f32, value: f32) -> f32 {
    let t = ((value - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The anchor spacing that samples about [`TARGET_ANCHORS`] positions.
fn anchor_spacing(width: u32, height: u32) -> u32 {
    let pixels = u64::from(width) * u64::from(height);
    (((pixels / TARGET_ANCHORS).max(1) as f64).sqrt().ceil() as u32).max(1)
}

/// The mean of the smaller half of `samples`, which it reorders.
fn lower_half_mean(samples: &mut [f32]) -> f32 {
    let half = samples.len() / 2;
    if half == 0 {
        return 0.0;
    }
    samples.select_nth_unstable_by(half, f32::total_cmp);
    samples[..half].iter().sum::<f32>() / half as f32
}

fn cfa_plane_present(
    width: u32,
    height: u32,
    color_indices: &CompactPixelMap<u8>,
    cfa_period: u32,
    plane: u8,
) -> bool {
    let scan_width = width.min(cfa_period.max(1));
    let scan_height = height.min(cfa_period.max(1));
    (0..scan_height).any(|y| {
        (0..scan_width).any(|x| {
            let index = (y * width + x) as usize;
            color_indices
                .get(index)
                .is_some_and(|value| *value == plane)
        })
    })
}

fn coprime_stride(mut stride: u32, period: u32) -> u32 {
    let period = period.max(1);
    while gcd_u32(stride, period) != 1 {
        stride = stride.saturating_add(1);
    }
    stride.max(1)
}

fn gcd_u32(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DenoiseQuality {
    Fast,
    #[default]
    Balanced,
    High,
}

impl DenoiseQuality {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Fast => "Fast",
            Self::Balanced => "Balanced",
            Self::High => "High",
        }
    }

    pub const fn shader_value(self) -> f32 {
        match self {
            Self::Fast => 0.0,
            Self::Balanced => 1.0,
            Self::High => 2.0,
        }
    }
}
