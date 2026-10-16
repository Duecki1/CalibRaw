//! Subject masks from BiRefNet General-Lite.

use super::masks::mask_model_retention;
use super::{mask_refine, normalized_rgb_input, sigmoid_probability};
use crate::placement::FallbackSession;
use crate::registry::{AiModel, ModelInfo};
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
    /// where sessions are expected on DirectML or CUDA, i.e. Windows and
    /// Linux with GPU acceleration enabled and Automatic mode's runtime built
    /// with a GPU provider. A manually selected runtime is not known before it
    /// loads, so it keeps float32, as does CoreML, which has not been tested
    /// with float16.
    pub fn for_current_sessions(manual_runtime: bool) -> Self {
        #[cfg(not(target_os = "android"))]
        let gpu = cfg!(any(target_os = "windows", target_os = "linux"))
            && !manual_runtime
            && crate::runtime::ai_sessions_may_use_gpu(false);
        #[cfg(target_os = "android")]
        let gpu = {
            let _ = manual_runtime;
            false
        };
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
    /// The 768 x 768 model or its float16 conversion, one pass.
    Low,
    /// The 1024 x 1024 model or its float16 conversion, one pass.
    #[default]
    Medium,
    /// `Medium`'s model with the cropped refinement pass. Desktop only.
    High,
}

impl SubjectMaskQuality {
    /// The qualities this platform offers.
    pub const AVAILABLE: &'static [Self] = if cfg!(target_os = "android") {
        &[Self::Low, Self::Medium]
    } else {
        &[Self::Low, Self::Medium, Self::High]
    };

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

/// Runs BiRefNet over the full frame and, for `High` on desktop, follows it
/// with one native-resolution crop pass for finer boundary detail.
pub(super) fn subject_mask(
    model_path: &Path,
    image: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    quality: SubjectMaskQuality,
    precision: SubjectModelPrecision,
) -> Result<Vec<u8>> {
    let (width, height) = image.dimensions();
    let mut mask = infer_birefnet(model_path, image, quality, precision)?;
    let crop_refinement =
        subject_crop_refinement_enabled(quality.crop_refinement(), cfg!(target_os = "android"));
    if crop_refinement {
        if let Some(crop) = mask_refine::mask_crop_above(&mask, width, height, 5, 0.15) {
            let crop_image =
                image::imageops::crop_imm(image, crop.x, crop.y, crop.width, crop.height)
                    .to_image();
            let crop_alpha = infer_birefnet(model_path, &crop_image, quality, precision)?;
            mask_refine::merge_crop_pass(&mut mask, width, crop, &crop_alpha, 8);
        }
    }
    mask_refine::guided_filter_color(image.as_raw(), &mut mask, width, height, 8, 1e-4)?;
    mask_refine::harden_model_alpha(&mut mask);
    Ok(mask)
}

const fn subject_crop_refinement_enabled(requested: bool, android: bool) -> bool {
    requested && !android
}

