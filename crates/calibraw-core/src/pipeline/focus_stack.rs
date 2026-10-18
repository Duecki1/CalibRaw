//! Focus stacking: fuses frames focused at different distances into one image that is
//! sharp wherever any frame is.
//!
//! Every frame enters a Laplacian pyramid (Burt & Adelson's multiresolution spline). Each
//! detail coefficient is taken from the frame with the highest salience at that level and
//! position: the energy of the luminance detail in a small window (Burt & Kolczynski,
//! "Enhanced image capture through fusion", 1993). This is the maximum-magnitude rule of
//! wavelet stackers such as focus-stack (Petteri Aimonen, MIT license): the sharpest frame
//! supplies fine detail and every coarser level follows the frame with the most contrast
//! at its own scale, so a blurred frame never averages into a sharp one. The base level,
//! which defocus does not change, is averaged. All three channels of a coefficient come
//! from one frame, so its colour stays consistent.
//!
//! Where the selection switches between frames, the collapsed pyramid can ring beyond
//! what any frame measured. As focus-stack does for its merged image, every pixel's
//! luminance is finally limited to the range the frames measured there.
//!
//! Frames are fused as cube-root encoded linear camera RGB, a roughly perceptual scale
//! that keeps pyramid ringing small in shadows, and decoded back to linear at the end.
//! One frame is resident at a time, and the result does not depend on frame order.

use super::alignment::{warp, Registration};
use super::raw_loader::validate_raw_dimensions;
use anyhow::{ensure, Result};
use rayon::prelude::*;

/// Added to linear luminance before cube-root encoding it for the salience, so read noise
/// in black shadows does not read as detail (≈ −10 EV below white).
const SHADOW_FLOOR: f32 = 1.0 / 1024.0;
/// A level's Laplacian coefficient depends on samples up to this many pixels away on that
/// level. Coefficients this close to a frame's uncovered area see its edge extension and
/// are not taken from that frame.
const LAPLACIAN_REACH: usize = 4;
/// The pyramid stops before the shorter edge falls below this.
const TOP_EDGE: usize = 8;
/// Burt–Adelson generating kernel (a = 0.375).
const KERNEL: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

/// Streaming focus-stack accumulator on the reference frame's pixel grid.
pub struct FocusStack {
    width: usize,
    height: usize,
    /// Detail levels, finest first.
    levels: Vec<SelectedLevel>,
    base: BaseLevel,
    /// Per pixel, the lowest and highest encoded luminance a covering frame measured.
    range: Vec<[f32; 2]>,
    frames: usize,
}

/// The most salient frame's detail per coefficient of one pyramid level.
struct SelectedLevel {
    salience: Vec<f32>,
    detail: Vec<[f32; 3]>,
}

/// The mean of the frames' coarsest Gaussian level.
struct BaseLevel {
    sum: Vec<[f32; 3]>,
    count: Vec<u32>,
}

