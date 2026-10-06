//! `LineAnalysisImage`: a small display-like luminance raster of a source photo in
//! lens-corrected pixel space, for detecting straight lines.

use crate::pipeline::geometry::LensGeometryMap;
use crate::pipeline::raw_loader::{CfaKind, LoadedRaw};
use anyhow::{ensure, Context, Result};
use rayon::prelude::*;

/// Longest edge the raster aims for. Bins are whole CFA periods, so the result may be smaller;
/// at 1000 px a line spanning half the frame still resolves its angle to about 0.1°.
const TARGET_EDGE: u32 = 2048;
/// Smallest raster edge on which line detection is meaningful.
const MIN_EDGE: usize = 16;
/// Percentile mapped to white, so a few specular highlights do not darken the raster.
const WHITE_PERCENTILE: f32 = 0.995;
const MAX_PERCENTILE_SAMPLES: usize = 1 << 16;

/// Luminance on 0–255 with a 1/2.2 tone curve, so edge contrast resembles what the viewer sees.
///
/// The raster is a uniform downscale of the source (the same bin size on both axes), so angles
/// measured on it equal angles in source pixels. When the source carries a lens geometry map the
/// raster is resampled into corrected space, matching what [`super::super::GeometryInverseMap`]
/// rotates.
#[derive(Clone, Debug)]
pub struct LineAnalysisImage {
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) values: Vec<f32>,
}

impl LineAnalysisImage {
    pub fn from_raw(raw: &LoadedRaw) -> Result<Self> {
        let width = raw.width as usize;
        let height = raw.height as usize;
        let pixels = width
            .checked_mul(height)
            .context("source pixel count overflow")?;
        let (linear, out_width, out_height) = if raw.is_pre_demosaiced_raster() {
            let rgb = raw
                .scene_linear_raster()
                .context("pre-demosaiced source has no raster")?;
            let bin = bin_size(raw.width, raw.height, 1);
            let (out_width, out_height) = binned_dimensions(width, height, bin)?;
            (
                bin_raster(rgb, width, bin, out_width, out_height),
                out_width,
                out_height,
            )
        } else {
            ensure!(
                raw.raw_pixels.len() == pixels,
                "sensor data has {} samples, expected {pixels}",
                raw.raw_pixels.len()
            );
            let period = match raw.cfa_kind {
                CfaKind::Bayer => 2,
                CfaKind::XTrans => 6,
            };
            let bin = bin_size(raw.width, raw.height, period);
            let (out_width, out_height) = binned_dimensions(width, height, bin)?;
            (
                bin_mosaic(raw, bin, out_width, out_height)?,
                out_width,
                out_height,
            )
        };
        let linear = match raw.lens_geometry.as_deref() {
            Some(lens) => resample_lens_corrected(&linear, out_width, out_height, lens),
            None => linear,
        };
        Ok(Self::from_linear(out_width, out_height, linear))
    }

