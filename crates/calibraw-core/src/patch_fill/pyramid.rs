//! The image pyramid a fill is solved on, coarse to fine.

use super::distance::hole_radius;
use super::field::Sources;
use super::membrane::sample_bilinear;
use super::{PatchFillError, PerceptualSpace, PATCH_RADIUS};
use rayon::prelude::*;

/// Matching features per pixel: perceptual colour, then the texture features
/// of Newson et al. (mean absolute horizontal and vertical luma gradient),
/// which tell a textured region from a flat one of the same mean colour.
pub(super) const FEATURES: usize = 5;
pub(super) type Feature = [f32; FEATURES];

/// Scale of the texture features relative to colour. Squared differences
/// between misaligned patches of a stochastic texture (foliage, gravel) are
/// about twice its variance, against once for a flat patch of the same mean,
/// so colour alone prefers flat sources; the texture term outweighs that.
const TEXTURE_WEIGHT: f32 = 4.0;
/// Gradients are averaged over the patch footprint (Newson et al.).
const TEXTURE_RADIUS: usize = PATCH_RADIUS;
/// The coarsest level is reached once no hole pixel lies further than this
/// from known pixels, so every patch there still overlaps real content.
const COARSEST_HOLE_RADIUS: f32 = PATCH_RADIUS as f32;
/// Levels smaller than this, or with fewer sources, carry too little
/// content to search and end the pyramid.
const MIN_LEVEL_EDGE: usize = 4 * (2 * PATCH_RADIUS + 1);
const MIN_LEVEL_SOURCES: usize = 64;
const MAX_LEVELS: usize = 12;
/// Smallest distance scale of the locality cost, in finest-level pixels, so
/// thin holes still sample a useful neighbourhood.
const MIN_LOCALITY_REACH: f32 = 24.0;
/// Binomial taps for the 2x downsampling, centred between taps 1 and 2.
const DOWNSAMPLE_TAPS: [f32; 4] = [1.0, 3.0, 3.0, 1.0];

pub(super) struct Level {
    pub(super) width: usize,
    pub(super) height: usize,
    /// Perceptual colour; hole pixels hold the current estimate.
    pub(super) guide: Vec<[f32; 3]>,
    pub(super) hole: Vec<bool>,
    /// Indices of the hole pixels, ascending.
    pub(super) hole_pixels: Vec<u32>,
    /// Matching features, kept in step with `guide` in the hole.
    pub(super) features: Vec<Feature>,
    pub(super) sources: Sources,
}

impl Level {
    fn new(
        width: usize,
        height: usize,
        guide: Vec<[f32; 3]>,
        hole: Vec<bool>,
        sources: Sources,
    ) -> Self {
        let hole_pixels = hole
            .iter()
            .enumerate()
            .filter(|(_, hole)| **hole)
            .map(|(index, _)| index as u32)
            .collect();
        Self {
            width,
            height,
            guide,
            hole,
            hole_pixels,
            features: Vec::new(),
            sources,
        }
    }

