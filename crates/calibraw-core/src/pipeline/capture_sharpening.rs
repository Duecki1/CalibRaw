//! Capture sharpening by deconvolution ([`SharpenMethod::Deconvolution`]):
//! the settings the RAW stage's Richardson-Lucy passes run with
//! (`capture_deconvolution.wgsl` in calibraw-gpu) and the photo's blur
//! radius measured from its mosaic.
//!
//! The method is darktable's capture sharpening (darktable 5.6,
//! `src/iop/demosaicing/capture.c`), which ports RawTherapee's
//! (`rtengine/capturesharpening.cc`, Ingo Weyrich): the demosaiced
//! luminance is deconvolved with a Gaussian blur of the photo's radius where
//! its local contrast clears a noise threshold, and every channel is scaled
//! by the luminance's change.

use super::{CfaKind, ExposureParams, LoadedRaw, SharpenMethod};
use rayon::prelude::*;

/// Largest blur sigma in sensor pixels, as darktable's radius slider.
pub const MAX_CAPTURE_RADIUS: f32 = 1.5;
/// The radius darktable falls back to when it cannot measure one: "almost all
/// tested sensor/lens combinations have a radius of at least 0.5".
pub const FALLBACK_CAPTURE_RADIUS: f32 = 0.5;
/// Amount per Richardson-Lucy iteration: Amount 40 runs darktable's default
/// 8 iterations, the default Amount 30 runs 6 and Amount 150 runs 30. The
/// default is lower than darktable's because darktable 5.6 measures a smaller
/// radius than [`measure_capture_radius`] (its measured area is not aligned to
/// the CFA); with the radius it measures, Amount 30 sharpens about as much as
/// darktable 5.6's default.
const AMOUNT_PER_ITERATION: f32 = 5.0;
/// Largest iteration count, at Amount 150.
pub const MAX_CAPTURE_ITERATIONS: u32 = 30;
/// Sigmas below this blur with darktable's 5x5 kernel, others with its 9x9
/// one (`CAPTURE_SMALL`).
const SMALL_KERNEL_SIGMA: f32 = 0.66;
/// Pixels the mask reads around a pixel: the 21 pixels of the contrast
/// measure and of the clipping dilation (2), then the mask's Gaussian blur
/// of sigma 2 to 3 sigma (6).
const MASK_SUPPORT: u32 = 8;
/// Pixels past a tile core the deconvolution reads at most: the mask, then
/// each iteration's two blurs of the 9x9 kernel.
pub const CAPTURE_DECONVOLUTION_MAX_SUPPORT: u32 =
    MASK_SUPPORT + MAX_CAPTURE_ITERATIONS * 2 * kernel_radius(MAX_CAPTURE_RADIUS);

/// The Detail value at which the contrast threshold is the photo's own
/// ([`auto_contrast_threshold`]); Detail 100 sharpens every contrast.
const NEUTRAL_DETAIL: f32 = 25.0;

/// Pixels the blur kernel of `sigma` reaches from its centre.
const fn kernel_radius(sigma: f32) -> u32 {
    if sigma < SMALL_KERNEL_SIGMA {
        2
    } else {
        4
    }
}

/// The settings of one render's deconvolution, in the units the shader reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaptureDeconvolution {
    /// Richardson-Lucy iterations, fractional: the last runs in part.
    iterations: f32,
    /// Gaussian sigma of the blur, in sensor pixels.
    sigma: f32,
    /// darktable's contrast sensitivity (`cs_thrs`), 0 to 1.
    contrast_threshold: f32,
    /// Restriction to strong edges, 0 to 1 (Masking / 100).
    masking: f32,
}

impl CaptureDeconvolution {
    /// The deconvolution `exposure` asks of `raw`, or None when it sharpens
    /// with [`SharpenMethod::Legacy`] or not at all.
    pub fn of(exposure: &ExposureParams, raw: &LoadedRaw) -> Option<Self> {
        let iterations = iterations(exposure)?;
        let radius = exposure.sharpen_radius;
        let sigma = if radius.is_finite() && radius > 0.0 {
            radius.min(MAX_CAPTURE_RADIUS)
        } else {
            raw.capture_radius
        };
        let detail = finite_or(exposure.sharpen_detail, NEUTRAL_DETAIL).clamp(0.0, 100.0);
        let white = raw.white_levels[..3].iter().copied().fold(0.0, f32::max);
        let auto = auto_contrast_threshold(raw.capture_metadata.iso_speed, white);
        Some(Self {
            iterations,
            sigma,
            contrast_threshold: (auto * (100.0 - detail) / (100.0 - NEUTRAL_DETAIL))
                .clamp(0.0, 1.0),
            masking: finite_or(exposure.sharpen_masking, 0.0).clamp(0.0, 100.0) / 100.0,
        })
    }

