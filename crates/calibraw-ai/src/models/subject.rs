//! Subject masks from BiRefNet General-Lite.

use super::mask_refine::{self, MaskCrop};
use super::masks::mask_model_retention;
use super::{normalized_rgb_input, sigmoid_probability};
use crate::config::{MemoryClass, CAPABILITIES};
use crate::placement::FallbackSession;
use crate::registry::{AiModel, ModelInfo, BIREFNET_FLOAT16_ON_GPU};
use crate::slot::with_model_session;
use anyhow::{Context, Result};
use image::{imageops::FilterType, ImageBuffer, Luma, Rgba};
use ort::value::Tensor;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Numeric format of the subject model file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SubjectModelPrecision {
    #[default]
    Float32,
    /// For GPU sessions. CPU providers have few float16 kernels and run it
    /// about 30% slower than `Float32`.
    Float16,
}

impl SubjectModelPrecision {
    /// The precision for subject masks with the current settings: float16
    /// where it is confirmed on the GPU provider (`BIREFNET_FLOAT16_ON_GPU`,
    /// so nowhere at present) and sessions are expected on it, i.e. with GPU
    /// acceleration enabled and Automatic mode's runtime built with a GPU
    /// provider. A manually selected runtime is not known before it loads, so
    /// it keeps float32.
    pub fn for_current_sessions(manual_runtime: bool) -> Self {
        let gpu = BIREFNET_FLOAT16_ON_GPU
            && !manual_runtime
            && crate::runtime::ai_sessions_may_use_gpu(false);
        if gpu {
            Self::Float16
        } else {
            Self::Float32
        }
    }
}

/// How much work a subject mask may take. Serialized in the performance
/// settings; keep the names stable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectMaskQuality {
    /// The 768 x 768 model or its float16 conversion, one pass, plus the
    /// crop pass for a small subject.
    Low,
    /// The 1024 x 1024 model or its float16 conversion, one pass, plus the
    /// crop pass for a small subject.
    #[default]
    Medium,
    /// `Medium`'s model, always with the crop pass. Needs
    /// `MemoryClass::Standard`.
    High,
}

impl SubjectMaskQuality {
    /// The qualities this device offers.
    pub const AVAILABLE: &'static [Self] = Self::available(CAPABILITIES.memory);

    const fn available(memory: MemoryClass) -> &'static [Self] {
        match memory {
            MemoryClass::Constrained => &[Self::Low, Self::Medium],
            MemoryClass::Standard => &[Self::Low, Self::Medium, Self::High],
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }

    pub const fn model(self, precision: SubjectModelPrecision) -> ModelInfo {
        self.ai_model(precision).spec().info()
    }

    pub(super) const fn ai_model(self, precision: SubjectModelPrecision) -> AiModel {
        match (self, precision) {
            (Self::Low, SubjectModelPrecision::Float32) => AiModel::BiRefNetLow,
            (Self::Low, SubjectModelPrecision::Float16) => AiModel::BiRefNetLowFp16,
            (Self::Medium | Self::High, SubjectModelPrecision::Float32) => AiModel::BiRefNet,
            (Self::Medium | Self::High, SubjectModelPrecision::Float16) => AiModel::BiRefNetFp16,
        }
    }

    const fn crop_refinement(self) -> bool {
        matches!(self, Self::High)
    }
}

pub fn birefnet_model_is_verified(
    quality: SubjectMaskQuality,
    precision: SubjectModelPrecision,
    path: &Path,
) -> bool {
    quality.ai_model(precision).spec().is_installed(path)
}

/// Runs BiRefNet over the full frame, then once more over the subject's
/// bounding crop for finer boundary detail: always for `High` with standard
/// memory, and at every quality when the subject is small in the frame.
pub(super) fn subject_mask(
    model_path: &Path,
    image: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    quality: SubjectMaskQuality,
    precision: SubjectModelPrecision,
) -> Result<Vec<u8>> {
    let (width, height) = image.dimensions();
    let model = quality.ai_model(precision);
    let spec = model.spec();
    let edge = spec.input.edge();
    let crop_refinement =
        subject_crop_refinement_enabled(quality.crop_refinement(), CAPABILITIES.memory);
    // Both passes share one session, so a one-shot session loads only once.
    let mut mask = with_model_session(
        model,
        model_path,
        spec.session,
        mask_model_retention(true),
        |session| {
            let mut mask = infer_birefnet(session, image, edge, image.dimensions())?;
            let crop = mask_refine::mask_crop_above(&mask, width, height, 5, 0.15)
                .filter(|&crop| crop_refinement || is_small_subject(crop, width, height));
            if let Some(crop) = crop {
                let crop_image =
                    image::imageops::crop_imm(image, crop.x, crop.y, crop.width, crop.height)
                        .to_image();
                let crop_alpha =
                    infer_birefnet(session, &crop_image, edge, crop_image.dimensions())?;
                mask_refine::merge_crop_pass(&mut mask, width, crop, &crop_alpha, 8);
            }
            Ok(mask)
        },
    )?;
    mask_refine::guided_filter_color(image.as_raw(), &mut mask, width, height, 8, 1e-4)?;
    mask_refine::harden_model_alpha(&mut mask);
    Ok(mask)
}

const fn subject_crop_refinement_enabled(requested: bool, memory: MemoryClass) -> bool {
    requested && matches!(memory, MemoryClass::Standard)
}

