//! Focus stacking: fuses frames focused at different distances into one image that is
//! sharp wherever any frame is.
//!
//! Every frame enters a Laplacian pyramid (Burt & Adelson's multiresolution spline) with
//! per-coefficient weights from its local focus measure, as in exposure fusion (Mertens et
//! al.) restricted to a contrast term. A weight is `focus^SELECTIVITY`, normalized per
//! coefficient across frames: the sharpest frame dominates fine detail, coarse levels
//! blend seamlessly, and frames of equal sharpness average, which lowers noise in flat
//! areas. The normalization is an online softmax over log weights, so one frame is
//! resident at a time and the result does not depend on frame order.
//!
//! Frames are blended as cube-root encoded linear camera RGB, a roughly perceptual scale
//! that keeps pyramid ringing small in shadows, and decoded back to linear at the end.

use super::alignment::{sample_coordinates, Registration};
use super::raw_loader::validate_raw_dimensions;
use anyhow::{ensure, Result};
use rayon::prelude::*;

/// Exponent on the focus measure (squared Laplacian energy): a frame twice as sharp has
/// 2^(2·4) = 256 times the weight.
const SELECTIVITY: f32 = 4.0;
/// Lower bound of the focus measure, so frames that are all flat at a pixel average.
const MIN_ENERGY: f32 = 1e-12;
/// Added before cube-root encoding the focus luminance, so read noise in black
/// shadows does not read as detail (≈ −10 EV below white).
const SHADOW_FLOOR: f32 = 1.0 / 1024.0;
/// Log weight of pixels a warped frame does not cover. Finite, so the online softmax
/// never subtracts infinities; blurred into neighbors by `reduce`, it stays far below
/// `UNCOVERED_LIMIT`.
const UNCOVERED: f32 = -1e30;
const UNCOVERED_LIMIT: f32 = -1e20;
/// A level's Laplacian coefficient depends on samples up to this many pixels away on that
/// level. Coefficients this close to a frame's uncovered area see its edge extension and
/// get no weight.
const LAPLACIAN_REACH: usize = 4;
/// The pyramid stops before the shorter edge falls below this.
const TOP_EDGE: usize = 8;
/// Burt–Adelson generating kernel (a = 0.375).
const KERNEL: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

/// Streaming focus-stack accumulator on the reference frame's pixel grid.
pub struct FocusStack {
    width: usize,
    height: usize,
    levels: Vec<FusedLevel>,
    /// Whether the frame with the highest finest-level weight was clipped, per pixel.
    clipped: Vec<bool>,
    frames: usize,
}

/// Online softmax state of one pyramid level.
struct FusedLevel {
    numerator: Vec<[f32; 3]>,
    denominator: Vec<f32>,
    max_log_weight: Vec<f32>,
}

impl FocusStack {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        let count = validate_raw_dimensions(width, height)?;
        let (width, height) = (width as usize, height as usize);
        let levels = level_sizes(width, height)
            .into_iter()
            .map(|(w, h)| FusedLevel {
                numerator: vec![[0.0; 3]; w * h],
                denominator: vec![0.0; w * h],
                max_log_weight: vec![f32::NEG_INFINITY; w * h],
            })
            .collect();
        Ok(Self {
            width,
            height,
            levels,
            clipped: vec![false; count],
            frames: 0,
        })
    }

    /// `rgb` is black-subtracted, white-normalized camera RGB without white balance.
    /// `registration` maps this frame onto the reference; its gain is divided out.
    /// `clipped` optionally marks demosaic footprints containing saturated sensor samples.
    pub fn add(
        &mut self,
        rgb: &[f32],
        registration: Registration,
        clipped: Option<&[u8]>,
    ) -> Result<()> {
        let pixels = self.width * self.height;
        ensure!(
            rgb.len() == pixels * 3,
            "Focus stack frame dimensions do not match"
        );
        ensure!(
            clipped.is_none_or(|clipped| clipped.len() == pixels),
            "Focus stack clipping mask dimensions do not match"
        );
        ensure!(
            registration.alignment.is_finite()
                && registration.gain.is_finite()
                && registration.gain > 0.0,
            "Invalid focus stack alignment"
        );
        ensure!(
            rgb.par_iter().all(|v| v.is_finite()),
            "Focus stack frame contains NaN or infinity"
        );
        let warped = Warped::new(self.width, self.height, rgb, registration, clipped);
        let log_weight = warped.log_weight();
        self.accumulate(warped, log_weight);
        self.frames += 1;
        Ok(())
    }

    fn accumulate(&mut self, warped: Warped, log_weight: Plane<f32>) {
        let mut image = warped.encoded;
        let mut weight = log_weight;
        let top = self.levels.len() - 1;
        for (index, level) in self.levels.iter_mut().enumerate() {
            let (detail, next) = if index < top {
                let next = image.reduce();
                let mut detail = image;
                let expanded = next.expand(detail.width, detail.height);
                detail
                    .data
                    .par_iter_mut()
                    .zip(expanded.data)
                    .for_each(|(value, low)| *value = sub(*value, low));
                (detail, Some(next))
            } else {
                (image, None)
            };
            let clipped = (index == 0).then_some((&mut self.clipped, &warped.clipped));
            level.add(&detail.data, &weight.away_from_uncovered().data, clipped);
            let Some(next) = next else {
                break;
            };
            image = next;
            weight = weight.reduce();
        }
    }

    /// Returns linear camera RGB and a mask of pixels whose dominant frame was clipped:
    /// saturated camera RGB has no trustworthy chroma.
    pub fn finish(self) -> Result<(Vec<f32>, Vec<bool>)> {
        ensure!(self.frames > 0, "Focus stack is empty");
        let sizes = level_sizes(self.width, self.height);
        let mut levels = self.levels.into_iter().zip(sizes).rev();
        let (top, (w, h)) = levels.next().expect("a pyramid has at least one level");
        let mut image = Plane {
            width: w,
            height: h,
            data: top.normalized(),
        };
        for (level, (w, h)) in levels {
            let detail = level.normalized();
            let mut expanded = image.expand(w, h);
            expanded
                .data
                .par_iter_mut()
                .zip(detail)
                .for_each(|(value, detail)| {
                    for c in 0..3 {
                        value[c] += detail[c];
                    }
                });
            image = expanded;
        }
        let rgb: Vec<f32> = image
            .data
            .into_par_iter()
            .flat_map_iter(|pixel| pixel.map(|v| v * v * v))
            .collect();
        ensure!(
            rgb.par_iter().all(|v| v.is_finite()),
            "Focus stack exceeded floating-point range"
        );
        Ok((rgb, self.clipped))
    }
}