    /// Tone-maps scene-linear luminance (any positive scale) into the analysis range.
    pub(super) fn from_linear(width: usize, height: usize, mut values: Vec<f32>) -> Self {
        debug_assert_eq!(values.len(), width * height);
        let white = percentile(&values, WHITE_PERCENTILE).max(1e-6);
        values.par_iter_mut().for_each(|value| {
            *value = (*value / white).clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0;
        });
        Self {
            width,
            height,
            values,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }
}

/// Bin edge in source pixels: a whole number of CFA periods, so every bin averages each CFA
/// colour equally and the mosaic pattern cancels out.
fn bin_size(width: u32, height: u32, period: u32) -> usize {
    let longest = width.max(height);
    (longest.div_ceil(TARGET_EDGE * period).max(1) * period) as usize
}

fn binned_dimensions(width: usize, height: usize, bin: usize) -> Result<(usize, usize)> {
    // Partial bins at the right and bottom edges are dropped so that the scale stays uniform.
    let dimensions = (width / bin, height / bin);
    ensure!(
        dimensions.0 >= MIN_EDGE && dimensions.1 >= MIN_EDGE,
        "image is too small to detect lines ({width}x{height})"
    );
    Ok(dimensions)
}

fn bin_raster(
    rgb: &[f32],
    width: usize,
    bin: usize,
    out_width: usize,
    out_height: usize,
) -> Vec<f32> {
    let scale = 1.0 / (bin * bin) as f32;
    let mut out = vec![0.0; out_width * out_height];
    out.par_chunks_mut(out_width)
        .enumerate()
        .for_each(|(out_y, row)| {
            for (out_x, value) in row.iter_mut().enumerate() {
                let mut sum = 0.0;
                for y in out_y * bin..(out_y + 1) * bin {
                    let start = (y * width + out_x * bin) * 3;
                    for pixel in rgb[start..start + bin * 3].chunks_exact(3) {
                        sum += ((pixel[0] + 2.0 * pixel[1] + pixel[2]) * 0.25).max(0.0);
                    }
                }
                *value = sum * scale;
            }
        });
    out
}

/// Averages black- and white-normalized sensor samples. White balance is not applied: it only
/// tints the raster, and line detection needs edges, not colour.
fn bin_mosaic(
    raw: &LoadedRaw,
    bin: usize,
    out_width: usize,
    out_height: usize,
) -> Result<Vec<f32>> {
    let width = raw.width as usize;
    let (color_width, color_height, colors) = raw.color_indices.storage_parts();
    let (black_width, black_height, blacks) = raw.black_levels_per_pixel.storage_parts();
    let (color_width, color_height) = (color_width as usize, color_height as usize);
    let (black_width, black_height) = (black_width as usize, black_height as usize);
    ensure!(
        color_width > 0 && color_height > 0 && colors.len() >= color_width * color_height,
        "invalid CFA colour map"
    );
    ensure!(
        black_width > 0 && black_height > 0 && blacks.len() >= black_width * black_height,
        "invalid black-level map"
    );
    let scale = 1.0 / (bin * bin) as f32;
    let mut out = vec![0.0; out_width * out_height];
    out.par_chunks_mut(out_width)
        .enumerate()
        .for_each(|(out_y, row)| {
            for (out_x, value) in row.iter_mut().enumerate() {
                let mut sum = 0.0;
                for y in out_y * bin..(out_y + 1) * bin {
                    let color_row = (y % color_height) * color_width;
                    let black_row = (y % black_height) * black_width;
                    for x in out_x * bin..(out_x + 1) * bin {
                        let channel = usize::from(colors[color_row + x % color_width]).min(3);
                        let black = blacks[black_row + x % black_width];
                        let white = raw.white_levels[channel].max(black + 1.0);
                        let sample = f32::from(raw.raw_pixels[y * width + x]);
                        sum += ((sample - black) / (white - black)).clamp(0.0, 1.0);
                    }
                }
                *value = sum * scale;
            }
        });
    Ok(out)
}

/// Resamples a source-space raster into lens-corrected space (bilinear, edge-clamped).
fn resample_lens_corrected(
    source: &[f32],
    width: usize,
    height: usize,
    lens: &LensGeometryMap,
) -> Vec<f32> {
    let mut out = vec![0.0; width * height];
    out.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        for (x, value) in row.iter_mut().enumerate() {
            let [source_x, source_y] =
                lens.source_position_for_raster(x as f32, y as f32, width as u32, height as u32);
            *value = sample_bilinear(source, width, height, source_x, source_y);
        }
    });
    out
}

fn sample_bilinear(values: &[f32], width: usize, height: usize, x: f32, y: f32) -> f32 {
    let x = if x.is_finite() { x } else { 0.0 };
    let y = if y.is_finite() { y } else { 0.0 };
    let x = x.clamp(0.0, (width - 1) as f32);
    let y = y.clamp(0.0, (height - 1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let top = values[y0 * width + x0] * (1.0 - tx) + values[y0 * width + x1] * tx;
    let bottom = values[y1 * width + x0] * (1.0 - tx) + values[y1 * width + x1] * tx;
    top * (1.0 - ty) + bottom * ty
}

fn percentile(values: &[f32], fraction: f32) -> f32 {
    let stride = values.len().div_ceil(MAX_PERCENTILE_SAMPLES).max(1);
    let mut samples: Vec<f32> = values
        .iter()
        .step_by(stride)
        .copied()
        .filter(|value| value.is_finite())
        .collect();
    if samples.is_empty() {
        return 0.0;
    }
    let index = ((samples.len() - 1) as f32 * fraction).round() as usize;
    let (_, value, _) = samples.select_nth_unstable_by(index, f32::total_cmp);
    *value
}