/// The full-frame pass squeezes the frame to the model's input edge, so a
/// subject that spans little of it gets few model pixels and a coarse
/// outline. A subject crop at most `1 / SMALL_SUBJECT_MAGNIFICATION` of the
/// frame on both axes gets the crop pass, which enlarges it at least this
/// many times. The crop is small then, so the pass costs little memory.
const SMALL_SUBJECT_MAGNIFICATION: u64 = 3;

fn is_small_subject(crop: MaskCrop, width: u32, height: u32) -> bool {
    u64::from(crop.width) * SMALL_SUBJECT_MAGNIFICATION <= u64::from(width)
        && u64::from(crop.height) * SMALL_SUBJECT_MAGNIFICATION <= u64::from(height)
}

/// One BiRefNet pass over `image`, resized to the model's square `edge`. The
/// returned probabilities, by row, are `output` (width, height) pixels
/// stretched over `image`.
pub(super) fn infer_birefnet(
    session: &mut FallbackSession,
    image: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    edge: u32,
    output: (u32, u32),
) -> Result<Vec<u8>> {
    let resized = image::imageops::resize(image, edge, edge, FilterType::Lanczos3);
    let input = normalized_rgb_input(&resized)?;
    let input = Tensor::from_array(([1usize, 3, edge as usize, edge as usize], input))
        .context("create BiRefNet input tensor")?;

    let (output_width, output_height, logits) = run_subject_session(session, input, edge, edge)?;

    restore_birefnet_output(&logits, output_width, output_height, output.0, output.1)
}

fn run_subject_session(
    session: &mut FallbackSession,
    input: Tensor<f32>,
    input_width: u32,
    input_height: u32,
) -> Result<(u32, u32, Vec<f32>)> {
    session.run_with_fallback("BiRefNet ONNX inference", |ort_session, _accelerated| {
        let outputs = ort_session
            .run(ort::inputs![&input])
            .context("run BiRefNet ONNX inference")?;
        let output = outputs
            .values()
            .next()
            .context("BiRefNet returned no output tensors")?;
        let (shape, logits) = output
            .try_extract_tensor::<f32>()
            .context("read BiRefNet output tensor")?;
        let (output_width, output_height, output_elements) =
            validate_birefnet_output_shape(shape, logits.len(), input_width, input_height)?;
        anyhow::ensure!(
            logits.iter().all(|value| value.is_finite()),
            "BiRefNet output contains non-finite logits"
        );
        let mut owned_logits = Vec::new();
        owned_logits
            .try_reserve_exact(output_elements)
            .context("reserve BiRefNet output logits")?;
        owned_logits.extend_from_slice(logits);
        Ok((output_width, output_height, owned_logits))
    })
}
fn validate_birefnet_output_shape(
    shape: &[i64],
    logits_len: usize,
    input_width: u32,
    input_height: u32,
) -> Result<(u32, u32, usize)> {
    anyhow::ensure!(
        shape.len() == 4 && shape[0] == 1 && shape[1] == 1,
        "unexpected BiRefNet output shape {shape:?}; expected [1, 1, H, W]"
    );
    let output_height =
        usize::try_from(shape[2]).context("BiRefNet output height is negative or too large")?;
    let output_width =
        usize::try_from(shape[3]).context("BiRefNet output width is negative or too large")?;
    anyhow::ensure!(
        output_width > 0 && output_height > 0,
        "BiRefNet output dimensions must be positive: {shape:?}"
    );
    let output_elements = output_width
        .checked_mul(output_height)
        .context("BiRefNet output dimensions overflow")?;
    anyhow::ensure!(
        output_elements <= input_width as usize * input_height as usize * 4,
        "BiRefNet output is implausibly large: {shape:?}"
    );
    anyhow::ensure!(
        logits_len == output_elements,
        "BiRefNet output shape {shape:?} describes {output_elements} values, but the tensor contains {logits_len}"
    );
    Ok((
        u32::try_from(output_width).context("BiRefNet output width exceeds u32")?,
        u32::try_from(output_height).context("BiRefNet output height exceeds u32")?,
        output_elements,
    ))
}

fn restore_birefnet_output(
    logits: &[f32],
    output_width: u32,
    output_height: u32,
    target_width: u32,
    target_height: u32,
) -> Result<Vec<u8>> {
    let output_elements = usize::try_from(output_width)
        .ok()
        .and_then(|width| {
            usize::try_from(output_height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .context("BiRefNet output dimensions overflow")?;
    anyhow::ensure!(
        logits.len() == output_elements,
        "BiRefNet logits do not match the declared output dimensions"
    );
    let mut probabilities = Vec::new();
    probabilities
        .try_reserve_exact(output_elements)
        .context("reserve BiRefNet probability map")?;
    for &logit in logits {
        anyhow::ensure!(
            logit.is_finite(),
            "BiRefNet output contains a non-finite logit"
        );
        probabilities.push(sigmoid_probability(logit));
    }
    let output =
        ImageBuffer::<Luma<f32>, Vec<f32>>::from_raw(output_width, output_height, probabilities)
            .context("invalid BiRefNet output buffer")?;
    let resized =
        image::imageops::resize(&output, target_width, target_height, FilterType::Lanczos3);
    Ok(resized
        .into_raw()
        .into_iter()
        .map(|probability| (probability.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
        .collect())
}