    /// Full iterations the passes run; the last of them counts by
    /// [`Self::last_fraction`].
    pub fn passes(self) -> u32 {
        (self.iterations.ceil() as u32).clamp(1, MAX_CAPTURE_ITERATIONS)
    }

    /// The share of the last iteration's change kept, in (0, 1].
    pub fn last_fraction(self) -> f32 {
        (self.iterations - (self.passes() - 1) as f32).clamp(0.0, 1.0)
    }

    pub fn sigma(self) -> f32 {
        self.sigma
    }

    pub fn contrast_threshold(self) -> f32 {
        self.contrast_threshold
    }

    pub fn masking(self) -> f32 {
        self.masking
    }
}

/// Richardson-Lucy iterations of `exposure`, or None without deconvolution.
fn iterations(exposure: &ExposureParams) -> Option<f32> {
    if exposure.sharpen_method != SharpenMethod::Deconvolution {
        return None;
    }
    let amount = finite_or(exposure.sharpen_amount, 0.0).clamp(0.0, 150.0);
    let iterations = amount / AMOUNT_PER_ITERATION;
    (iterations > 1e-4).then_some(iterations)
}

/// Pixels past a tile core the deconvolution of `exposure` reads, 0 without
/// one. A radius taken from the photo counts with the larger kernel.
pub fn capture_deconvolution_support(exposure: &ExposureParams) -> u32 {
    let Some(iterations) = iterations(exposure) else {
        return 0;
    };
    let radius = exposure.sharpen_radius;
    let sigma = if radius.is_finite() && radius > 0.0 {
        radius.min(MAX_CAPTURE_RADIUS)
    } else {
        MAX_CAPTURE_RADIUS
    };
    let passes = (iterations.ceil() as u32).clamp(1, MAX_CAPTURE_ITERATIONS);
    MASK_SUPPORT + passes * 2 * kernel_radius(sigma)
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

/// darktable 5.6's automatic contrast sensitivity (`_get_variance_threshold`
/// and `_capture_noise`): 0.4, lowered for sensors of more than 12 bits and
/// for low ISO, raised for high ISO, then truncated to hundredths.
/// `white_level` is the raw code sensors saturate at.
pub fn auto_contrast_threshold(iso_speed: f32, white_level: f32) -> f32 {
    let mut threshold = 0.4;
    if white_level > 4096.0 {
        threshold -= 0.07;
    }
    let iso = if iso_speed.is_finite() {
        iso_speed.clamp(100.0, 1000.0)
    } else {
        100.0
    };
    threshold -= 0.012 * (600.0 - iso.round()) / 100.0;
    (100.0 * threshold).trunc() / 100.0
}

/// Normalized sensor values below this are not compared (`RAWEPS`).
const RADIUS_RAW_EPSILON: f32 = 0.005;
/// A pair is compared only when its brighter sample exceeds this.
const RADIUS_LOWER_LIMIT: f32 = 0.01;
/// Samples at or above this count as clipped.
const RADIUS_UPPER_LIMIT: f32 = 0.9;

/// The blur radius of the photo in `raw`, measured from its mosaic as
/// darktable measures it (`_calc_auto_radius`, after darktable 5.6 aligned
/// the measured area to the CFA): the steepest ratio between two
/// diagonally adjacent green photosites in the centre 60% of the photo,
/// where no sample nearby clipped, is the steepest edge the lens and sensor
/// image, and a Gaussian of sigma `sqrt(1 / ln(ratio))` blurs a step into
/// that ratio. X-Trans adds 0.2. [`FALLBACK_CAPTURE_RADIUS`] for a photo
/// without a mosaic or under 1000 pixels a side.
pub fn measure_capture_radius(raw: &LoadedRaw) -> f32 {
    let width = raw.width as usize;
    let height = raw.height as usize;
    if raw.raw_pixels.len() != width * height || width < 1000 || height < 1000 {
        return FALLBACK_CAPTURE_RADIUS;
    }
    // The centre 60%, its origin on a multiple of 6 so that the CFA phase
    // of both Bayer and X-Trans stays that of the full mosaic.
    let x0 = ((0.2 * width as f32) as usize / 6) * 6;
    let y0 = ((0.2 * height as f32) as usize / 6) * 6;
    let area_width = (0.8 * width as f32) as usize - x0;
    let area_height = (0.8 * height as f32) as usize - y0;
    let samples = NormalizedMosaic { raw, width, x0, y0 };
    let radius = match raw.cfa_kind {
        CfaKind::Bayer => bayer_radius(&samples, area_width, area_height),
        CfaKind::XTrans => 0.2 + xtrans_radius(&samples, area_width, area_height),
    };
    radius.clamp(0.0, MAX_CAPTURE_RADIUS)
}

/// The mosaic's samples normalized to its black and white levels, addressed
/// relative to the measured area's origin.
struct NormalizedMosaic<'a> {
    raw: &'a LoadedRaw,
    width: usize,
    x0: usize,
    y0: usize,
}

