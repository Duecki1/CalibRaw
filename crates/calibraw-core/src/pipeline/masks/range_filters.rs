//! Noise-robust cleanup for luminance- and colour-range masks. Both classify
//! each pixel of the mask source on its own, so sensor noise there would turn
//! into speckles and stair-stepped edges. Chroma is smoothed before colour
//! classification, and coverage is refined afterwards with a guided filter
//! that follows the image's own edges.
//!
//! Radii are fractions of the image's shorter edge, like mask grow and
//! feather, so the preview atlas, zoomed detail rasters and every device's
//! mask source filter the same image area. Edges replicate their outermost
//! pixel; region rasters carry enough margin ([`range_filter_reach`]) that
//! tile seams never see it.

use rayon::prelude::*;

/// Radius of the chroma smoothing before colour classification, applied
/// twice for a soft, roughly Gaussian kernel: 3 pixels of a 4096 × 2732
/// mask source.
const CHROMA_SMOOTHING_RADIUS: f32 = 0.0011;
/// Radius of the lighter lightness smoothing before classification. The
/// guided refinement restores sharp edges.
const LIGHTNESS_SMOOTHING_RADIUS: f32 = 0.00055;
/// Radius of the guided coverage refinement.
const GUIDED_RADIUS: f32 = 0.0018;
/// Guided-filter regularization, as a variance of OKLab lightness. Lightness
/// varying less than about its square root within the window is treated as
/// noise and the coverage there is averaged; stronger edges are kept.
const GUIDED_EPSILON: f32 = 0.005;

/// How far range-mask filtering reaches from a pixel, as a fraction of the
/// image's shorter edge: two passes of the chroma smoothing (the widest
/// classification smoothing) and the guided filter's two box passes.
pub(super) const fn range_filter_reach() -> f32 {
    2.0 * CHROMA_SMOOTHING_RADIUS + 2.0 * GUIDED_RADIUS
}

/// Raster-pixel radius of `radius` (a fraction of the image's shorter edge)
/// for a raster with `edge` pixels per shorter image edge. Zero means no
/// filtering: below one raster pixel, raster sampling already averages the
/// source.
fn raster_radius(radius: f32, edge: f32) -> usize {
    (radius * edge).round().max(0.0) as usize
}

/// Smooths OKLab `lab` in place for classification: chroma strongly,
/// lightness lightly.
pub(super) fn smooth_for_classification(
    lab: &mut [[f32; 3]],
    width: usize,
    height: usize,
    edge: f32,
) {
    for (channel, radius, passes) in [
        (0, LIGHTNESS_SMOOTHING_RADIUS, 1),
        (1, CHROMA_SMOOTHING_RADIUS, 2),
        (2, CHROMA_SMOOTHING_RADIUS, 2),
    ] {
        let radius = raster_radius(radius, edge);
        if radius == 0 {
            continue;
        }
        let mut values: Vec<f32> = lab.iter().map(|color| color[channel]).collect();
        for _ in 0..passes {
            values = box_mean(&values, width, height, radius);
        }
        for (color, value) in lab.iter_mut().zip(values) {
            color[channel] = value;
        }
    }
}

/// Smooths lightness in place for classification, like
/// [`smooth_for_classification`] does.
pub(super) fn smooth_lightness(lightness: &mut Vec<f32>, width: usize, height: usize, edge: f32) {
    let radius = raster_radius(LIGHTNESS_SMOOTHING_RADIUS, edge);
    if radius > 0 {
        *lightness = box_mean(lightness, width, height, radius);
    }
}

