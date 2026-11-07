//! The job behind generated masks: subject, sky and scene depth.

use super::scene_geometry::SceneGeometryModel;
use super::{depth, sky, subject};
use crate::config::CAPABILITIES;
use crate::jobs::{spawn_job, AiJobEvent, JobKind};
use crate::pipeline::{MaskImage, SceneDepth};
use crate::registry::AiModel;
use crate::runtime::initialize_runtime;
use crate::slot::ModelRetention;
use anyhow::{Context, Result};
use image::{ImageBuffer, Rgba};
use std::{
    path::PathBuf,
    sync::{atomic::AtomicBool, mpsc, Arc},
};

const MASK_JOB: JobKind = JobKind {
    thread: "calibraw-onnx-mask",
    worker: "AI mask worker",
    panicked: "ONNX Runtime terminated inference",
    cancelled: "background task cancelled",
};

/// How long a mask model's session stays loaded: models unload after each
/// job unless `cache_supported` and the device keeps mask sessions warm.
pub(super) fn mask_model_retention(cache_supported: bool) -> ModelRetention {
    if cache_supported && CAPABILITIES.keep_mask_sessions_warm {
        ModelRetention::WhileWarm
    } else {
        ModelRetention::OneShot
    }
}

#[derive(Debug)]
pub enum AiMaskProgress {
    Inferencing,
}

pub type AiMaskEvent = AiJobEvent<AiMaskResult, AiMaskProgress>;

#[derive(Debug)]
pub enum AiMaskResult {
    /// `AiMaskModel::Subject` and `AiMaskModel::Sky`: the probability of each
    /// pixel of the input, 0 to 255.
    Probability(MaskImage),
    /// `AiMaskModel::Depth`.
    SceneDepth(SceneDepth),
}

pub struct AiMaskWorkerRequest {
    pub model: AiMaskModel,
    /// Used by `AiMaskModel::Subject` only.
    pub subject_quality: subject::SubjectMaskQuality,
    /// Used by `AiMaskModel::Subject` only.
    pub subject_precision: subject::SubjectModelPrecision,
    /// Required by `AiMaskModel::Depth`, which runs it beside the depth model
    /// at `model_path`.
    pub scene_geometry: Option<SceneGeometryModel>,
    /// Used by `AiMaskModel::Depth`: the photo's 35 mm-equivalent focal
    /// length, when known.
    pub focal_length_35mm: Option<f32>,
    pub model_path: PathBuf,
    pub allow_download: bool,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiMaskModel {
    Subject,
    Sky,
    Depth,
}

impl AiMaskModel {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Subject => "Subject",
            Self::Sky => "Sky",
            Self::Depth => "Depth",
        }
    }

    pub const fn noun(self) -> &'static str {
        match self {
            Self::Subject => "subject",
            Self::Sky => "sky",
            Self::Depth => "depth",
        }
    }
}

pub fn spawn_ai_mask(
    request: AiMaskWorkerRequest,
    cancellation: Arc<AtomicBool>,
) -> mpsc::Receiver<AiMaskEvent> {
    spawn_job(&MASK_JOB, cancellation, move |job| {
        job.ensure_not_cancelled()?;
        let model = match request.model {
            AiMaskModel::Subject => request.subject_quality.ai_model(request.subject_precision),
            AiMaskModel::Sky => AiModel::SkySeg,
            AiMaskModel::Depth => AiModel::Depth,
        };
        job.install(model.spec(), &request.model_path, request.allow_download)?;
        if request.model == AiMaskModel::Depth {
            let geometry = request
                .scene_geometry
                .as_ref()
                .context("scene depth needs a normal model")?;
            job.ensure_not_cancelled()?;
            job.install(
                geometry.quality.ai_model().spec(),
                &geometry.path,
                request.allow_download,
            )?;
        }
        job.ensure_not_cancelled()?;
        job.progress(AiMaskProgress::Inferencing);
        infer_ai_mask(request)
    })
}

fn infer_ai_mask(request: AiMaskWorkerRequest) -> Result<AiMaskResult> {
    let AiMaskWorkerRequest {
        model,
        subject_quality,
        subject_precision,
        scene_geometry,
        focal_length_35mm,
        model_path,
        allow_download: _,
        width,
        height,
        rgba,
    } = request;
    const MAX_AI_MASK_PIXELS: u64 = 17_000_000;
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .context("AI-mask input dimensions overflow")?;
    anyhow::ensure!(
        pixels > 0 && pixels <= MAX_AI_MASK_PIXELS,
        "AI-mask input {width}x{height} exceeds the {MAX_AI_MASK_PIXELS}-pixel limit"
    );
    let expected_bytes = pixels
        .checked_mul(4)
        .and_then(|value| usize::try_from(value).ok())
        .context("AI-mask input byte count overflow")?;
    anyhow::ensure!(
        rgba.len() == expected_bytes,
        "AI-mask RGBA buffer has {} bytes, expected {expected_bytes}",
        rgba.len()
    );
    calibraw_core::diagnostics::record(format!("AI {} mask: preparing ONNX Runtime", model.noun()));
    initialize_runtime()?;
    let image = ImageBuffer::<Rgba<u8>, _>::from_raw(width, height, rgba)
        .context("invalid preview image for AI mask")?;
    let started = std::time::Instant::now();
    let probability = |mask| MaskImage::new(width, height, mask).context("invalid mask image");
    let result = match model {
        AiMaskModel::Depth => {
            let geometry = scene_geometry.context("scene depth needs a normal model")?;
            AiMaskResult::SceneDepth(depth::scene_depth(
                &model_path,
                &geometry,
                &image,
                focal_length_35mm,
            )?)
        }
        AiMaskModel::Sky => {
            AiMaskResult::Probability(probability(sky::sky_mask(&model_path, &image)?)?)
        }
        AiMaskModel::Subject => AiMaskResult::Probability(probability(subject::subject_mask(
            &model_path,
            &image,
            subject_quality,
            subject_precision,
        )?)?),
    };
    calibraw_core::diagnostics::record(format!(
        "AI {} mask finished in {:.3}s",
        model.noun(),
        started.elapsed().as_secs_f64()
    ));
    Ok(result)
}