impl NormalizedMosaic<'_> {
    fn index(&self, row: usize, col: usize, dy: isize, dx: isize) -> usize {
        let y = (self.y0 + row).wrapping_add_signed(dy);
        let x = (self.x0 + col).wrapping_add_signed(dx);
        y * self.width + x
    }

    /// The sample `(dy, dx)` from area pixel `(row, col)`.
    fn at(&self, row: usize, col: usize, dy: isize, dx: isize) -> f32 {
        let index = self.index(row, col, dy, dx);
        let raw = self.raw;
        let channel = raw.color_indices.get(index).copied().unwrap_or(1).min(3) as usize;
        let black = raw
            .black_levels_per_pixel
            .get(index)
            .copied()
            .unwrap_or(0.0);
        let white = raw.white_levels[channel].max(black + 1.0);
        (f32::from(raw.raw_pixels[index]) - black) / (white - black)
    }

    /// Whether area pixel `(row, col)` is green.
    fn is_green(&self, row: usize, col: usize, dy: isize, dx: isize) -> bool {
        let channel = self
            .raw
            .color_indices
            .get(self.index(row, col, dy, dx))
            .copied()
            .unwrap_or(0);
        channel == 1 || channel == 3
    }
}

/// `max / min` of a pair whose brighter sample clears the lower limit.
fn pair_ratio(a: f32, b: f32) -> f32 {
    let max = a.max(b);
    if max > RADIUS_LOWER_LIMIT {
        max / a.min(b)
    } else {
        1.0
    }
}

/// Radius of the steepest ratio, as `sqrt(1 / ln(ratio))`.
fn radius_of_ratio(ratio: f32) -> f32 {
    (1.0 / ratio.ln()).sqrt()
}

/// darktable's `_calcRadiusBayer`: every green photosite against the greens
/// diagonally below it.
fn bayer_radius(m: &NormalizedMosaic<'_>, width: usize, height: usize) -> f32 {
    let ratio = (4..height.saturating_sub(4))
        .into_par_iter()
        .map(|row| {
            let mut max_ratio = 1.0f32;
            let start = if m.is_green(row, 0, 0, 0) { 6 } else { 5 };
            for col in (start..width.saturating_sub(4)).step_by(2) {
                let at = |dy, dx| m.at(row, col, dy, dx);
                let val00 = at(0, 0);
                if val00 <= RADIUS_RAW_EPSILON {
                    continue;
                }
                let val1m1 = at(1, -1);
                let val1p1 = at(1, 1);
                if val1m1 > RADIUS_RAW_EPSILON {
                    let clipped = if val00 >= val1m1 {
                        at(-1, -1).max(at(-1, 1)).max(val1p1) >= RADIUS_UPPER_LIMIT
                    } else {
                        at(0, -2).max(val00).max(at(2, -2)).max(at(2, 0)) >= RADIUS_UPPER_LIMIT
                    };
                    if !clipped {
                        max_ratio = max_ratio.max(pair_ratio(val00, val1m1));
                    }
                }
                if val1p1 > RADIUS_RAW_EPSILON {
                    let clipped = if val00 >= val1p1 {
                        at(-1, -1).max(at(-1, 1)).max(val1p1) >= RADIUS_UPPER_LIMIT
                    } else {
                        val00.max(at(0, 2)).max(at(2, 0)).max(at(2, 2)) >= RADIUS_UPPER_LIMIT
                    };
                    if !clipped {
                        max_ratio = max_ratio.max(pair_ratio(val00, val1p1));
                    }
                }
            }
            max_ratio
        })
        .reduce(|| 1.0, f32::max);
    radius_of_ratio(ratio)
}

