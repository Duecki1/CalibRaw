//! HDR merge of an exposure bracket in linear, unbalanced camera RGB. Never tone maps or
//! clamps radiance.
//!
//! Frame weights follow darktable's RAW HDR merge (`src/control/jobs/control_jobs.c`,
//! release 5.6.0, GPL-3.0-or-later): a frame contributes in proportion to the light it
//! collected, times a smooth envelope of the brightest sensor sample its pixel was
//! demosaiced from. A frame therefore fades out before it clips instead of switching off
//! at a threshold, and a clipped measurement only counts where no frame measured the
//! pixel.
//!
//! As in HDRMerge, the exposure ratio of every frame to the reference is measured from the
//! pixels both expose well, so imprecise shutter and aperture metadata cannot leave a
//! brightness step where one frame takes over from another.
//!
//! Motion between shots (flames, leaves, people) is rejected against the reference frame
//! block by block, after the robust temporal merge of HDR+ (Hasinoff et al., "Burst
//! photography for high dynamic range and low-light imaging on mobile cameras", 2016):
//! a frame whose local brightness disagrees with the reference, or which reports a
//! clipped reference highlight as darker than the clip, fades out there.

use super::alignment::{warp, Alignment, Registration, Warped};
use super::raw_loader::validate_raw_dimensions;
use anyhow::{ensure, Result};
use rayon::prelude::*;

pub fn exposure_scale(shutter_seconds: f32, iso: f32, aperture: f32) -> Result<f32> {
    let light = collected_light(shutter_seconds, aperture)?;
    ensure!(
        iso.is_finite() && iso > 0.0,
        "HDR merge needs valid shutter speed, ISO and aperture metadata in every source"
    );
    let scale = light * iso;
    ensure!(
        scale.is_finite() && scale > 0.0,
        "Invalid HDR exposure scale"
    );
    Ok(scale)
}

/// Light reaching the sensor, shutter time × relative aperture area, in s/f².
pub fn collected_light(shutter_seconds: f32, aperture: f32) -> Result<f32> {
    ensure!(
        [shutter_seconds, aperture]
            .iter()
            .all(|v| v.is_finite() && *v > 0.0),
        "HDR merge needs valid shutter speed, ISO and aperture metadata in every source"
    );
    let light = shutter_seconds / aperture.powi(2);
    ensure!(
        light.is_finite() && light > 0.0,
        "Invalid HDR exposure scale"
    );
    Ok(light)
}

/// darktable's safety margin below the white level for demosaic interpolation and
/// quantization: a pixel whose footprint peak reaches `1 − ENVELOPE_OFFSET` is saturated.
const ENVELOPE_OFFSET: f32 = 3000.0 / 65535.0;
/// How strongly the fallback for pixels no frame measured prefers the shortest exposure:
/// a frame one stop shorter weighs 2^8 = 256 times as much.
const FALLBACK_SELECTIVITY: i32 = 8;
/// Weight of the fallback relative to the smallest frame weight, so pixels change over to
/// it continuously as the last measuring frame fades out near its clip.
const FALLBACK_WEIGHT: f32 = 1e-4;
/// Footprint peaks (white = 1) treated as well exposed when measuring exposure ratios:
/// above sensor noise and safely below saturation, where RAW data is linear.
const GAIN_SAMPLE_RANGE: std::ops::Range<f32> = 0.01..0.9;
/// Exposure-ratio histogram: ln(ratio) in ±`GAIN_LOG_RANGE`, binned at about 0.05 %.
const GAIN_LOG_RANGE: f32 = 1.0;
const GAIN_BINS: usize = 4096;
const MIN_GAIN_SAMPLES: usize = 256;
/// Edge of the square cells motion is judged on, in pixels.
const CELL: usize = 8;
/// Samples a cell needs for a comparison to count.
const MIN_CELL_SAMPLES: usize = 16;
/// Generic sensor noise in normalized units (white = 1): variance `A·x + B`. Pessimistic
/// for low ISO, so only differences well above noise read as motion.
const NOISE_SHOT: f32 = 1e-3;
const NOISE_READ: f32 = 1e-6;
const NOISE_SIGMAS: f32 = 4.0;
/// Brightness difference between a frame and the reference that is still accepted as
/// residual vignetting or flicker; a frame fades out between one and two times this.
const RELATIVE_TOLERANCE: f32 = 0.1;
/// A frame contradicts a clipped reference when it measures less than this share of the
/// clip; the cell fades out as 10 % to 30 % of its samples contradict.
const CONTRADICTION: f32 = 0.7;
const CONTRADICTION_SHARE: (f32, f32) = (0.1, 0.3);