impl FusedLevel {
    fn add(
        &mut self,
        detail: &[[f32; 3]],
        log_weight: &[f32],
        clipped: Option<(&mut Vec<bool>, &Vec<bool>)>,
    ) {
        let update = |numerator: &mut [f32; 3],
                      denominator: &mut f32,
                      max: &mut f32,
                      detail: &[f32; 3],
                      log_weight: f32| {
            if log_weight > *max {
                let rescale = (*max - log_weight).exp();
                *numerator = numerator.map(|v| v * rescale);
                *denominator *= rescale;
                *max = log_weight;
            }
            let weight = (log_weight - *max).exp();
            for c in 0..3 {
                numerator[c] += weight * detail[c];
            }
            *denominator += weight;
        };
        let state = self
            .numerator
            .par_iter_mut()
            .zip(&mut self.denominator)
            .zip(&mut self.max_log_weight)
            .zip(detail.par_iter().zip(log_weight));
        match clipped {
            Some((dominant, frame)) => state.zip(dominant.par_iter_mut().zip(frame)).for_each(
                |((((n, d), m), (detail, &w)), (dominant, &clipped))| {
                    if w > *m {
                        *dominant = clipped;
                    }
                    update(n, d, m, detail, w);
                },
            ),
            None => state.for_each(|(((n, d), m), (detail, &w))| update(n, d, m, detail, w)),
        }
    }

    fn normalized(self) -> Vec<[f32; 3]> {
        self.numerator
            .into_par_iter()
            .zip(self.denominator)
            .map(|(n, d)| n.map(|v| v / d))
            .collect()
    }
}

/// Pyramid level sizes, finest first; each halves the previous one, rounding up.
fn level_sizes(width: usize, height: usize) -> Vec<(usize, usize)> {
    let mut sizes = vec![(width, height)];
    while let Some(&(w, h)) = sizes.last().filter(|(w, h)| (*w).min(*h) >= 2 * TOP_EDGE) {
        sizes.push((w.div_ceil(2), h.div_ceil(2)));
    }
    sizes
}

/// A frame resampled onto the reference grid.
struct Warped {
    /// Cube-root encoded, gain-corrected camera RGB.
    encoded: Plane<[f32; 3]>,
    /// Cube-root encoded luminance for the focus measure, floored in the shadows.
    luminance: Plane<f32>,
    covered: Vec<bool>,
    clipped: Vec<bool>,
}

