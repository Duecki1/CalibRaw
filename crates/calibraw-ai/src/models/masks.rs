//! The job behind generated masks: subject, sky and scene depth.

use super::{depth, sky, subject};
use crate::jobs::{spawn_job, AiJobEvent, JobKind};
use crate::pipeline::MaskImage;
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

/// How long a mask model's session stays loaded on this platform: Android
/// unloads every model after its job.
pub(super) fn mask_model_retention(cache_supported: bool) -> ModelRetention {
    #[cfg(target_os = "android")]
    {
        let _ = cache_supported;
        ModelRetention::OneShot
    }
    #[cfg(not(target_os = "android"))]
    {
        if cache_supported {
            ModelRetention::WhileWarm
        } else {
            ModelRetention::OneShot
        }
    }
}

#[cfg(target_os = "linux")]
fn running_from_appimage() -> bool {
    std::env::var_os("APPIMAGE").is_some() || std::env::var_os("APPDIR").is_some()
}

/// Whether SAM sessions may stay loaded between object-mask jobs.
pub(super) fn cache_object_ai_sessions() -> bool {
    #[cfg(target_os = "android")]
    {
        false
    }
    #[cfg(all(not(target_os = "android"), target_os = "linux"))]
    {
        !running_from_appimage()
    }
    #[cfg(all(not(target_os = "android"), not(target_os = "linux")))]
    {
        true
    }
}

#[derive(Debug)]
pub enum AiMaskProgress {
    Inferencing,
}

pub type AiMaskEvent = AiJobEvent<AiMaskResult, AiMaskProgress>;

#[derive(Debug)]
pub struct AiMaskResult {
    pub width: u32,
    pub height: u32,
    pub mask: Vec<u8>,
}

impl AiMaskResult {
    pub fn into_probability_mask(self) -> Option<MaskImage> {
        MaskImage::new(self.width, self.height, self.mask)
    }
}

pub struct AiMaskWorkerRequest {
    pub model: AiMaskModel,
    /// Used by `AiMaskModel::Subject` only.
    pub subject_quality: subject::SubjectMaskQuality,
    /// Used by `AiMaskModel::Subject` only.
    pub subject_precision: subject::SubjectModelPrecision,
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
    let mask = match model {
        AiMaskModel::Depth => depth::depth_mask(&model_path, &image)?,
        AiMaskModel::Sky => sky::sky_mask(&model_path, &image)?,
        AiMaskModel::Subject => {
            subject::subject_mask(&model_path, &image, subject_quality, subject_precision)?
        }
    };
    calibraw_core::diagnostics::record(format!(
        "AI {} mask finished in {:.3}s",
        model.noun(),
        started.elapsed().as_secs_f64()
    ));
    Ok(AiMaskResult {
        width,
        height,
        mask,
    })
}
