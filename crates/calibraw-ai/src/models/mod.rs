//! Each feature's model code: preparing the input, running its sessions and
//! turning the output into a mask, a fill or denoised pixels. Sessions,
//! downloads and the worker thread come from the rest of the crate.

pub mod denoise;
pub mod depth;
mod depth_refine;
mod mask_refine;
pub mod masks;
pub mod object;
pub mod remove;
pub mod sky;
pub mod subject;

use anyhow::{Context, Result};
use image::{ImageBuffer, Rgba};

const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

/// Planar RGB normalized with the ImageNet mean and deviation, the input
/// BiRefNet, SkySeg, Depth Anything and SAM were trained on.
pub(crate) fn normalized_rgb_input(resized: &ImageBuffer<Rgba<u8>, Vec<u8>>) -> Result<Vec<f32>> {
    let (input_width, input_height) = resized.dimensions();
    let plane = usize::try_from(input_width)
        .ok()
        .and_then(|width| {
            usize::try_from(input_height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .context("AI input dimensions overflow")?;
    let values = plane.checked_mul(3).context("AI input size overflow")?;
    let mut input = Vec::new();
    input
        .try_reserve_exact(values)
        .context("reserve AI input tensor")?;
    input.resize(values, 0.0);
    for y in 0..input_height {
        for x in 0..input_width {
            let pixel = resized.get_pixel(x, y);
            let destination = (y * input_width + x) as usize;
            for channel in 0..3 {
                let value = pixel[channel] as f32 / 255.0;
                input[channel * plane + destination] =
                    (value - IMAGENET_MEAN[channel]) / IMAGENET_STD[channel];
            }
        }
    }
    Ok(input)
}

/// The probability a logit stands for, without overflow for large ones.
pub(crate) fn sigmoid_probability(logit: f32) -> f32 {
    if logit >= 0.0 {
        1.0 / (1.0 + (-logit).exp())
    } else {
        let exponential = logit.exp();
        exponential / (1.0 + exponential)
    }
}