impl Warped {
    /// Catmull–Rom resampling keeps detail that bilinear interpolation would soften.
    /// Taps outside the frame repeat its edge; such pixels are marked uncovered.
    fn new(
        width: usize,
        height: usize,
        rgb: &[f32],
        registration: Registration,
        clipped: Option<&[u8]>,
    ) -> Self {
        let alignment = registration.alignment;
        let gain = registration.gain;
        let ((encoded, luminance), (covered, clipped)): ((Vec<_>, Vec<_>), (Vec<_>, Vec<_>)) = (0
            ..width * height)
            .into_par_iter()
            .map(|i| {
                let (x, y) = alignment.map((i % width) as f32, (i / width) as f32);
                let covered = sample_coordinates(width, height, x, y).is_some();
                let (ix, fx) = split(x, width);
                let (iy, fy) = split(y, height);
                let (wx, wy) = (catmull_rom(fx), catmull_rom(fy));
                let mut value = [0.0f32; 3];
                for (j, wy) in wy.into_iter().enumerate() {
                    let row = tap(iy, j, height) * width;
                    for (k, wx) in wx.into_iter().enumerate() {
                        let source = (row + tap(ix, k, width)) * 3;
                        for c in 0..3 {
                            value[c] += wy * wx * rgb[source + c];
                        }
                    }
                }
                let value = value.map(|v| v / gain);
                let clipped = clipped.is_some_and(|clipped| {
                    (1..3).any(|j| {
                        (1..3).any(|k| clipped[tap(iy, j, height) * width + tap(ix, k, width)] != 0)
                    })
                });
                let luminance = (value[0] + 2.0 * value[1] + value[2]) * 0.25;
                (
                    (
                        value.map(f32::cbrt),
                        (luminance.max(0.0) + SHADOW_FLOOR).cbrt(),
                    ),
                    (covered, clipped),
                )
            })
            .unzip();
        Self {
            encoded: Plane {
                width,
                height,
                data: encoded,
            },
            luminance: Plane {
                width,
                height,
                data: luminance,
            },
            covered,
            clipped,
        }
    }

    /// `SELECTIVITY · ln(energy)`, where energy is the squared Laplacian of the lightly
    /// smoothed luminance, averaged over a small window (σ ≈ 1.4 px).
    fn log_weight(&self) -> Plane<f32> {
        let smooth = self.luminance.blur3();
        let (w, h) = (smooth.width, smooth.height);
        let at = |x: usize, y: usize| smooth.data[y * w + x];
        let energy: Vec<f32> = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let laplacian = at(x.saturating_sub(1), y)
                    + at((x + 1).min(w - 1), y)
                    + at(x, y.saturating_sub(1))
                    + at(x, (y + 1).min(h - 1))
                    - 4.0 * at(x, y);
                laplacian * laplacian
            })
            .collect();
        let energy = Plane {
            width: w,
            height: h,
            data: energy,
        }
        .blur5()
        .blur5();
        let data = energy
            .data
            .into_par_iter()
            .zip(&self.covered)
            .map(|(energy, &covered)| {
                if covered {
                    SELECTIVITY * (energy + MIN_ENERGY).ln()
                } else {
                    UNCOVERED
                }
            })
            .collect();
        Plane {
            width: w,
            height: h,
            data,
        }
    }
}

/// Integer part (clamped into the image) and fraction of a sample coordinate.
fn split(value: f32, size: usize) -> (isize, f32) {
    let floor = value.floor();
    // NaN comes from a point the homography cannot map.
    if floor.is_nan() || floor < 0.0 {
        (0, 0.0)
    } else if floor >= (size - 1) as f32 {
        (size as isize - 1, 0.0)
    } else {
        (floor as isize, value - floor)
    }
}

/// The `k`-th of four Catmull–Rom taps around `base`, clamped to the edge.
fn tap(base: isize, k: usize, size: usize) -> usize {
    (base + k as isize - 1).clamp(0, size as isize - 1) as usize
}

fn catmull_rom(t: f32) -> [f32; 4] {
    let (t2, t3) = (t * t, t * t * t);
    [
        0.5 * (-t3 + 2.0 * t2 - t),
        0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
        0.5 * (-3.0 * t3 + 4.0 * t2 + t),
        0.5 * (t3 - t2),
    ]
}

