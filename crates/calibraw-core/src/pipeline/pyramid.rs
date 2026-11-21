//! Burt & Adelson's Gaussian and Laplacian pyramids ("The Laplacian Pyramid as a Compact
//! Image Code", 1983) on row-major planes, shared by focus stacking and panorama blending.
//!
//! Filters repeat the edge. A Laplacian level stores `g − expand(reduce(g))` and collapsing
//! adds the same `expand` back, so the pyramid reconstructs exactly with any boundary rule.
//!
//! The masked variant follows Enblend's alpha-aware reduce and expand (`pyramid.h`):
//! samples outside a mask are left out of every filter and each result is normalized by
//! the weight of the samples inside, so an image's uncovered surroundings never bleed
//! into its coarse levels.

use rayon::prelude::*;

/// Burt–Adelson generating kernel (a = 0.375).
pub(crate) const KERNEL: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

/// A value that pyramid filters combine linearly.
pub(crate) trait Texel: Copy + Send + Sync {
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

/// A row-major image.
pub(crate) struct Plane<T> {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) data: Vec<T>,
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
    pub(crate) fn reduce(&self) -> Self {
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
    pub(crate) fn expand(&self, width: usize, height: usize) -> Self {
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

    /// Binomial `[1 4 6 4 1] / 16` smoothing (σ = 1).
    pub(crate) fn blur5(&self) -> Self {
        self.separable(self.width, self.height, |i, size| {
            std::array::from_fn(|k| (clamp_index(i as isize + k as isize - 2, size), KERNEL[k]))
        })
    }
}

/// A filter applied to a plane: [`Plane::reduce`] or an expand to a size.
#[derive(Clone, Copy)]
pub(crate) enum Resample {
    Reduce,
    Expand { width: usize, height: usize },
}

impl Resample {
    fn apply<T: Texel>(self, plane: &Plane<T>) -> Plane<T> {
        match self {
            Self::Reduce => plane.reduce(),
            Self::Expand { width, height } => plane.expand(width, height),
        }
    }
}

/// `filter` of `values` counting only samples where `mask` is 1 (it holds 0 or 1). Returns
/// the normalized result and its mask: 1 where any masked-in sample contributed. Values
/// where the result's mask is 0 are 0.
pub(crate) fn filter_masked(
    values: &Plane<[f32; 3]>,
    mask: &Plane<f32>,
    filter: Resample,
) -> (Plane<[f32; 3]>, Plane<f32>) {
    let premultiplied = Plane {
        width: values.width,
        height: values.height,
        data: values
            .data
            .par_iter()
            .zip(&mask.data)
            .map(|(v, &m)| v.map(|c| c * m))
            .collect(),
    };
    let mut filtered = filter.apply(&premultiplied);
    let mut weights = filter.apply(mask);
    filtered
        .data
        .par_iter_mut()
        .zip(&mut weights.data)
        .for_each(|(value, weight)| {
            if *weight > MASK_EPSILON {
                *value = value.map(|c| c / *weight);
                *weight = 1.0;
            } else {
                *value = [0.0; 3];
                *weight = 0.0;
            }
        });
    (filtered, weights)
}

/// Filter weights at or below this count as no masked-in sample.
const MASK_EPSILON: f32 = 1e-6;

fn clamp_index(index: isize, size: usize) -> usize {
    index.clamp(0, size as isize - 1) as usize
}
