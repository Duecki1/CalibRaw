//! Sky masks from SkySeg U2Net.

use super::masks::mask_model_retention;
use super::{mask_refine, normalized_rgb_input};
use crate::registry::{AiModel, ModelInfo};
use crate::slot::with_model_session;
use anyhow::{Context, Result};
use image::{imageops::FilterType, ImageBuffer, Rgba};
use ort::value::Tensor;
use std::path::Path;

pub const MODEL: ModelInfo = AiModel::SkySeg.spec().info();
const INPUT_EDGE: u32 = MODEL.input.edge();

pub fn skyseg_model_is_verified(path: &Path) -> bool {
    AiModel::SkySeg.spec().is_installed(path)
}

pub(super) fn sky_mask(
    model_path: &Path,
    image: &ImageBuffer<Rgba<u8>, Vec<u8>>,
) -> Result<Vec<u8>> {
    let resized = image::imageops::resize(image, INPUT_EDGE, INPUT_EDGE, FilterType::Lanczos3);
    let input = Tensor::from_array((
        [1usize, 3, INPUT_EDGE as usize, INPUT_EDGE as usize],
        normalized_rgb_input(&resized)?,
    ))
    .context("create sky segmentation input tensor")?;
    let (output_width, output_height, sky_probabilities) = with_model_session(
        AiModel::SkySeg,
        model_path,
        AiModel::SkySeg.spec().session,
        mask_model_retention(true),
        |session| {
            session.run_with_fallback("SkySeg ONNX inference", |ort_session, _| {
                let outputs = ort_session
                    .run(ort::inputs![&input])
                    .context("run SkySeg ONNX inference")?;
                let output = outputs
                    .get("1959")
                    .context("SkySeg returned no primary sky output")?;
                let (shape, probabilities) = output
                    .try_extract_tensor::<f32>()
                    .context("read SkySeg output tensor")?;
                let (width, height) = validate_output_shape(shape, probabilities.len())?;
                Ok((width, height, sky_probabilities_to_mask(probabilities)?))
            })
        },
    )?;
    let mask = image::GrayImage::from_raw(output_width, output_height, sky_probabilities)
        .context("SkySeg returned an invalid sky mask")?;
    let mut mask =
        image::imageops::resize(&mask, image.width(), image.height(), FilterType::Triangle)
            .into_raw();
    mask_refine::guided_filter_color(
        image.as_raw(),
        &mut mask,
        image.width(),
        image.height(),
        8,
        1e-4,
    )?;
    Ok(mask)
}

fn validate_output_shape(shape: &[i64], len: usize) -> Result<(u32, u32)> {
    anyhow::ensure!(
        shape.len() == 4 && shape[0] == 1 && shape[1] == 1,
        "unexpected SkySeg output shape {shape:?}; expected [1, 1, H, W]"
    );
    let height = usize::try_from(shape[2]).context("invalid SkySeg output height")?;
    let width = usize::try_from(shape[3]).context("invalid SkySeg output width")?;
    anyhow::ensure!(
        width > 0 && height > 0 && width <= INPUT_EDGE as usize && height <= INPUT_EDGE as usize,
        "SkySeg output dimensions are invalid: {shape:?}"
    );
    anyhow::ensure!(
        len == width * height,
        "SkySeg output tensor length does not match {shape:?}"
    );
    Ok((width as u32, height as u32))
}

fn sky_probabilities_to_mask(probabilities: &[f32]) -> Result<Vec<u8>> {
    let mut result = Vec::with_capacity(probabilities.len());
    for &probability in probabilities {
        anyhow::ensure!(probability.is_finite(), "SkySeg returned non-finite values");
        result.push((probability.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
    }
    Ok(result)
}
