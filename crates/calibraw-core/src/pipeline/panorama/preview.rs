//! Reduced, lens-undistorted copies of a frame: the luminance image features are found on
//! and the colour thumbnail exposure balancing and seam finding work on. Both are in the
//! frame's undistorted pinhole coordinates, so the solver never sees lens distortion.

use super::super::LensGeometryMap;
use super::exposure::luminance;
use rayon::prelude::*;

/// Long edge of the image features are found on. cpfind finds keypoints at half size;
/// 2048 px keeps a 24–60 MP frame's detail at scales features can use.
const REGISTRATION_EDGE: usize = 2048;
/// Long edge of the colour thumbnail.
const THUMBNAIL_EDGE: usize = 1024;
/// Samples whose demosaic footprint reaches this (white = 1) count as clipped.
pub(super) const CLIPPED: f32 = 0.95;
/// A frame's bright level: this share of its unclipped pixels is darker.
const LEVEL_PERCENTILE: f64 = 0.99;
/// Luminance more than eight stops below the frame's bright level is too noisy to match
/// or compare. Judged against the frame's own level rather than the sensor's white, so a
/// dim scene exposed well within the sensor's range is measured too.
const BLACK_SHARE: f32 = 1.0 / 256.0;

/// A frame reduced by an integer `factor` in its undistorted coordinates: pixel `(x, y)`
/// covers full-resolution undistorted pixels around `factor·x + (factor − 1)/2`.
pub(super) struct Reduced {
    pub(super) factor: usize,
    pub(super) width: usize,
    pub(super) height: usize,
    /// Camera RGB divided by the frame's exposure relative to the reference.
    pub(super) rgb: Vec<[f32; 3]>,
    /// Whether the pixel measured the scene: inside the sensor after undistortion, not
    /// clipped and above black.
    pub(super) measured: Vec<bool>,
    /// The frame's bright level: the luminance (in `rgb`'s units) that
    /// `LEVEL_PERCENTILE` of its unclipped pixels stay below.
    pub(super) level: f32,
}

impl Reduced {
    /// Full-resolution undistorted position of reduced position `x` (either axis).
    pub(super) fn full_position(&self, x: f64) -> f64 {
        self.factor as f64 * x + 0.5 * (self.factor as f64 - 1.0)
    }

    /// Reduced position of full-resolution undistorted position `x`.
    pub(super) fn reduced_position(&self, x: f64) -> f64 {
        (x - 0.5 * (self.factor as f64 - 1.0)) / self.factor as f64
    }

    /// Bilinear colour at full-resolution undistorted position `(x, y)`, or `None`
    /// outside the reduced image.
    pub(super) fn sample(&self, x: f64, y: f64) -> Option<ReducedSample> {
        let (x, y) = (self.reduced_position(x), self.reduced_position(y));
        let (w, h) = (self.width, self.height);
        if !(x >= 0.0 && y >= 0.0 && x <= (w - 1) as f64 && y <= (h - 1) as f64) {
            return None;
        }
        let (ix, iy) = (x.floor() as usize, y.floor() as usize);
        let (jx, jy) = ((ix + 1).min(w - 1), (iy + 1).min(h - 1));
        let (fx, fy) = ((x - ix as f64) as f32, (y - iy as f64) as f32);
        let taps = [
            (iy * w + ix, (1.0 - fx) * (1.0 - fy)),
            (iy * w + jx, fx * (1.0 - fy)),
            (jy * w + ix, (1.0 - fx) * fy),
            (jy * w + jx, fx * fy),
        ];
        let mut sample = ReducedSample {
            rgb: [0.0; 3],
            measured: true,
        };
        for (index, weight) in taps {
            for c in 0..3 {
                sample.rgb[c] += weight * self.rgb[index][c];
            }
            if weight > 0.0 {
                sample.measured &= self.measured[index];
            }
        }
        Some(sample)
    }
}

/// [`Reduced::sample`]'s result.
pub(super) struct ReducedSample {
    pub(super) rgb: [f32; 3],
    /// Every tap measured the scene.
    pub(super) measured: bool,
}

/// The source frame a preview is made from.
pub(super) struct SourceFrame<'a> {
    pub(super) width: usize,
    pub(super) height: usize,
    /// Camera RGB in sensor coordinates, row-major.
    pub(super) rgb: &'a [f32],
    /// Per pixel, the brightest sensor sample of its demosaic footprint (white = 1).
    pub(super) peak: &'a [f32],
    /// Maps undistorted to sensor positions; `None` when the lens is not corrected.
    pub(super) lens: Option<&'a LensGeometryMap>,
    /// Exposure relative to the reference frame.
    pub(super) exposure: f32,
}