impl Plane<f32> {
    /// These log weights with every coefficient within `LAPLACIAN_REACH` of an uncovered
    /// one excluded. A frame without uncovered pixels, such as the reference, is unchanged.
    fn away_from_uncovered(&self) -> Self {
        let covered: Vec<bool> = self.data.par_iter().map(|&w| w > UNCOVERED_LIMIT).collect();
        if covered.iter().all(|&c| c) {
            return Self {
                width: self.width,
                height: self.height,
                data: self.data.clone(),
            };
        }
        let (width, height, radius) = (self.width, self.height, LAPLACIAN_REACH);
        let horizontal: Vec<bool> = (0..covered.len())
            .into_par_iter()
            .map(|i| {
                let (x, row) = (i % width, i - i % width);
                covered[row + x.saturating_sub(radius)..=row + (x + radius).min(width - 1)]
                    .iter()
                    .all(|&c| c)
            })
            .collect();
        let data = self
            .data
            .par_iter()
            .enumerate()
            .map(|(i, &weight)| {
                let (x, y) = (i % width, i / width);
                let usable = (y.saturating_sub(radius)..=(y + radius).min(height - 1))
                    .all(|sy| horizontal[sy * width + x]);
                if usable {
                    weight
                } else {
                    UNCOVERED
                }
            })
            .collect();
        Self {
            width,
            height,
            data,
        }
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// A value that pyramid filters combine linearly.
trait Texel: Copy + Send + Sync {
    const ZERO: Self;
    fn add_weighted(self, weight: f32, sum: Self) -> Self;
}

impl Texel for f32 {
    const ZERO: Self = 0.0;
    fn add_weighted(self, weight: f32, sum: Self) -> Self {
        sum + weight * self
    }
}

impl Texel for [f32; 3] {
    const ZERO: Self = [0.0; 3];
    fn add_weighted(self, weight: f32, sum: Self) -> Self {
        [
            sum[0] + weight * self[0],
            sum[1] + weight * self[1],
            sum[2] + weight * self[2],
        ]
    }
}

/// A row-major image. Filters repeat the edge. The Laplacian pyramid reconstructs exactly
/// with any boundary rule: a level stores `g − expand(reduce(g))` and collapsing adds the
/// same `expand` back.
struct Plane<T> {
    width: usize,
    height: usize,
    data: Vec<T>,
}

impl<T: Texel> Plane<T> {
    /// Separable filter of the rows and then the columns. `taps(i, size)` lists the
    /// source indices and weights of output index `i` along an axis of `size` samples.
    fn separable<F>(&self, width: usize, height: usize, taps: F) -> Self
    where
        F: Fn(usize, usize) -> [(usize, f32); 5] + Sync,
    {
        let mut rows = vec![T::ZERO; width * self.height];
        rows.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
            let source = &self.data[y * self.width..(y + 1) * self.width];
            for (x, out) in row.iter_mut().enumerate() {
                *out = taps(x, self.width)
                    .into_iter()
                    .fold(T::ZERO, |sum, (i, w)| source[i].add_weighted(w, sum));
            }
        });
        let mut data = vec![T::ZERO; width * height];
        data.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
            let taps = taps(y, self.height);
            for (x, out) in row.iter_mut().enumerate() {
                *out = taps.into_iter().fold(T::ZERO, |sum, (i, w)| {
                    rows[i * width + x].add_weighted(w, sum)
                });
            }
        });
        Self {
            width,
            height,
            data,
        }
    }

    /// Low-pass filter and decimate by two.
    fn reduce(&self) -> Self {
        self.separable(
            self.width.div_ceil(2),
            self.height.div_ceil(2),
            |i, size| {
                std::array::from_fn(|k| {
                    (
                        clamp_index(2 * i as isize + k as isize - 2, size),
                        KERNEL[k],
                    )
                })
            },
        )
    }

    /// Upsample to `width × height` (at most twice this size) with the interpolating
    /// counterpart of `reduce`: even samples weigh 1/8, 6/8, 1/8 and odd ones 1/2, 1/2.
    fn expand(&self, width: usize, height: usize) -> Self {
        self.separable(width, height, |i, size| {
            let n = (i / 2) as isize;
            if i % 2 == 0 {
                [
                    (clamp_index(n - 1, size), 0.125),
                    (clamp_index(n, size), 0.75),
                    (clamp_index(n + 1, size), 0.125),
                    (0, 0.0),
                    (0, 0.0),
                ]
            } else {
                [
                    (clamp_index(n, size), 0.5),
                    (clamp_index(n + 1, size), 0.5),
                    (0, 0.0),
                    (0, 0.0),
                    (0, 0.0),
                ]
            }
        })
    }

    /// Binomial `[1 2 1] / 4` smoothing.
    fn blur3(&self) -> Self {
        self.separable(self.width, self.height, |i, size| {
            let i = i as isize;
            [
                (clamp_index(i - 1, size), 0.25),
                (clamp_index(i, size), 0.5),
                (clamp_index(i + 1, size), 0.25),
                (0, 0.0),
                (0, 0.0),
            ]
        })
    }

    /// Binomial `[1 4 6 4 1] / 16` smoothing (σ = 1).
    fn blur5(&self) -> Self {
        self.separable(self.width, self.height, |i, size| {
            std::array::from_fn(|k| (clamp_index(i as isize + k as isize - 2, size), KERNEL[k]))
        })
    }
}

fn clamp_index(index: isize, size: usize) -> usize {
    index.clamp(0, size as isize - 1) as usize
}

#[cfg(test)]
mod tests;