/// Refines `coverage` (0–1) with a guided filter on `guide` (lightness) for a
/// raster with `edge` pixels per shorter image edge (He, Sun and Tang, 2010):
/// within
/// each window, coverage becomes the best linear function of lightness, so
/// mask edges snap to lightness edges while isolated noise averages away.
pub(super) fn refine_coverage(
    coverage: &mut [f32],
    guide: &[f32],
    width: usize,
    height: usize,
    edge: f32,
) {
    let radius = raster_radius(GUIDED_RADIUS, edge);
    if radius == 0 {
        return;
    }
    let mean_guide = box_mean(guide, width, height, radius);
    let mean_coverage = box_mean(coverage, width, height, radius);
    let guide_squared: Vec<f32> = guide.par_iter().map(|value| value * value).collect();
    let mean_guide_squared = box_mean(&guide_squared, width, height, radius);
    drop(guide_squared);
    let product: Vec<f32> = guide
        .par_iter()
        .zip(coverage.par_iter())
        .map(|(guide, coverage)| guide * coverage)
        .collect();
    let mean_product = box_mean(&product, width, height, radius);
    drop(product);
    let (slope, offset): (Vec<f32>, Vec<f32>) = (0..coverage.len())
        .into_par_iter()
        .map(|index| {
            let variance = (mean_guide_squared[index] - mean_guide[index].powi(2)).max(0.0);
            let covariance = mean_product[index] - mean_guide[index] * mean_coverage[index];
            let slope = covariance / (variance + GUIDED_EPSILON);
            (slope, mean_coverage[index] - slope * mean_guide[index])
        })
        .unzip();
    let mean_slope = box_mean(&slope, width, height, radius);
    let mean_offset = box_mean(&offset, width, height, radius);
    coverage
        .par_iter_mut()
        .zip(guide.par_iter())
        .enumerate()
        .for_each(|(index, (coverage, guide))| {
            *coverage = (mean_slope[index] * guide + mean_offset[index]).clamp(0.0, 1.0);
        });
}

/// Mean over the (2·radius+1)² window around each pixel.
pub(super) fn box_mean(values: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    let rows = box_mean_rows(values, width, radius);
    box_mean_columns(&rows, width, height, radius)
}

/// Mean over the 2·radius+1 pixels around each pixel of every row.
fn box_mean_rows(values: &[f32], width: usize, radius: usize) -> Vec<f32> {
    let mut out = vec![0.0; values.len()];
    if width == 0 {
        return out;
    }
    let last = width - 1;
    let scale = 1.0 / (2 * radius + 1) as f64;
    out.par_chunks_mut(width)
        .zip(values.par_chunks(width))
        .for_each(|(out, row)| {
            let at = |index: isize| row[index.clamp(0, last as isize) as usize] as f64;
            let radius = radius as isize;
            let mut sum: f64 = (-radius..=radius).map(at).sum();
            for (x, value) in out.iter_mut().enumerate() {
                *value = (sum * scale) as f32;
                let x = x as isize;
                sum += at(x + radius + 1) - at(x - radius);
            }
        });
    out
}

/// Mean over the 2·radius+1 pixels around each pixel of every column. Bands
/// of rows run in parallel, each sliding a row of column sums downwards.
fn box_mean_columns(values: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    const BAND_ROWS: usize = 32;
    let mut out = vec![0.0; values.len()];
    if width == 0 || height == 0 {
        return out;
    }
    let last = height as isize - 1;
    let row = |y: isize| {
        let y = y.clamp(0, last) as usize;
        &values[y * width..][..width]
    };
    let scale = 1.0 / (2 * radius + 1) as f64;
    let radius = radius as isize;
    out.par_chunks_mut(width * BAND_ROWS)
        .enumerate()
        .for_each(|(band, out)| {
            let first = (band * BAND_ROWS) as isize;
            let mut sums = vec![0.0f64; width];
            for y in first - radius..=first + radius {
                for (sum, value) in sums.iter_mut().zip(row(y)) {
                    *sum += *value as f64;
                }
            }
            for (offset, out) in out.chunks_mut(width).enumerate() {
                let y = first + offset as isize;
                for (value, sum) in out.iter_mut().zip(&sums) {
                    *value = (sum * scale) as f32;
                }
                for ((sum, entering), leaving) in sums
                    .iter_mut()
                    .zip(row(y + radius + 1))
                    .zip(row(y - radius))
                {
                    *sum += *entering as f64 - *leaving as f64;
                }
            }
        });
    out
}