    /// Recomputes every pixel's features from the guide.
    pub(super) fn refresh_features(&mut self) {
        let (width, height) = (self.width, self.height);
        let luma = self
            .guide
            .iter()
            .map(|rgb| 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2])
            .collect::<Vec<_>>();
        let mut gradient = vec![[0.0f32; 2]; width * height];
        gradient
            .par_chunks_mut(width)
            .enumerate()
            .for_each(|(y, row)| {
                let (up, down) = (y.saturating_sub(1), (y + 1).min(height - 1));
                for (x, value) in row.iter_mut().enumerate() {
                    let (left, right) = (x.saturating_sub(1), (x + 1).min(width - 1));
                    *value = [
                        (luma[y * width + right] - luma[y * width + left]).abs() * 0.5,
                        (luma[down * width + x] - luma[up * width + x]).abs() * 0.5,
                    ];
                }
            });
        let texture = box_mean(&gradient, width, height, TEXTURE_RADIUS);
        self.features = self
            .guide
            .par_iter()
            .zip(texture.par_iter())
            .map(|(rgb, texture)| {
                [
                    rgb[0],
                    rgb[1],
                    rgb[2],
                    texture[0] * TEXTURE_WEIGHT,
                    texture[1] * TEXTURE_WEIGHT,
                ]
            })
            .collect();
    }

    /// Starts the hole from the finished coarser level, interpolated up.
    pub(super) fn upsample_hole_from(&mut self, coarse: &Level) {
        for &index in &self.hole_pixels {
            let index = index as usize;
            let (x, y) = (index % self.width, index / self.width);
            self.guide[index] = sample_bilinear(
                &coarse.guide,
                coarse.width,
                coarse.height,
                (x as f32 + 0.5) * 0.5 - 0.5,
                (y as f32 + 0.5) * 0.5 - 0.5,
            );
        }
    }
}

/// Builds the pyramid, finest level first. Coarser levels mark a pixel as
/// hole when any pixel below it is, and as copyable only when all are, so the
/// object being removed never leaks into a coarse level.
pub(super) fn build(
    perceptual: &[[f32; 3]],
    hole: &[bool],
    allowed: Vec<bool>,
    width: usize,
    height: usize,
) -> Result<Vec<Level>, PatchFillError> {
    let radius = hole_radius(hole, width, height) as f32;
    let reach = radius.max(MIN_LOCALITY_REACH);
    let sources = Sources::new(&allowed, hole, width, height, reach);
    if sources.is_empty() {
        return Err(PatchFillError::NoSource);
    }
    let mut levels = vec![Level::new(
        width,
        height,
        perceptual.to_vec(),
        hole.to_vec(),
        sources,
    )];
    let mut allowed = allowed;
    while levels.len() < MAX_LEVELS {
        let Some(finer) = levels.last() else {
            break;
        };
        let scale = (1u32 << (levels.len() - 1)) as f32;
        if radius / scale <= COARSEST_HOLE_RADIUS {
            break;
        }
        let (coarse_width, coarse_height) = (finer.width.div_ceil(2), finer.height.div_ceil(2));
        if coarse_width.min(coarse_height) < MIN_LEVEL_EDGE {
            break;
        }
        let coarse_hole = downsample_mask(&finer.hole, finer.width, finer.height, true);
        let coarse_allowed = downsample_mask(&allowed, finer.width, finer.height, false);
        let sources = Sources::new(
            &coarse_allowed,
            &coarse_hole,
            coarse_width,
            coarse_height,
            reach / (2.0 * scale),
        );
        if sources.len() < MIN_LEVEL_SOURCES {
            break;
        }
        let guide = downsample_guide(&finer.guide, &finer.hole, finer.width, finer.height);
        levels.push(Level::new(
            coarse_width,
            coarse_height,
            guide,
            coarse_hole,
            sources,
        ));
        allowed = coarse_allowed;
    }
    Ok(levels)
}

/// The guide of a guided fill, perceptually encoded and halved `level` times
/// with the pyramid's own filter, so it lines up with that level.
pub(super) fn guide_at_level(
    guide: &[[f32; 3]],
    space: &impl PerceptualSpace,
    width: usize,
    height: usize,
    level: usize,
) -> Vec<[f32; 3]> {
    let mut values = guide
        .par_iter()
        .map(|value| space.encode(*value))
        .collect::<Vec<_>>();
    let (mut width, mut height) = (width, height);
    for _ in 0..level {
        values = downsample_where(&values, width, height, |_| true);
        (width, height) = (width.div_ceil(2), height.div_ceil(2));
    }
    values
}