/// darktable's envelope, applied to `peak + ENVELOPE_OFFSET`: a quadratic rise to one at
/// half range, then a smoothstep fall to zero at saturation.
fn envelope(peak: f32) -> f32 {
    let x = (peak + ENVELOPE_OFFSET).clamp(0.0, 1.0);
    if x < 0.5 {
        let t = (x / 0.5 - 1.0).abs();
        1.0 - t * t
    } else {
        let t = (1.0 - x) / 0.5;
        t * t * (3.0 - 2.0 * t)
    }
}

/// Whether `envelope(peak)` is zero; `peak` is finite.
fn saturated(peak: f32) -> bool {
    peak + ENVELOPE_OFFSET >= 1.0
}

/// Green-weighted camera luminance, the same estimate alignment registers on.
fn luminance(rgb: [f32; 3]) -> f32 {
    (rgb[0] + 2.0 * rgb[1] + rgb[2]) * 0.25
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One developed bracket frame.
pub struct HdrFrame<'a> {
    /// Black-subtracted, white-normalized camera RGB without white balance, row-major.
    /// Clipped highlights may be reconstructed: they only count as a fallback.
    pub rgb: &'a [f32],
    /// Per pixel, the brightest normalized sensor sample (white = 1) of the footprint the
    /// demosaicer interpolated it from.
    pub peak: &'a [f32],
    /// Exposure (light × sensor gain) relative to the reference, from metadata.
    pub exposure: f32,
    /// Light collected (shutter × aperture area) relative to the reference. Shot noise,
    /// and so the weight of a measurement, depends on light, not on sensor gain.
    pub light: f32,
    /// Maps the reference grid onto this frame. Its gain is used only when too few pixels
    /// are exposed well in both frames to measure the exposure ratio directly.
    pub registration: Registration,
}

/// A streaming accumulator: only one decoded bracket needs to be resident at a time. The
/// first frame added is the reference that the others are aligned and compared to.
/// RGB channels share a weight so a clipped channel cannot contaminate the merged hue.
pub struct HdrAccumulator {
    width: usize,
    height: usize,
    reference: Option<Reference>,
    pixels: Vec<MergePixel>,
    min_light: f32,
}

/// What the reference measured, in reference units.
struct Reference {
    luminance: Vec<f32>,
    peak: Vec<f32>,
}

#[derive(Clone, Copy, Default)]
struct MergePixel {
    sum: [f32; 3],
    weight: f32,
    fallback: [f32; 3],
    fallback_weight: f32,
}

