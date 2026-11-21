//! Multi-band blending (Burt & Adelson, "A Multiresolution Spline With Application to
//! Image Mosaics", 1983) of the frames over one region of the panorama.
//!
//! Frames are accumulated as OpenCV's `MultiBandBlender::feed` does
//! (`modules/stitching/src/blenders.cpp`, BSD-3-Clause): each adds its Laplacian levels
//! times the Gaussian pyramid of its blend weight to one pyramid, and the weights to
//! another; the levels are normalized by the weights and collapsed. The result does not
//! depend on the order of the frames. A frame's own pyramid is masked to the pixels it
//! covers with Enblend's alpha-aware filters ([`filter_masked`]), so its uncovered
//! surroundings do not enter its coarse levels.
//!
//! Values are blended cube-root encoded, the roughly perceptual scale focus stacking
//! also blends on: ringing across a seam stays small in shadows and around bright
//! sources. As in focus stacking, every pixel's luminance is finally limited to the range
//! the covering frames measured there.

use super::super::pyramid::{filter_masked, Plane, Resample};
use super::exposure::luminance;
use rayon::prelude::*;

/// Weight sums at or below this count as no frame.
const WEIGHT_EPSILON: f32 = 1e-12;

/// The blend of one square region, `size` pixels on a side, over `levels` detail levels.
pub(super) struct RegionBlend {
    size: usize,
    levels: usize,
    /// Weighted Laplacian levels, finest first, then the weighted base.
    sums: Vec<Plane<[f32; 3]>>,
    weights: Vec<Plane<f32>>,
    /// Per pixel, the lowest and highest encoded luminance a covering frame measured.
    range: Vec<[f32; 2]>,
}

impl RegionBlend {
    /// `size` must be divisible by `2^levels`.
    pub(super) fn new(size: usize, levels: usize) -> Self {
        debug_assert_eq!(size % (1 << levels), 0);
        let sizes = (0..=levels).map(|l| size >> l);
        Self {
            size,
            levels,
            sums: sizes.clone().map(|s| plane(s, [0.0; 3])).collect(),
            weights: sizes.map(|s| plane(s, 0.0)).collect(),
            range: vec![[f32::INFINITY, f32::NEG_INFINITY]; size * size],
        }
    }

    /// Adds a frame: its cube-root encoded `values`, its `coverage` (1 where it sees the
    /// pixel, else 0) and its blend `weight` (0 where it does not cover).
    pub(super) fn add(&mut self, values: Vec<[f32; 3]>, coverage: Vec<f32>, weight: Vec<f32>) {
        let size = self.size;
        self.range
            .par_iter_mut()
            .zip(&values)
            .zip(&coverage)
            .filter(|(_, &c)| c > 0.0)
            .for_each(|((range, value), _)| {
                let l = luminance(*value);
                range[0] = range[0].min(l);
                range[1] = range[1].max(l);
            });
        let mut image = Plane {
            width: size,
            height: size,
            data: values,
        };
        let mut mask = Plane {
            width: size,
            height: size,
            data: coverage,
        };
        let mut weight = Plane {
            width: size,
            height: size,
            data: weight,
        };
        for level in 0..self.levels {
            let (coarse, coarse_mask) = filter_masked(&image, &mask, Resample::Reduce);
            let expanded = filter_masked(
                &coarse,
                &coarse_mask,
                Resample::Expand {
                    width: image.width,
                    height: image.height,
                },
            )
            .0;
            accumulate(
                &mut self.sums[level],
                &mut self.weights[level],
                image
                    .data
                    .par_iter()
                    .zip(expanded.data)
                    .map(|(v, low)| [v[0] - low[0], v[1] - low[1], v[2] - low[2]]),
                &mask,
                &weight,
            );
            let coarse_weight = weight.reduce();
            image = coarse;
            mask = coarse_mask;
            weight = coarse_weight;
        }
        accumulate(
            &mut self.sums[self.levels],
            &mut self.weights[self.levels],
            image.data.into_par_iter(),
            &mask,
            &weight,
        );
    }

    /// Collapses the blend into linear values, row-major. Pixels no frame covers are 0.
    pub(super) fn finish(self) -> Vec<[f32; 3]> {
        let normalized: Vec<(Plane<[f32; 3]>, Plane<f32>)> = self
            .sums
            .into_par_iter()
            .zip(self.weights)
            .map(|(mut sums, weights)| {
                let mask = Plane {
                    width: weights.width,
                    height: weights.height,
                    data: weights
                        .data
                        .par_iter()
                        .map(|&w| f32::from(u8::from(w > WEIGHT_EPSILON)))
                        .collect(),
                };
                sums.data
                    .par_iter_mut()
                    .zip(&weights.data)
                    .for_each(|(value, &w)| {
                        *value = if w > WEIGHT_EPSILON {
                            value.map(|c| c / w)
                        } else {
                            [0.0; 3]
                        };
                    });
                (sums, mask)
            })
            .collect();
        let mut levels = normalized.into_iter().rev();
        let (mut image, mut mask) = levels.next().expect("a blend has a base level");
        for (detail, detail_mask) in levels {
            let expanded = filter_masked(
                &image,
                &mask,
                Resample::Expand {
                    width: detail.width,
                    height: detail.height,
                },
            )
            .0;
            image = Plane {
                width: detail.width,
                height: detail.height,
                data: expanded
                    .data
                    .into_par_iter()
                    .zip(detail.data)
                    .zip(&detail_mask.data)
                    .map(|((low, d), &m)| {
                        if m > 0.0 {
                            [low[0] + d[0], low[1] + d[1], low[2] + d[2]]
                        } else {
                            [0.0; 3]
                        }
                    })
                    .collect(),
            };
            mask = detail_mask;
        }
        image
            .data
            .into_par_iter()
            .zip(self.range)
            .map(|(value, [low, high])| {
                if low > high {
                    return [0.0; 3];
                }
                let l = luminance(value);
                let shift = l.clamp(low, high) - l;
                value.map(|v| {
                    let v = v + shift;
                    v * v * v
                })
            })
            .collect()
    }
}

fn plane<T: Copy>(size: usize, value: T) -> Plane<T> {
    Plane {
        width: size,
        height: size,
        data: vec![value; size * size],
    }
}

/// Adds `detail · weight` and `weight` where the frame's `mask` covers the sample.
fn accumulate(
    sums: &mut Plane<[f32; 3]>,
    weights: &mut Plane<f32>,
    detail: impl IndexedParallelIterator<Item = [f32; 3]>,
    mask: &Plane<f32>,
    weight: &Plane<f32>,
) {
    sums.data
        .par_iter_mut()
        .zip(&mut weights.data)
        .zip(detail)
        .zip(mask.data.par_iter().zip(&weight.data))
        .for_each(|(((sum, total), d), (&m, &w))| {
            if m > 0.0 && w > 0.0 {
                for c in 0..3 {
                    sum[c] += d[c] * w;
                }
                *total += w;
            }
        });
}

/// Signed cube root of each channel: the encoding blending works in.
pub(super) fn encode(rgb: [f32; 3]) -> [f32; 3] {
    rgb.map(f32::cbrt)
}