/// Halves a mask: a coarse pixel is set when any (`any`) or every (`!any`)
/// fine pixel below it is.
fn downsample_mask(mask: &[bool], width: usize, height: usize, any: bool) -> Vec<bool> {
    let (coarse_width, coarse_height) = (width.div_ceil(2), height.div_ceil(2));
    let mut coarse = vec![false; coarse_width * coarse_height];
    for coarse_y in 0..coarse_height {
        for coarse_x in 0..coarse_width {
            let mut children = (2 * coarse_y..(2 * coarse_y + 2).min(height)).flat_map(|y| {
                (2 * coarse_x..(2 * coarse_x + 2).min(width)).map(move |x| mask[y * width + x])
            });
            coarse[coarse_y * coarse_width + coarse_x] = if any {
                children.any(|set| set)
            } else {
                children.all(|set| set)
            };
        }
    }
    coarse
}

/// Halves the guide with a binomial filter over known pixels only, so hole
/// content never bleeds into the coarse context.
fn downsample_guide(
    guide: &[[f32; 3]],
    hole: &[bool],
    width: usize,
    height: usize,
) -> Vec<[f32; 3]> {
    downsample_where(guide, width, height, |index| !hole[index])
}

/// Halves an image with the binomial filter over the pixels `known` admits.
fn downsample_where(
    guide: &[[f32; 3]],
    width: usize,
    height: usize,
    known: impl Fn(usize) -> bool + Sync,
) -> Vec<[f32; 3]> {
    let (coarse_width, coarse_height) = (width.div_ceil(2), height.div_ceil(2));
    let mut coarse = vec![[0.0f32; 3]; coarse_width * coarse_height];
    coarse
        .par_chunks_mut(coarse_width)
        .enumerate()
        .for_each(|(coarse_y, row)| {
            for (coarse_x, value) in row.iter_mut().enumerate() {
                let mut sum = [0.0f32; 3];
                let mut total = 0.0f32;
                for (tap_y, weight_y) in DOWNSAMPLE_TAPS.iter().enumerate() {
                    let Some(y) = (2 * coarse_y + tap_y)
                        .checked_sub(1)
                        .filter(|y| *y < height)
                    else {
                        continue;
                    };
                    for (tap_x, weight_x) in DOWNSAMPLE_TAPS.iter().enumerate() {
                        let Some(x) = (2 * coarse_x + tap_x).checked_sub(1).filter(|x| *x < width)
                        else {
                            continue;
                        };
                        let index = y * width + x;
                        if !known(index) {
                            continue;
                        }
                        let weight = weight_x * weight_y;
                        for channel in 0..3 {
                            sum[channel] += guide[index][channel] * weight;
                        }
                        total += weight;
                    }
                }
                if total > 0.0 {
                    *value = sum.map(|channel| channel / total);
                }
            }
        });
    coarse
}

/// Mean over the `(2 * radius + 1)` square around each pixel, clipped to the
/// grid, computed separably with running sums.
fn box_mean(values: &[[f32; 2]], width: usize, height: usize, radius: usize) -> Vec<[f32; 2]> {
    let mut rows = vec![[0.0f32; 2]; width * height];
    rows.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        let source = &values[y * width..(y + 1) * width];
        for (x, value) in row.iter_mut().enumerate() {
            let window = &source[x.saturating_sub(radius)..(x + radius + 1).min(width)];
            let sum = window.iter().fold([0.0f32; 2], |sum, value| {
                [sum[0] + value[0], sum[1] + value[1]]
            });
            *value = sum.map(|sum| sum / window.len() as f32);
        }
    });
    let mut out = vec![[0.0f32; 2]; width * height];
    out.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        let (top, bottom) = (y.saturating_sub(radius), (y + radius + 1).min(height));
        for (x, value) in row.iter_mut().enumerate() {
            let mut sum = [0.0f32; 2];
            for sample_y in top..bottom {
                let sample = rows[sample_y * width + x];
                sum[0] += sample[0];
                sum[1] += sample[1];
            }
            *value = sum.map(|sum| sum / (bottom - top) as f32);
        }
    });
    out
}