impl HdrAccumulator {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        let count = validate_raw_dimensions(width, height)?;
        Ok(Self {
            width: width as usize,
            height: height as usize,
            reference: None,
            pixels: vec![MergePixel::default(); count],
            min_light: f32::INFINITY,
        })
    }

    pub fn add(&mut self, frame: HdrFrame) -> Result<()> {
        let pixels = self.pixels.len();
        ensure!(
            frame.rgb.len() == pixels * 3 && frame.peak.len() == pixels,
            "HDR frame dimensions do not match"
        );
        ensure!(
            [frame.exposure, frame.light, frame.registration.gain]
                .iter()
                .all(|v| v.is_finite() && *v > 0.0),
            "Invalid HDR relative exposure"
        );
        ensure!(
            frame.registration.alignment.is_finite(),
            "Invalid HDR alignment"
        );
        ensure!(
            self.reference.is_some() || frame.registration.alignment == Alignment::default(),
            "The HDR reference frame cannot be warped"
        );
        ensure!(
            frame
                .rgb
                .par_iter()
                .chain(frame.peak)
                .all(|v| v.is_finite()),
            "HDR source contains NaN or infinity"
        );
        let warped = warp(
            self.width,
            self.height,
            frame.rgb,
            frame.registration.alignment,
            Some(frame.peak),
        );
        let luminance: Vec<f32> = warped
            .rgb
            .par_iter()
            .map(|&rgb| luminance(rgb) / frame.exposure)
            .collect();
        let (gain, consistency) = match &self.reference {
            Some(reference) => {
                let gain = reference
                    .relative_gain(&warped, &luminance)
                    .unwrap_or(frame.registration.gain);
                let consistency = reference.consistency(
                    self.width,
                    self.height,
                    &warped,
                    &luminance,
                    frame.exposure,
                    gain,
                );
                (gain, Some(consistency))
            }
            None => (1.0, None),
        };
        let scale = 1.0 / (frame.exposure * gain);
        let fallback_preference = frame.exposure.powi(-FALLBACK_SELECTIVITY);
        self.pixels
            .par_iter_mut()
            .enumerate()
            .filter(|(i, _)| warped.covered[*i])
            .for_each(|(i, merged)| {
                let agreement = consistency.as_ref().map_or(1.0, |c| c[i]);
                let value = warped.rgb[i].map(|v| v * scale);
                let weight = frame.light * envelope(warped.peak[i]) * agreement;
                let fallback = fallback_preference * agreement;
                for (c, value) in value.into_iter().enumerate() {
                    merged.sum[c] += weight * value;
                    merged.fallback[c] += fallback * value;
                }
                merged.weight += weight;
                merged.fallback_weight += fallback;
            });
        self.min_light = self.min_light.min(frame.light);
        if self.reference.is_none() {
            self.reference = Some(Reference {
                luminance,
                peak: warped.peak,
            });
        }
        Ok(())
    }

    /// Returns merged camera RGB in reference units. Pixels no frame measured unclipped
    /// take the shortest consistent exposure, whose highlights the caller reconstructed.
    pub fn finish(self) -> Result<Vec<f32>> {
        ensure!(
            self.pixels.par_iter().all(|p| p.fallback_weight > 0.0),
            "HDR merge has uncovered pixels"
        );
        let epsilon = FALLBACK_WEIGHT * self.min_light;
        let rgb: Vec<f32> = self
            .pixels
            .par_iter()
            .flat_map_iter(|p| {
                std::array::from_fn::<f32, 3, _>(|c| {
                    (p.sum[c] + epsilon * p.fallback[c] / p.fallback_weight) / (p.weight + epsilon)
                })
            })
            .collect();
        ensure!(
            rgb.par_iter().all(|v| v.is_finite()),
            "HDR radiance exceeded floating-point range"
        );
        Ok(rgb)
    }
}

impl Reference {
    /// The frame's brightness relative to the reference after exposure normalization: the
    /// median luminance ratio over pixels both frames expose well, or `None` when too few
    /// do. The median ignores moving subjects.
    fn relative_gain(&self, frame: &Warped, luminance: &[f32]) -> Option<f32> {
        let usable = |peak: f32| GAIN_SAMPLE_RANGE.contains(&peak);
        let bin_width = 2.0 * GAIN_LOG_RANGE / GAIN_BINS as f32;
        let histogram = (0..luminance.len())
            .into_par_iter()
            .filter(|&i| frame.covered[i] && usable(frame.peak[i]) && usable(self.peak[i]))
            .filter_map(|i| {
                let ratio = (luminance[i] / self.luminance[i]).ln();
                // Bins are centered on multiples of the bin width, so equal frames
                // measure a ratio of exactly one.
                (ratio.abs() < GAIN_LOG_RANGE).then(|| {
                    (((ratio + GAIN_LOG_RANGE) / bin_width).round() as usize).min(GAIN_BINS)
                })
            })
            .fold(
                || vec![0usize; GAIN_BINS + 1],
                |mut bins, bin| {
                    bins[bin] += 1;
                    bins
                },
            )
            .reduce(
                || vec![0usize; GAIN_BINS + 1],
                |mut a, b| {
                    a.iter_mut().zip(b).for_each(|(a, b)| *a += b);
                    a
                },
            );
        let count: usize = histogram.iter().sum();
        if count < MIN_GAIN_SAMPLES {
            return None;
        }
        // The median, interpolated within its bin.
        let half = count as f32 * 0.5;
        let mut below = 0;
        let bin = histogram.iter().position(|&n| {
            below += n;
            below as f32 >= half
        })?;
        let inside = histogram[bin] as f32;
        let fraction = (half - (below as f32 - inside)) / inside;
        Some(((bin as f32 - 0.5 + fraction) * bin_width - GAIN_LOG_RANGE).exp())
    }

