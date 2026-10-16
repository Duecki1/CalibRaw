//! Object masks from SAM 2.1 Hiera Tiny: the encoder runs once per crop, the
//! decoder once per prompt change.

use super::masks::mask_model_retention;
use super::normalized_rgb_input;
use crate::jobs::{spawn_job, AiJobEvent, JobKind};
use crate::registry::{AiModel, InputShape, ModelInfo};
use crate::runtime::initialize_runtime;
use anyhow::{Context, Result};
use image::{imageops::FilterType, ImageBuffer, Rgba};
use std::{
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, mpsc, Arc},
};

mod candidates;
mod cleanup;
mod prompts;
mod sam;
use candidates::*;
use cleanup::*;
use prompts::*;
use sam::*;

pub const ENCODER_MODEL: ModelInfo = AiModel::SamEncoder.spec().info();
pub const DECODER_MODEL: ModelInfo = AiModel::SamDecoder.spec().info();
/// Both SAM 2.1 files together, as the download consent shows them.
pub const SAM21_MODEL_BYTES_ESTIMATE: u64 = 125_500_000;
const SAM21_MODEL_SIZE: u32 = ENCODER_MODEL.input.edge();
const SAM21_MASK_INPUT_SIZE: u32 = match DECODER_MODEL.input {
    InputShape::Prompted { mask_edge, .. } => mask_edge,
    InputShape::Square { .. } | InputShape::Tiled { .. } => {
        panic!("the SAM 2.1 decoder takes prompts and a previous mask")
    }
};
const SAM21_MAX_PROMPTS: usize = 32;
const MAX_OBJECT_MASK_PIXELS: u64 = 17_000_000;

const OBJECT_JOB: JobKind = JobKind {
    thread: "calibraw-onnx-object",
    worker: "SAM 2.1 worker",
    panicked: "ONNX Runtime terminated object-mask inference",
    cancelled: "background task cancelled",
};

pub fn object_models_are_verified(encoder: &Path, decoder: &Path) -> bool {
    AiModel::SamEncoder.spec().is_installed(encoder)
        && AiModel::SamDecoder.spec().is_installed(decoder)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectCropRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug)]
pub struct SamTensorData {
    pub shape: Vec<usize>,
    pub values: std::sync::Arc<[f32]>,
}

#[derive(Clone, Debug)]
pub struct ObjectInferenceCache {
    pub source_width: u32,
    pub source_height: u32,
    pub crop: ObjectCropRect,
    pub high_res_feats_0: SamTensorData,
    pub high_res_feats_1: SamTensorData,
    pub image_embedding: SamTensorData,
    pub low_res_logits: std::sync::Arc<[f32]>,
    pub prompt_strokes: Vec<crate::pipeline::ObjectStroke>,
    pub prompt_brush_size: f32,
}

#[derive(Clone, Debug)]
pub struct ObjectMaskRequest {
    pub source_width: u32,
    pub source_height: u32,
    pub source_rgba: Vec<u8>,
    pub strokes: Vec<crate::pipeline::ObjectStroke>,
    pub brush_size: f32,
    pub edge_refine: f32,
    pub cache: Option<ObjectInferenceCache>,
}

#[derive(Debug)]
pub struct ObjectMaskResult {
    pub width: u32,
    pub height: u32,
    pub mask: Vec<u8>,
    pub cache: ObjectInferenceCache,
}

#[derive(Debug)]
pub enum ObjectMaskProgress {
    /// `decoder_only` when the request's cache spares the encoder.
    Inferencing { decoder_only: bool },
}

pub type ObjectMaskEvent = AiJobEvent<ObjectMaskResult, ObjectMaskProgress>;

pub struct ObjectMaskWorkerRequest {
    pub encoder_path: PathBuf,
    pub decoder_path: PathBuf,
    pub allow_download: bool,
    pub inference: ObjectMaskRequest,
    pub cancellation: Arc<AtomicBool>,
}

pub fn spawn_object_mask(worker: ObjectMaskWorkerRequest) -> mpsc::Receiver<ObjectMaskEvent> {
    let ObjectMaskWorkerRequest {
        encoder_path,
        decoder_path,
        allow_download,
        inference,
        cancellation,
    } = worker;
    spawn_job(&OBJECT_JOB, cancellation, move |job| {
        job.install(AiModel::SamEncoder.spec(), &encoder_path, allow_download)?;
        job.install(AiModel::SamDecoder.spec(), &decoder_path, allow_download)?;
        job.ensure_not_cancelled()?;
        let decoder_only = inference.cache.is_some();
        job.progress(ObjectMaskProgress::Inferencing { decoder_only });
        infer_object_mask(&encoder_path, &decoder_path, inference)
    })
}

