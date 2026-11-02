//! Box filtering of single-channel rasters, with rows and bands of rows running
//! in parallel. Edges replicate their outermost pixel, so region rasters need a
//! margin of the filter's reach for tile seams never to see it.

use rayon::prelude::*;

/// Box passes per axis of [`gaussian_blur`]. Three bring the kernel within a
/// few percent of a Gaussian while it stays finite.
const GAUSSIAN_PASSES: usize = 3;

/// A box of `2·radius+1` whole pixels plus, at both of its ends, one more pixel
/// counted with `outer_weight` (0 to 1). The weight lets a box widen smoothly
/// from one whole radius to the next (Gwosdek et al., "Theoretical Foundations
/// of Gaussian Convolution by Extended Box Filtering", 2011).
#[derive(Clone, Copy)]
struct BoxKernel {
    radius: usize,
    outer_weight: f64,
}

impl BoxKernel {
    const fn whole(radius: usize) -> Self {
        Self {
            radius,
            outer_weight: 0.0,
        }
    }

    /// The narrowest kernel of `variance` squared pixels. Variance 0 gives the
    /// identity; the kernel changes continuously with the variance.
    fn with_variance(variance: f64) -> Self {
        let radius = (0.5 * (12.0 * variance + 1.0).sqrt() - 0.5).floor();
        let outer_weight = (2.0 * radius + 1.0) * (radius * (radius + 1.0) - 3.0 * variance)
            / (6.0 * (variance - (radius + 1.0).powi(2)));
        Self {
            radius: radius as usize,
            outer_weight,
        }
    }

    /// Pixels the kernel reads on each side of its centre.
    fn reach(self) -> usize {
        if self.outer_weight > 0.0 {
            self.radius + 1
        } else {
            self.radius
        }
    }

    fn scale(self) -> f64 {
        1.0 / ((2 * self.radius + 1) as f64 + 2.0 * self.outer_weight)
    }

    /// Writes the kernel's mean over every row of `values` to `out`.
    fn filter_rows(self, values: &[f32], out: &mut [f32], width: usize) {
        let last = width as isize - 1;
        let scale = self.scale();
        let radius = self.radius as isize;
        out.par_chunks_mut(width)
            .zip(values.par_chunks(width))
            .for_each(|(out, row)| {
                let at = |index: isize| row[index.clamp(0, last) as usize] as f64;
                let mut sum: f64 = (-radius..=radius).map(at).sum();
                for (x, value) in out.iter_mut().enumerate() {
                    let x = x as isize;
                    let entering = at(x + radius + 1);
                    *value = ((sum + self.outer_weight * (at(x - radius - 1) + entering)) * scale)
                        as f32;
                    sum += entering - at(x - radius);
                }
            });
    }

    /// Writes the kernel's mean over every column of `values` to `out`. Bands
    /// of rows run in parallel, each sliding a row of column sums downwards.
    fn filter_columns(self, values: &[f32], out: &mut [f32], width: usize, height: usize) {
        const MIN_BAND_ROWS: usize = 32;
        // Starting a band sums a whole window, so bands are no shorter than one.
        let band_rows = MIN_BAND_ROWS.max(2 * self.radius + 1);
        let last = height as isize - 1;
        let row = |y: isize| {
            let y = y.clamp(0, last) as usize;
            &values[y * width..][..width]
        };
        let scale = self.scale();
        let radius = self.radius as isize;
        out.par_chunks_mut(width * band_rows)
            .enumerate()
            .for_each(|(band, out)| {
                let first = (band * band_rows) as isize;
                let mut sums = vec![0.0f64; width];
                for y in first - radius..=first + radius {
                    for (sum, value) in sums.iter_mut().zip(row(y)) {
                        *sum += *value as f64;
                    }
                }
                for (offset, out) in out.chunks_mut(width).enumerate() {
                    let y = first + offset as isize;
                    let (before, entering, leaving) =
                        (row(y - radius - 1), row(y + radius + 1), row(y - radius));
                    for (x, value) in out.iter_mut().enumerate() {
                        let outer = before[x] as f64 + entering[x] as f64;
                        *value = ((sums[x] + self.outer_weight * outer) * scale) as f32;
                    }
                    for ((sum, entering), leaving) in sums.iter_mut().zip(entering).zip(leaving) {
                        *sum += *entering as f64 - *leaving as f64;
                    }
                }
            });
    }
}

/// Mean over the (2·radius+1)² window around each pixel.
pub(super) fn box_mean(values: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    let mut rows = vec![0.0; values.len()];
    let mut out = vec![0.0; values.len()];
    if width == 0 || height == 0 {
        return out;
    }
    let kernel = BoxKernel::whole(radius);
    kernel.filter_rows(values, &mut rows, width);
    kernel.filter_columns(&rows, &mut out, width, height);
    out
}

fn gaussian_kernel(sigma: f32) -> BoxKernel {
    BoxKernel::with_variance((sigma as f64).powi(2) / GAUSSIAN_PASSES as f64)
}

/// Blurs `values` with a near-Gaussian of `sigma` pixels, at a cost that does
/// not depend on `sigma`. A sigma of 0 leaves them untouched, and the result
/// changes continuously with `sigma`, also below one pixel.
pub(super) fn gaussian_blur(values: &mut Vec<f32>, width: usize, height: usize, sigma: f32) {
    if sigma <= 0.0 || width == 0 || height == 0 {
        return;
    }
    let kernel = gaussian_kernel(sigma);
    let mut scratch = vec![0.0; values.len()];
    for _ in 0..GAUSSIAN_PASSES {
        kernel.filter_rows(values, &mut scratch, width);
        std::mem::swap(values, &mut scratch);
    }
    for _ in 0..GAUSSIAN_PASSES {
        kernel.filter_columns(values, &mut scratch, width, height);
        std::mem::swap(values, &mut scratch);
    }
}

/// Pixels [`gaussian_blur`] reads on each side of a pixel at `sigma`.
pub(super) fn gaussian_reach(sigma: f32) -> usize {
    if sigma <= 0.0 {
        return 0;
    }
    GAUSSIAN_PASSES * gaussian_kernel(sigma).reach()
}