impl SourceFrame<'_> {
    pub(super) fn registration(&self) -> Reduced {
        self.reduce(REGISTRATION_EDGE)
    }

    pub(super) fn thumbnail(&self) -> Reduced {
        self.reduce(THUMBNAIL_EDGE)
    }

    /// Box-reduces the sensor image so its long edge fits `edge` (the average colour and
    /// the maximum peak of each block), then resamples the reduction bilinearly onto
    /// the undistorted grid.
    fn reduce(&self, edge: usize) -> Reduced {
        let factor = self.width.max(self.height).div_ceil(edge).max(1);
        let (w, h) = (self.width.div_ceil(factor), self.height.div_ceil(factor));
        let mut boxed = vec![([0.0f32; 3], f32::NEG_INFINITY); w * h];
        boxed.par_chunks_mut(w).enumerate().for_each(|(by, row)| {
            let mut counts = vec![0u32; w];
            for sy in by * factor..((by + 1) * factor).min(self.height) {
                for sx in 0..self.width {
                    let i = sy * self.width + sx;
                    let (sum, peak) = &mut row[sx / factor];
                    for (total, value) in sum.iter_mut().zip(&self.rgb[i * 3..i * 3 + 3]) {
                        *total += value;
                    }
                    *peak = peak.max(self.peak[i]);
                    counts[sx / factor] += 1;
                }
            }
            for ((sum, _), count) in row.iter_mut().zip(counts) {
                *sum = sum.map(|v| v / count.max(1) as f32);
            }
        });
        let half = 0.5 * (factor as f64 - 1.0);
        let full_width = self.width as u32;
        let full_height = self.height as u32;
        let exposure = self.exposure;
        let pixels: Vec<([f32; 3], bool)> = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let (x, y) = (
                    factor as f64 * (i % w) as f64 + half,
                    factor as f64 * (i / w) as f64 + half,
                );
                let (sx, sy) = match self.lens {
                    Some(map) => {
                        let [sx, sy] = map.source_position_for_raster(
                            x as f32,
                            y as f32,
                            full_width,
                            full_height,
                        );
                        (f64::from(sx), f64::from(sy))
                    }
                    None => (x, y),
                };
                let (bx, by) = ((sx - half) / factor as f64, (sy - half) / factor as f64);
                if !(bx >= 0.0 && by >= 0.0 && bx <= (w - 1) as f64 && by <= (h - 1) as f64) {
                    return ([0.0; 3], false);
                }
                let (ix, iy) = (bx.floor() as usize, by.floor() as usize);
                let (jx, jy) = ((ix + 1).min(w - 1), (iy + 1).min(h - 1));
                let (fx, fy) = ((bx - ix as f64) as f32, (by - iy as f64) as f32);
                let mut rgb = [0.0f32; 3];
                let mut peak = f32::NEG_INFINITY;
                for (index, weight) in [
                    (iy * w + ix, (1.0 - fx) * (1.0 - fy)),
                    (iy * w + jx, fx * (1.0 - fy)),
                    (jy * w + ix, (1.0 - fx) * fy),
                    (jy * w + jx, fx * fy),
                ] {
                    let (value, tap_peak) = boxed[index];
                    for c in 0..3 {
                        rgb[c] += weight * value[c];
                    }
                    if weight > 0.0 {
                        peak = peak.max(tap_peak);
                    }
                }
                let clipped = peak.is_nan() || peak >= CLIPPED;
                let valid = !clipped && rgb.iter().all(|v| v.is_finite());
                (rgb.map(|v| v / exposure), valid)
            })
            .collect();
        let level = bright_level(pixels.iter().filter(|p| p.1).map(|p| luminance(p.0)));
        let mut rgb = Vec::with_capacity(pixels.len());
        let mut measured = Vec::with_capacity(pixels.len());
        for (value, valid) in pixels {
            measured.push(valid && luminance(value) > level * BLACK_SHARE);
            rgb.push(value);
        }
        Reduced {
            factor,
            width: w,
            height: h,
            rgb,
            measured,
            level,
        }
    }
}

/// The `LEVEL_PERCENTILE` percentile of `luminances`; a tiny positive value when there are
/// none or all are black, so dividing by it stays finite.
fn bright_level(luminances: impl Iterator<Item = f32>) -> f32 {
    let mut values: Vec<f32> = luminances.collect();
    if values.is_empty() {
        return f32::MIN_POSITIVE;
    }
    let index = ((values.len() - 1) as f64 * LEVEL_PERCENTILE) as usize;
    let (_, level, _) = values.select_nth_unstable_by(index, f32::total_cmp);
    level.max(f32::MIN_POSITIVE)
}

/// Fills the samples `valid` marks false from their surroundings by push-pull
/// interpolation (an average pyramid of the valid samples, read back coarse to fine),
/// so a filter running over the image sees no hard step where the mask begins.
pub(super) fn fill_invalid(width: usize, height: usize, values: &mut [f32], valid: &[bool]) {
    if valid.iter().all(|&v| v) {
        return;
    }
    // Weighted sums per level, finest first.
    let mut levels: Vec<(usize, usize, Vec<f32>, Vec<f32>)> = vec![(
        width,
        height,
        values
            .iter()
            .zip(valid)
            .map(|(&v, &ok)| if ok { v } else { 0.0 })
            .collect(),
        valid.iter().map(|&ok| f32::from(u8::from(ok))).collect(),
    )];
    while let Some((w, h, sums, weights)) = levels.last().filter(|l| l.0 > 1 || l.1 > 1) {
        let (nw, nh) = (w.div_ceil(2), h.div_ceil(2));
        let mut next_sums = vec![0.0; nw * nh];
        let mut next_weights = vec![0.0; nw * nh];
        for y in 0..*h {
            for x in 0..*w {
                let (i, j) = (y * w + x, (y / 2) * nw + x / 2);
                next_sums[j] += sums[i];
                next_weights[j] += weights[i];
            }
        }
        levels.push((nw, nh, next_sums, next_weights));
    }
    // Coarse to fine: a level's empty cell takes the mean of its parent.
    let mut parent: Option<(usize, Vec<f32>)> = None;
    for (w, h, sums, weights) in levels.into_iter().rev() {
        let means: Vec<f32> = (0..w * h)
            .map(|i| {
                if weights[i] > 0.0 {
                    sums[i] / weights[i]
                } else if let Some((pw, means)) = &parent {
                    means[(i / w / 2) * pw + (i % w) / 2]
                } else {
                    0.0
                }
            })
            .collect();
        parent = Some((w, means));
    }
    if let Some((_, means)) = parent {
        for ((value, &ok), mean) in values.iter_mut().zip(valid).zip(means) {
            if !ok {
                *value = mean;
            }
        }
    }
}