fn infer_object_mask(
    encoder_path: &Path,
    decoder_path: &Path,
    request: ObjectMaskRequest,
) -> Result<ObjectMaskResult> {
    anyhow::ensure!(
        request.source_width > 0 && request.source_height > 0,
        "object-mask source is empty"
    );
    let pixels = u64::from(request.source_width)
        .checked_mul(u64::from(request.source_height))
        .context("object-mask input dimensions overflow")?;
    anyhow::ensure!(
        pixels <= MAX_OBJECT_MASK_PIXELS,
        "object-mask input {}x{} exceeds the {MAX_OBJECT_MASK_PIXELS}-pixel limit",
        request.source_width,
        request.source_height
    );
    let expected = pixels
        .checked_mul(4)
        .and_then(|bytes| usize::try_from(bytes).ok())
        .context("object-mask input byte count overflow")?;
    anyhow::ensure!(
        request.source_rgba.len() == expected,
        "object-mask RGBA buffer has {}, expected {expected}",
        request.source_rgba.len()
    );
    anyhow::ensure!(
        request
            .strokes
            .iter()
            .any(|stroke| stroke.positive && !stroke.points.is_empty()),
        "paint inside an object before running selection"
    );
    initialize_runtime()?;

    let source = ImageBuffer::<Rgba<u8>, _>::from_raw(
        request.source_width,
        request.source_height,
        request.source_rgba,
    )
    .context("invalid canonical image for object selection")?;

    let prompt_set = sampled_object_prompts(
        &request.strokes,
        request.brush_size,
        request.source_width,
        request.source_height,
        SAM21_MAX_PROMPTS,
    );
    let prompts = &prompt_set.prompts;
    let supplied_cache = request.cache;
    let mut last_result = None;

    for expansion in 0..3 {
        let crop = object_crop_for_prompts(
            request.source_width,
            request.source_height,
            prompt_set.focus,
            expansion,
        );
        let cached = supplied_cache
            .as_ref()
            .filter(|cache| {
                cache.source_width == request.source_width
                    && cache.source_height == request.source_height
                    && cache.crop == crop
            })
            .cloned();
        let crop_image =
            image::imageops::crop_imm(&source, crop.x, crop.y, crop.width, crop.height).to_image();
        let resized = image::imageops::resize(
            &crop_image,
            SAM21_MODEL_SIZE,
            SAM21_MODEL_SIZE,
            FilterType::Lanczos3,
        );

        let (features, previous_logits) = if let Some(cache) = cached {
            let can_reuse_logits = strokes_extend(&cache.prompt_strokes, &request.strokes)
                && (cache.prompt_brush_size - request.brush_size).abs() <= f32::EPSILON;
            (cache, can_reuse_logits)
        } else {
            (
                encode_sam_image(
                    encoder_path,
                    &resized,
                    request.source_width,
                    request.source_height,
                    crop,
                )?,
                false,
            )
        };
        let decoded = decode_sam_mask(decoder_path, &features, &prompt_set, previous_logits)?;
        let touches_border =
            mask_touches_crop_border(&decoded.probabilities, decoded.width, decoded.height);
        last_result = Some((crop, resized, features, decoded));
        if !touches_border
            || expansion == 2
            || crop_is_full(crop, request.source_width, request.source_height)
        {
            break;
        }
    }

    let (crop, resized_guidance, mut cache, decoded) =
        last_result.context("SAM 2.1 produced no object-mask candidate")?;
    let selected = keep_prompt_connected_component(
        decoded.probabilities,
        decoded.width,
        decoded.height,
        prompts,
        request.source_width,
        request.source_height,
        crop,
    );
    let refine_guidance = if decoded.width == resized_guidance.width()
        && decoded.height == resized_guidance.height()
    {
        resized_guidance
    } else {
        image::imageops::resize(
            &resized_guidance,
            decoded.width,
            decoded.height,
            FilterType::Lanczos3,
        )
    };
    let refined = edge_aware_refine(
        selected,
        decoded.width,
        decoded.height,
        refine_guidance.as_raw(),
        request.edge_refine,
    );
    let crop_mask = resize_probability_u8(
        &refined,
        decoded.width,
        decoded.height,
        crop.width,
        crop.height,
    );
    let mut full_mask = vec![0u8; request.source_width as usize * request.source_height as usize];
    for y in 0..crop.height as usize {
        let source_start = y * crop.width as usize;
        let target_start = (crop.y as usize + y) * request.source_width as usize + crop.x as usize;
        full_mask[target_start..target_start + crop.width as usize]
            .copy_from_slice(&crop_mask[source_start..source_start + crop.width as usize]);
    }
    cache.low_res_logits = resize_f32(
        &decoded.selected_logits,
        decoded.width,
        decoded.height,
        SAM21_MASK_INPUT_SIZE,
        SAM21_MASK_INPUT_SIZE,
    )
    .into();
    cache.prompt_strokes = request.strokes.clone();
    cache.prompt_brush_size = request.brush_size;

    Ok(ObjectMaskResult {
        width: request.source_width,
        height: request.source_height,
        mask: full_mask,
        cache,
    })
}