fn infer_birefnet(
    model_path: &Path,
    image: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    quality: SubjectMaskQuality,
    precision: SubjectModelPrecision,
) -> Result<Vec<u8>> {
    let model = quality.ai_model(precision);
    let spec = model.spec();
    let edge = spec.input.edge();
    let resized = image::imageops::resize(image, edge, edge, FilterType::Lanczos3);
    let input = normalized_rgb_input(&resized)?;
    let input = Tensor::from_array(([1usize, 3, edge as usize, edge as usize], input))
        .context("create BiRefNet input tensor")?;

    let (output_width, output_height, logits) = with_model_session(
        model,
        model_path,
        spec.session,
        mask_model_retention(true),
        |session| run_subject_session(session, input, edge, edge),
    )?;

    restore_birefnet_output(
        &logits,
        output_width,
        output_height,
        image.width(),
        image.height(),
    )
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

#[cfg(test)]
mod tests {
    use super::{
        birefnet_model_is_verified, infer_birefnet, restore_birefnet_output,
        subject_crop_refinement_enabled, validate_birefnet_output_shape, SubjectMaskQuality,
        SubjectModelPrecision,
    };
    use crate::registry::AiModel;
    use image::{ImageBuffer, Rgba};
    use std::path::Path;

    /// `CALIBRAW_BIREFNET_MODEL` may be either published subject model.
    #[test]
    #[ignore = "requires CALIBRAW_BIREFNET_MODEL and ONNX Runtime via ORT_DYLIB_PATH"]
    fn published_birefnet_runs_on_the_expected_provider() {
        let path = std::env::var_os("CALIBRAW_BIREFNET_MODEL").expect("CALIBRAW_BIREFNET_MODEL");
        let runtime = std::env::var_os("ORT_DYLIB_PATH").expect("ORT_DYLIB_PATH");
        ort::init_from(Path::new(&runtime)).unwrap().commit();
        let (quality, precision) = [SubjectMaskQuality::Low, SubjectMaskQuality::Medium]
            .into_iter()
            .flat_map(|quality| {
                [
                    SubjectModelPrecision::Float32,
                    SubjectModelPrecision::Float16,
                ]
                .map(|precision| (quality, precision))
            })
            .find(|&(quality, precision)| {
                birefnet_model_is_verified(quality, precision, Path::new(&path))
            })
            .expect("CALIBRAW_BIREFNET_MODEL is not a published subject model");
        let image = ImageBuffer::from_fn(96, 64, |x, y| {
            Rgba([(x * 255 / 95) as u8, (y * 255 / 63) as u8, 128, 255])
        });
        let mask = infer_birefnet(Path::new(&path), &image, quality, precision).unwrap();
        assert_eq!(mask.len(), 96 * 64);
        let providers = crate::active_execution_providers();
        eprintln!("BiRefNet inference providers: {providers:?}");
        if let Ok(expected) = std::env::var("CALIBRAW_TEST_BIREFNET_PROVIDER") {
            assert!(providers.iter().any(|status| {
                status.model_name == "BiRefNet"
                    && status.active_provider == expected
                    && !status.degraded
            }));
        }
    }

    #[test]
    fn android_always_disables_subject_crop_refinement() {
        assert!(subject_crop_refinement_enabled(true, false));
        assert!(!subject_crop_refinement_enabled(false, false));
        assert!(!subject_crop_refinement_enabled(true, true));
    }

    #[test]
    fn subject_qualities_choose_model_and_crop_pass() {
        use SubjectModelPrecision::{Float16, Float32};
        assert_eq!(
            SubjectMaskQuality::Low.ai_model(Float32),
            AiModel::BiRefNetLow
        );
        assert_eq!(
            SubjectMaskQuality::Medium.ai_model(Float32),
            AiModel::BiRefNet
        );
        assert_eq!(
            SubjectMaskQuality::High.ai_model(Float32),
            AiModel::BiRefNet
        );
        assert_eq!(
            SubjectMaskQuality::Low.ai_model(Float16),
            AiModel::BiRefNetLowFp16
        );
        assert_eq!(
            SubjectMaskQuality::Medium.ai_model(Float16),
            AiModel::BiRefNetFp16
        );
        assert_eq!(
            SubjectMaskQuality::High.ai_model(Float16),
            AiModel::BiRefNetFp16
        );
        assert!(!SubjectMaskQuality::Low.crop_refinement());
        assert!(!SubjectMaskQuality::Medium.crop_refinement());
        assert!(SubjectMaskQuality::High.crop_refinement());
    }

    /// Each precision of a quality sees the same input and needs less GPU
    /// memory as float16.
    #[test]
    fn float16_subject_models_match_their_float32_input() {
        use SubjectModelPrecision::{Float16, Float32};
        for &quality in SubjectMaskQuality::AVAILABLE {
            let (float32, float16) = (
                quality.ai_model(Float32).spec(),
                quality.ai_model(Float16).spec(),
            );
            assert_eq!(float32.input, float16.input);
            assert!(float16.session.gpu_memory_bytes < float32.session.gpu_memory_bytes);
        }
    }

    #[test]
    fn manual_and_cpu_runtimes_use_float32_subject_models() {
        assert_eq!(
            SubjectModelPrecision::for_current_sessions(true),
            SubjectModelPrecision::Float32
        );
        if !cfg!(any(target_os = "windows", target_os = "linux")) {
            assert_eq!(
                SubjectModelPrecision::for_current_sessions(false),
                SubjectModelPrecision::Float32
            );
        }
    }

    #[test]
    fn every_platform_defaults_to_an_available_subject_quality() {
        assert!(SubjectMaskQuality::AVAILABLE.contains(&SubjectMaskQuality::default()));
        assert_eq!(
            SubjectMaskQuality::AVAILABLE.contains(&SubjectMaskQuality::High),
            !cfg!(target_os = "android")
        );
    }

    #[test]
    fn birefnet_output_keeps_soft_probabilities_when_restoring_size() {
        let mask = restore_birefnet_output(&[-2.0, 0.0, 2.0], 3, 1, 3, 1).unwrap();
        assert_eq!(mask, vec![30, 128, 225]);
    }

    #[test]
    fn birefnet_output_requires_single_batch_and_channel() {
        assert!(
            validate_birefnet_output_shape(&[1, 1, 1024, 1024], 1024 * 1024, 1024, 1024).is_ok()
        );
        assert!(
            validate_birefnet_output_shape(&[1, 2, 1024, 1024], 2 * 1024 * 1024, 1024, 1024)
                .is_err()
        );
        assert!(validate_birefnet_output_shape(&[1, 1, -1, 1024], 0, 1024, 1024).is_err());
        assert!(validate_birefnet_output_shape(&[1, 1, 2, 2], 5, 1024, 1024).is_err());
    }
}