impl FocusStack {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        let count = validate_raw_dimensions(width, height)?;
        let (width, height) = (width as usize, height as usize);
        let mut sizes = level_sizes(width, height);
        let (base_width, base_height) = sizes.pop().expect("a pyramid has at least one level");
        let levels = sizes
            .into_iter()
            .map(|(w, h)| SelectedLevel {
                salience: vec![f32::NEG_INFINITY; w * h],
                detail: vec![[0.0; 3]; w * h],
            })
            .collect();
        Ok(Self {
            width,
            height,
            levels,
            base: BaseLevel {
                sum: vec![[0.0; 3]; base_width * base_height],
                count: vec![0; base_width * base_height],
            },
            range: vec![[f32::INFINITY, f32::NEG_INFINITY]; count],
            frames: 0,
        })
    }

    /// `rgb` is black-subtracted, white-normalized camera RGB without white balance.
    /// `registration` maps the reference grid onto this frame; its gain is divided out.
    pub fn add(&mut self, rgb: &[f32], registration: Registration) -> Result<()> {
        let pixels = self.width * self.height;
        ensure!(
            rgb.len() == pixels * 3,
            "Focus stack frame dimensions do not match"
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
        let warped = warp(self.width, self.height, rgb, registration.alignment, None);
        let gain = registration.gain;
        let (encoded, focus): (Vec<_>, Vec<_>) = warped
            .rgb
            .par_iter()
            .map(|rgb| {
                let value = rgb.map(|v| v / gain);
                let luminance = (value[0] + 2.0 * value[1] + value[2]) * 0.25;
                (
                    value.map(f32::cbrt),
                    (luminance.max(0.0) + SHADOW_FLOOR).cbrt(),
                )
            })
            .unzip();
        self.range
            .par_iter_mut()
            .zip(&encoded)
            .zip(&warped.covered)
            .filter(|(_, &covered)| covered)
            .for_each(|((range, encoded), _)| {
                let luminance = encoded_luminance(*encoded);
                range[0] = range[0].min(luminance);
                range[1] = range[1].max(luminance);
            });
        let coverage = warped
            .covered
            .into_par_iter()
            .map(|covered| if covered { 1.0 } else { 0.0 })
            .collect();
        self.accumulate(self.plane(encoded), self.plane(focus), self.plane(coverage));
        self.frames += 1;
        Ok(())
    }

    fn plane<T>(&self, data: Vec<T>) -> Plane<T> {
        Plane {
            width: self.width,
            height: self.height,
            data,
        }
    }

    fn accumulate(
        &mut self,
        mut image: Plane<[f32; 3]>,
        mut focus: Plane<f32>,
        mut coverage: Plane<f32>,
    ) {
        for level in &mut self.levels {
            let next = image.reduce();
            let next_focus = focus.reduce();
            let mut detail = image;
            let expanded = next.expand(detail.width, detail.height);
            detail
                .data
                .par_iter_mut()
                .zip(expanded.data)
                .for_each(|(value, low)| *value = sub(*value, low));
            let expanded = next_focus.expand(focus.width, focus.height);
            focus
                .data
                .par_iter_mut()
                .zip(expanded.data)
                .for_each(|(value, low)| *value = (*value - low) * (*value - low));
            let salience = focus.blur5();
            let usable = coverage.fully_covered(LAPLACIAN_REACH);
            level
                .salience
                .par_iter_mut()
                .zip(&mut level.detail)
                .zip(salience.data.par_iter().zip(detail.data))
                .enumerate()
                .filter(|(i, _)| usable.as_ref().is_none_or(|usable| usable[*i]))
                .for_each(|(_, ((best, selected), (&salience, detail)))| {
                    if salience > *best {
                        *best = salience;
                        *selected = detail;
                    }
                });
            image = next;
            focus = next_focus;
            coverage = coverage.reduce();
        }
        let usable = coverage.fully_covered(LAPLACIAN_REACH);
        self.base
            .sum
            .par_iter_mut()
            .zip(&mut self.base.count)
            .zip(image.data)
            .enumerate()
            .filter(|(i, _)| usable.as_ref().is_none_or(|usable| usable[*i]))
            .for_each(|(_, ((sum, count), value))| {
                for c in 0..3 {
                    sum[c] += value[c];
                }
                *count += 1;
            });
    }

    /// Returns linear camera RGB.
    pub fn finish(self) -> Result<Vec<f32>> {
        ensure!(self.frames > 0, "Focus stack is empty");
        ensure!(
            self.base.count.par_iter().all(|&count| count > 0)
                && self
                    .levels
                    .iter()
                    .all(|level| level.salience.par_iter().all(|s| s.is_finite())),
            "Focus stack has uncovered pixels"
        );
        let mut sizes = level_sizes(self.width, self.height);
        let (w, h) = sizes.pop().expect("a pyramid has at least one level");
        let mut image = Plane {
            width: w,
            height: h,
            data: self
                .base
                .sum
                .into_par_iter()
                .zip(self.base.count)
                .map(|(sum, count)| sum.map(|v| v / count as f32))
                .collect(),
        };
        for (level, (w, h)) in self.levels.into_iter().zip(sizes).rev() {
            let mut expanded = image.expand(w, h);
            expanded
                .data
                .par_iter_mut()
                .zip(level.detail)
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
            .zip(self.range)
            .flat_map_iter(|(pixel, [low, high])| {
                let luminance = encoded_luminance(pixel);
                let shift = luminance.clamp(low, high) - luminance;
                pixel.map(|v| {
                    let v = v + shift;
                    v * v * v
                })
            })
            .collect();
        ensure!(
            rgb.par_iter().all(|v| v.is_finite()),
            "Focus stack exceeded floating-point range"
        );
        Ok(rgb)
    }
}

/// Luminance of cube-root encoded camera RGB, with the weights of the linear estimate.
fn encoded_luminance(rgb: [f32; 3]) -> f32 {
    (rgb[0] + 2.0 * rgb[1] + rgb[2]) * 0.25
}

/// Pyramid level sizes, finest first; each halves the previous one, rounding up.
fn level_sizes(width: usize, height: usize) -> Vec<(usize, usize)> {
    let mut sizes = vec![(width, height)];
    while let Some(&(w, h)) = sizes.last().filter(|(w, h)| (*w).min(*h) >= 2 * TOP_EDGE) {
        sizes.push((w.div_ceil(2), h.div_ceil(2)));
    }
    sizes
}

impl Plane<f32> {
    /// Whether every sample within `reach` of each coefficient is fully covered by the
    /// frame, or `None` when the whole level is.
    fn fully_covered(&self, reach: usize) -> Option<Vec<bool>> {
        // Reduced coverage is a mean of ones where the frame covers every tap.
        let covered: Vec<bool> = self.data.par_iter().map(|&c| c > 1.0 - 1e-4).collect();
        if covered.par_iter().all(|&c| c) {
            return None;
        }
        let (width, height) = (self.width, self.height);
        let horizontal: Vec<bool> = (0..covered.len())
            .into_par_iter()
            .map(|i| {
                let (x, row) = (i % width, i - i % width);
                covered[row + x.saturating_sub(reach)..=row + (x + reach).min(width - 1)]
                    .iter()
                    .all(|&c| c)
            })
            .collect();
        Some(
            (0..covered.len())
                .into_par_iter()
                .map(|i| {
                    let (x, y) = (i % width, i / width);
                    (y.saturating_sub(reach)..=(y + reach).min(height - 1))
                        .all(|sy| horizontal[sy * width + x])
                })
                .collect(),
        )
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

    /// Binomial `[1 4 6 4 1] / 16` smoothing (σ = 1): the salience window.
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