    /// Per pixel, how well the frame agrees with the reference, from 1 (consistent) to 0
    /// (motion). `luminance` is the frame's, divided by its metadata `exposure` but not
    /// yet by its measured `gain`.
    fn consistency(
        &self,
        width: usize,
        height: usize,
        frame: &Warped,
        luminance: &[f32],
        exposure: f32,
        gain: f32,
    ) -> Vec<f32> {
        // Converts reference units to the frame's normalized sensor units.
        let exposure = exposure * gain;
        let (cells_x, cells_y) = (width.div_ceil(CELL), height.div_ceil(CELL));
        let cells: Vec<f32> = (0..cells_x * cells_y)
            .into_par_iter()
            .map(|cell| {
                let (cx, cy) = (cell % cells_x, cell / cells_x);
                let mut both = 0usize;
                let (mut frame_sum, mut reference_sum) = (0.0f32, 0.0f32);
                let (mut clipped, mut contradicting) = (0usize, 0usize);
                for y in cy * CELL..((cy + 1) * CELL).min(height) {
                    for x in cx * CELL..((cx + 1) * CELL).min(width) {
                        let i = y * width + x;
                        if !frame.covered[i] || saturated(frame.peak[i]) {
                            continue;
                        }
                        if saturated(self.peak[i]) {
                            clipped += 1;
                            let frame_peak = frame.peak[i] / exposure;
                            if frame_peak < CONTRADICTION * (1.0 - ENVELOPE_OFFSET) {
                                contradicting += 1;
                            }
                        } else {
                            both += 1;
                            frame_sum += luminance[i] / gain;
                            reference_sum += self.luminance[i];
                        }
                    }
                }
                let mut agreement = 1.0f32;
                if both >= MIN_CELL_SAMPLES {
                    let n = both as f32;
                    let (frame_mean, reference_mean) = (frame_sum / n, reference_sum / n);
                    let frame_variance = (NOISE_SHOT * frame_mean.max(0.0) * exposure + NOISE_READ)
                        / (exposure * exposure);
                    let reference_variance = NOISE_SHOT * reference_mean.max(0.0) + NOISE_READ;
                    let tolerance = NOISE_SIGMAS
                        * ((frame_variance + reference_variance) / n).sqrt()
                        + RELATIVE_TOLERANCE * frame_mean.max(reference_mean).max(0.0);
                    agreement = 1.0
                        - smoothstep(
                            tolerance,
                            2.0 * tolerance,
                            (frame_mean - reference_mean).abs(),
                        );
                }
                if clipped >= MIN_CELL_SAMPLES {
                    let share = contradicting as f32 / clipped as f32;
                    agreement = agreement
                        .min(1.0 - smoothstep(CONTRADICTION_SHARE.0, CONTRADICTION_SHARE.1, share));
                }
                agreement
            })
            .collect();
        // Motion rarely stops at a cell edge: spread a rejection into the neighbors, then
        // interpolate between cell centers so the frame fades instead of switching.
        let eroded: Vec<f32> = (0..cells.len())
            .into_par_iter()
            .map(|cell| {
                let (cx, cy) = (cell % cells_x, cell / cells_x);
                let mut minimum = 1.0f32;
                for y in cy.saturating_sub(1)..=(cy + 1).min(cells_y - 1) {
                    for x in cx.saturating_sub(1)..=(cx + 1).min(cells_x - 1) {
                        minimum = minimum.min(cells[y * cells_x + x]);
                    }
                }
                minimum
            })
            .collect();
        let center = |pixel: usize, cells: usize| {
            let position =
                ((pixel as f32 + 0.5) / CELL as f32 - 0.5).clamp(0.0, (cells - 1) as f32);
            let low = position.floor() as usize;
            (low, (low + 1).min(cells - 1), position - low as f32)
        };
        (0..width * height)
            .into_par_iter()
            .map(|i| {
                let (x0, x1, fx) = center(i % width, cells_x);
                let (y0, y1, fy) = center(i / width, cells_y);
                let at = |x: usize, y: usize| eroded[y * cells_x + x];
                (1.0 - fy) * ((1.0 - fx) * at(x0, y0) + fx * at(x1, y0))
                    + fy * ((1.0 - fx) * at(x0, y1) + fx * at(x1, y1))
            })
            .collect()
    }
}