#[cfg(test)]
mod object_mask_tests {
    use super::*;
    use crate::pipeline::ObjectStroke;

    fn stroke(points: &[[f32; 2]], positive: bool) -> ObjectStroke {
        ObjectStroke {
            points: points.to_vec(),
            positive,
            brush_size: 0.0,
        }
    }

    #[test]
    fn prompt_sampling_adds_box_and_background_guards_within_limit() {
        let strokes = vec![
            stroke(&[[0.1, 0.1], [0.2, 0.2], [0.3, 0.3], [0.4, 0.4]], true),
            stroke(&[[0.8, 0.8], [0.7, 0.7], [0.6, 0.6]], false),
        ];
        let set = sampled_object_prompts(&strokes, 0.04, 1000, 600, SAM21_MAX_PROMPTS);
        assert!(set.prompts.len() <= SAM21_MAX_PROMPTS);
        assert!(set
            .prompts
            .iter()
            .any(|prompt| prompt.kind == ObjectPromptKind::Foreground));
        assert!(set
            .prompts
            .iter()
            .any(|prompt| prompt.kind == ObjectPromptKind::Background));
        assert!(set
            .prompts
            .iter()
            .any(|prompt| prompt.kind == ObjectPromptKind::BoxTopLeft));
        assert!(set
            .prompts
            .iter()
            .any(|prompt| prompt.kind == ObjectPromptKind::BoxBottomRight));
    }

    #[test]
    fn focus_and_guard_prompts_are_inside_the_adaptive_crop() {
        let strokes = vec![stroke(&[[0.50, 0.50], [0.62, 0.50]], true)];
        let set = sampled_object_prompts(&strokes, 0.035, 1000, 600, SAM21_MAX_PROMPTS);
        let crop = object_crop_for_prompts(1000, 600, set.focus, 0);
        for prompt in &set.prompts {
            let x = (prompt.point[0] * 1000.0) as u32;
            let y = (prompt.point[1] * 600.0) as u32;
            assert!(x >= crop.x && x <= crop.x + crop.width);
            assert!(y >= crop.y && y <= crop.y + crop.height);
        }
        assert_eq!(
            object_crop_for_prompts(1000, 600, set.focus, 2),
            ObjectCropRect {
                x: 0,
                y: 0,
                width: 1000,
                height: 600,
            }
        );
    }

    #[test]
    fn previous_logits_only_apply_to_prompt_extensions() {
        let original = vec![stroke(&[[0.4, 0.4], [0.5, 0.5]], true)];
        let mut extended = original.clone();
        extended.push(stroke(&[[0.7, 0.7]], false));
        assert!(strokes_extend(&original, &original));
        assert!(strokes_extend(&original, &extended));
        assert!(!strokes_extend(&extended, &original));
        assert!(!strokes_extend(&original, &[stroke(&[[0.1, 0.1]], true)]));
    }

    #[test]
    fn connected_component_cleanup_keeps_soft_edge_near_prompted_object() {
        let width = 50;
        let height = 5;
        let mut probabilities = vec![0.0; width * height];
        for y in 1..4 {
            for x in 1..4 {
                probabilities[y * width + x] = 0.9;
            }
            for x in 40..43 {
                probabilities[y * width + x] = 0.95;
            }
        }
        probabilities[2 * width + 4] = 0.35;
        let prompts = [ObjectPrompt {
            point: [2.0 / width as f32, 2.0 / height as f32],
            kind: ObjectPromptKind::Foreground,
        }];
        let cleaned = keep_prompt_connected_component(
            probabilities,
            width as u32,
            height as u32,
            &prompts,
            width as u32,
            height as u32,
            ObjectCropRect {
                x: 0,
                y: 0,
                width: width as u32,
                height: height as u32,
            },
        );
        assert!(cleaned[2 * width + 2] > 0.8);
        assert!(cleaned[2 * width + 4] > 0.3);
        assert_eq!(cleaned[2 * width + 41], 0.0);
    }

    #[test]
    fn probability_resize_preserves_endpoints() {
        let resized = resize_f32(&[0.0, 1.0], 2, 1, 5, 1);
        assert_eq!(resized.len(), 5);
        assert!(resized[0] <= 0.001);
        assert!(resized[4] >= 0.999);
        assert!(resized.windows(2).all(|pair| pair[0] <= pair[1]));
    }
}