/// darktable's `_xtrans_radius_start`: the column of the first green
/// photosite of the 6x6 pattern whose left and right neighbours differ and
/// which has no green above or to its left, and the row below it. Both
/// darktable and RawTherapee leave the row counter one past the match (their
/// outer loop increments it after the inner `break`), and the sampling
/// pattern of [`xtrans_radius`] is laid out from there.
fn xtrans_start(m: &NormalizedMosaic<'_>) -> (usize, usize) {
    let color = |row: usize, col: usize| {
        let channel = m.raw.color_indices.get(m.index(row, col, 0, 0)).copied();
        match channel.unwrap_or(0) {
            3 => 1,
            channel => channel,
        }
    };
    for row in 6..12 {
        for col in 6..12 {
            if color(row, col) == 1
                && color(row, col - 1) != color(row, col + 1)
                && color(row - 1, col) != 1
                && color(row, col - 1) != 1
            {
                return (col, row + 1);
            }
        }
    }
    // Neither loop matched: both counters end at 12.
    (12, 12)
}

/// darktable's `_calcRadiusXtrans`: the solitary greens against their
/// diagonal greens and the greens of each 2x2 green square against each
/// other, every third row and column.
fn xtrans_radius(m: &NormalizedMosaic<'_>, width: usize, height: usize) -> f32 {
    let (start_x, start_y) = xtrans_start(m);
    let upper = RADIUS_UPPER_LIMIT;
    let eps = RADIUS_RAW_EPSILON;
    let ratio = (start_y + 2..height.saturating_sub(4))
        .into_par_iter()
        .step_by(3)
        .map(|row| {
            let mut max_ratio = 1.0f32;
            for col in (start_x + 2..width.saturating_sub(4)).step_by(3) {
                let at = |dy, dx| m.at(row, col, dy, dx);
                let valp1p1 = at(1, 1);
                let square_clipped = valp1p1.max(at(1, 2)).max(at(2, 1)).max(at(2, 2)) >= upper;
                let green_solitary = at(0, 0);
                if green_solitary > eps
                    && at(-1, -1).max(at(-1, 1)) < upper
                    && green_solitary < upper
                {
                    let valp1m1 = at(1, -1);
                    if valp1m1 > eps && at(1, -2).max(valp1m1).max(at(2, -2)).max(at(1, -1)) < upper
                    {
                        max_ratio = max_ratio.max(pair_ratio(green_solitary, valp1m1));
                    }
                    if valp1p1 > eps && !square_clipped {
                        max_ratio = max_ratio.max(pair_ratio(green_solitary, valp1p1));
                    }
                }
                if !square_clipped {
                    let valp2p2 = at(2, 2);
                    if valp2p2 > eps {
                        if valp1p1 > eps {
                            max_ratio = max_ratio.max(pair_ratio(valp1p1, valp2p2));
                        }
                        let green_solitary_right = at(3, 3);
                        if green_solitary_right.max(at(4, 2)).max(at(4, 4)) < upper
                            && green_solitary_right > eps
                        {
                            max_ratio = max_ratio.max(pair_ratio(green_solitary_right, valp2p2));
                        }
                    }
                    let valp1p2 = at(1, 2);
                    let valp2p1 = at(2, 1);
                    if valp2p1 > eps {
                        if valp1p2 > eps {
                            max_ratio = max_ratio.max(pair_ratio(valp1p2, valp2p1));
                        }
                        let green_solitary_left = at(3, 0);
                        if green_solitary_left.max(at(4, -1)).max(at(4, 1)) < upper
                            && green_solitary_left > eps
                        {
                            max_ratio = max_ratio.max(pair_ratio(green_solitary_left, valp2p1));
                        }
                    }
                }
            }
            max_ratio
        })
        .reduce(|| 1.0, f32::max);
    radius_of_ratio(ratio)
}
