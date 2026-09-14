//! Exposure-normalized HDR in linear, unbalanced camera RGB. Never tone maps or clamps radiance.
//! Clipping weights follow the principle of excluding saturated measurements described in
//! https://www.pauldebevec.com/Research/HDR/ (the RAW response is already linear).

mod alignment;
pub use alignment::{Alignment, AlignmentReference};

use super::raw_loader::validate_raw_dimensions;
use anyhow::{ensure, Result};
use rayon::prelude::*;

pub fn exposure_scale(shutter_seconds: f32, iso: f32, aperture: f32) -> Result<f32> {
    ensure!(
        [shutter_seconds, iso, aperture]
            .iter()
            .all(|v| v.is_finite() && *v > 0.0),
        "HDR merge needs valid shutter speed, ISO and aperture metadata in every source"
    );
    let scale = shutter_seconds * iso / aperture.powi(2);
    ensure!(
        scale.is_finite() && scale > 0.0,
        "Invalid HDR exposure scale"
    );
    Ok(scale)
}

pub(super) fn validate_rgb(width: u32, height: u32, rgb: &[f32], exposure: f32) -> Result<()> {
    let pixels = validate_raw_dimensions(width, height)?;
    ensure!(
        rgb.len() == pixels * 3,
        "HDR RGB dimensions do not match the buffer"
    );
    ensure!(
        exposure.is_finite() && exposure > 0.0,
        "Invalid HDR relative exposure"
    );
    ensure!(
        rgb.par_iter().all(|v| v.is_finite()),
        "HDR source contains NaN or infinity"
    );
    Ok(())
}

/// A streaming accumulator: only one decoded bracket needs to be resident at a time.
/// RGB channels share a weight so a clipped channel cannot contaminate the merged hue.
pub struct HdrAccumulator {
    width: u32,
    height: u32,
    pixels: Vec<MergePixel>,
}

#[derive(Clone, Copy, Default)]
struct MergePixel {
    sum: [f32; 3],
    weight: f32,
    fallback: [f32; 3],
    fallback_score: f32,
}

impl HdrAccumulator {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        let count = validate_raw_dimensions(width, height)?;
        Ok(Self {
            width,
            height,
            pixels: vec![MergePixel::default(); count],
        })
    }

    /// `rgb` must be black-subtracted, white-normalized camera RGB without white balance.
    /// `clipped` optionally marks demosaic footprints containing saturated sensor samples.
    /// `exposure` is relative to the reference frame. Out-of-frame samples are excluded.
    pub fn add(
        &mut self,
        rgb: &[f32],
        exposure: f32,
        alignment: Alignment,
        clipped: Option<&[u8]>,
    ) -> Result<()> {
        validate_rgb(self.width, self.height, rgb, exposure)?;
        ensure!(alignment.is_finite(), "Invalid HDR alignment");
        if let Some(clipped) = clipped {
            ensure!(
                clipped.len() == self.pixels.len(),
                "HDR clipping mask dimensions do not match"
            );
        }
        let width = self.width as usize;
        let height = self.height as usize;
        let transform = alignment.transform(width, height);
        self.pixels
            .par_iter_mut()
            .enumerate()
            .for_each(|(i, merged)| {
                let (x, y) = transform.map((i % width) as f32, (i / width) as f32);
                let Some((indices, fractions)) = sample_coordinates(width, height, x, y) else {
                    return;
                };
                let mut sample = [0.0; 3];
                let mut peak = 0.0f32;
                for (index, fraction) in indices.into_iter().zip(fractions) {
                    if fraction <= 0.0 {
                        continue;
                    }
                    for c in 0..3 {
                        let value = rgb[index * 3 + c];
                        sample[c] += fraction * value;
                        // Test clipping BEFORE interpolation, including every contributing channel.
                        peak = peak.max(value);
                    }
                }
                if let Some(clipped) = clipped {
                    if indices
                        .into_iter()
                        .zip(fractions)
                        .any(|(i, f)| f > 0.0 && clipped[i] != 0)
                    {
                        peak = peak.max(1.0);
                    }
                }
                let signal = sample.iter().copied().fold(0.0f32, f32::max);
                let highlight_weight = ((0.98 - peak) / 0.08).clamp(0.0, 1.0);
                let shadow_weight = (signal / 0.02).clamp(0.0, 1.0);
                // Longer exposures have better shot-noise SNR after exposure normalization.
                let weight = exposure * highlight_weight * shadow_weight;
                for (c, value) in sample.iter().enumerate() {
                    merged.sum[c] += weight * (*value / exposure);
                }
                merged.weight += weight;
                // If every exposure clips, retain the shortest; if every exposure is black,
                // retain the longest. Never turn missing coverage into black borders.
                let score = if peak >= 0.98 {
                    1.0 / (1.0 + exposure)
                } else {
                    2.0 + exposure
                };
                if score > merged.fallback_score {
                    merged.fallback = sample.map(|v| v / exposure);
                    merged.fallback_score = score;
                }
            });
        Ok(())
    }

    pub fn finish(self) -> Result<Vec<f32>> {
        ensure!(
            self.pixels.iter().all(|p| p.fallback_score > 0.0),
            "HDR merge has uncovered pixels"
        );
        let mut rgb = vec![0.0; self.pixels.len() * 3];
        rgb.par_chunks_exact_mut(3)
            .zip(self.pixels.par_iter())
            .for_each(|(rgb, p)| {
                for (c, value) in rgb.iter_mut().enumerate() {
                    *value = if p.weight > 0.0 {
                        p.sum[c] / p.weight
                    } else {
                        p.fallback[c]
                    };
                }
            });
        ensure!(
            rgb.par_iter().all(|v| v.is_finite()),
            "HDR radiance exceeded floating-point range"
        );
        Ok(rgb)
    }
}

pub(super) fn sample_coordinates(
    width: usize,
    height: usize,
    x: f32,
    y: f32,
) -> Option<([usize; 4], [f32; 4])> {
    if x < 0.0 || y < 0.0 || x > (width - 1) as f32 || y > (height - 1) as f32 {
        return None;
    }
    let ix = x.floor() as usize;
    let iy = y.floor() as usize;
    let jx = (ix + 1).min(width - 1);
    let jy = (iy + 1).min(height - 1);
    let dx = x - ix as f32;
    let dy = y - iy as f32;
    Some((
        [
            iy * width + ix,
            iy * width + jx,
            jy * width + ix,
            jy * width + jx,
        ],
        [
            (1.0 - dx) * (1.0 - dy),
            dx * (1.0 - dy),
            (1.0 - dx) * dy,
            dx * dy,
        ],
    ))
}

#[cfg(test)]
mod tests;
