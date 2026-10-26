// SPDX-License-Identifier: GPL-3.0-or-later
// The heal solver includes an adaptation of GIMP 3.0.4 app/paint/gimpheal.c.
// Copyright the GIMP contributors.
// Copyright (C) 2026 CalibRaw contributors (Rust adaptation).

use crate::jobs::{spawn_job, AiJobEvent, JobContext, JobKind};
use crate::pipeline::{
    adaptive_remove_dilation, pipeline_scene_to_canonical_remove_scene,
    pipeline_scene_to_working_rec2020, plan_remove_passes, rasterize_remove_brush,
    remove_model_srgb_to_canonical_scene, remove_model_view_gain, remove_scene_to_model_srgb,
    render_remove_scene_crop, render_remove_scene_crop_resized,
    working_rec2020_to_canonical_remove_scene, DevelopedCropJob, ExposureParams, GeometryTransform,
    GpuProgramPrewarm, GpuWait, LoadedRaw, MaskStack, NativeRect, RemoveBrushStroke,
    RemoveEditState, RemoveMask, RemovePass, RemovePatch, RemoveStroke, ResizedRemoveSceneCrop,
    RetouchAlignment, RetouchStroke, RetouchTool, BIG_LAMA_INPUT_EDGE,
};
use crate::registry::{AiModel, ModelInfo};
use crate::runtime::initialize_runtime;
use crate::slot::with_model_session;

mod lama;
use anyhow::{Context, Result};
use calibraw_core::color_math::{srgb_decode_signed, srgb_encode_signed};
use image::{imageops::FilterType, ImageBuffer, Rgb32FImage};
use ort::value::Tensor;
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
};

mod retouch;
use retouch::*;

pub const MODEL: ModelInfo = AiModel::BigLama.spec().info();
pub const MODEL_LICENSE: &str = "Apache-2.0";
pub const MODEL_PROVENANCE: &str =
    "Carve/LaMa-ONNX port of the original PyTorch big-lama inpainting model";

const REMOVE_JOB: JobKind = JobKind {
    thread: "calibraw-big-lama-remove",
    worker: "Remove worker",
    panicked: "Remove stopped unexpectedly",
    cancelled: "Remove operation cancelled",
};
const RETOUCH_JOB: JobKind = JobKind {
    thread: "calibraw-retouch-brush",
    worker: "retouch worker",
    panicked: "Retouch stopped unexpectedly",
    cancelled: "Remove operation cancelled",
};

#[derive(Clone)]
pub struct RemoveRequest {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub raw: Arc<LoadedRaw>,
    pub geometry: GeometryTransform,
    pub exposure: ExposureParams,
    pub masks: MaskStack,
    pub existing: RemoveEditState,
    pub brush: RemoveBrushStroke,
    pub opacity: f32,
    pub model_path: PathBuf,
    pub allow_download: bool,
    pub program_prewarm: Option<Arc<GpuProgramPrewarm>>,
    pub cancellation: Arc<AtomicBool>,
}

#[derive(Clone)]
pub struct RetouchRequest {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub raw: Arc<LoadedRaw>,
    pub geometry: GeometryTransform,
    pub exposure: ExposureParams,
    pub masks: MaskStack,
    pub existing: RemoveEditState,
    pub brush: RemoveBrushStroke,
    pub retouch: RetouchStroke,
    pub program_prewarm: Option<Arc<GpuProgramPrewarm>>,
    pub cancellation: Arc<AtomicBool>,
}

/// Remove fills a stroke in one or more passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoveProgress {
    pub completed: usize,
    pub total: usize,
}

/// Events of Remove and retouch jobs; retouch reports no progress.
pub type RemoveEvent = AiJobEvent<RemoveStroke, RemoveProgress>;

pub fn big_lama_model_is_verified(path: &Path) -> bool {
    AiModel::BigLama.spec().is_installed(path)
}

pub fn spawn_remove(request: RemoveRequest) -> mpsc::Receiver<RemoveEvent> {
    let cancellation = Arc::clone(&request.cancellation);
    spawn_job(&REMOVE_JOB, cancellation, move |job| {
        run_remove(request, job)
    })
}

/// Local clone/heal path; never initializes or downloads an ONNX model.
pub fn spawn_retouch(request: RetouchRequest) -> mpsc::Receiver<RemoveEvent> {
    let cancellation = Arc::clone(&request.cancellation);
    spawn_job(&RETOUCH_JOB, cancellation, move |_| run_retouch(request))
}

fn ensure_not_cancelled(cancellation: &AtomicBool) -> Result<()> {
    anyhow::ensure!(
        !cancellation.load(Ordering::Acquire),
        "Remove operation cancelled"
    );
    Ok(())
}

fn run_remove(
    request: RemoveRequest,
    job: &JobContext<RemoveStroke, RemoveProgress>,
) -> Result<RemoveStroke> {
    job.ensure_not_cancelled()?;
    initialize_runtime()?;
    job.install(
        AiModel::BigLama.spec(),
        &request.model_path,
        request.allow_download,
    )?;
    job.ensure_not_cancelled()?;

    let mut brush = request.brush;
    if brush.dilation_radius == 0 {
        brush.dilation_radius = adaptive_remove_dilation(&brush.points);
    }
    let mask = rasterize_remove_brush(request.raw.width, request.raw.height, &brush)
        .context("Remove brush produced no native image mask")?;
    let passes = plan_remove_passes(request.raw.width, request.raw.height, &mask);
    anyhow::ensure!(!passes.is_empty(), "Remove mask produced no inference pass");
    // Replaces any finished download progress before the first pass runs.
    job.progress(RemoveProgress {
        completed: 0,
        total: passes.len(),
    });

    // Passes run in order: each one sees earlier fills as real context, while
    // the stroke pixels still to fill stay masked.
    let mut unfilled = mask;
    let mut patches = Vec::with_capacity(passes.len());
    for (index, pass) in passes.iter().enumerate() {
        ensure_not_cancelled(&request.cancellation)?;
        let mut context = request.existing.clone();
        if !patches.is_empty() {
            context.strokes.push(RemoveStroke {
                brush: RemoveBrushStroke::default(),
                patches: patches.clone(),
                retouch: None,
                opacity: 1.0,
            });
        }
        let crop = pass.crop;
        let surroundings_bounds =
            lama::surroundings_region(pass, request.raw.width, request.raw.height);
        let surroundings = render_remove_scene_crop(
            DevelopedCropJob {
                device: request.device.clone(),
                queue: request.queue.clone(),
                raw: Arc::clone(&request.raw),
                geometry: request.geometry,
                exposure: request.exposure,
                masks: request.masks.clone(),
                remove: context.clone(),
                crop: surroundings_bounds,
                program_prewarm: request.program_prewarm.clone(),
            },
            GpuWait::Limited,
        )
        .context("render native surroundings for Remove grain")?;
        let scene = render_remove_scene_crop_resized(
            DevelopedCropJob {
                device: request.device.clone(),
                queue: request.queue.clone(),
                raw: Arc::clone(&request.raw),
                geometry: request.geometry,
                exposure: request.exposure,
                masks: request.masks.clone(),
                remove: context,
                crop,
                program_prewarm: request.program_prewarm.clone(),
            },
            BIG_LAMA_INPUT_EDGE,
        )
        .with_context(|| {
            format!(
                "render Big-LaMa context {}x{} at {},{}",
                crop.width, crop.height, crop.x, crop.y,
            )
        })?;
        ensure_not_cancelled(&request.cancellation)?;
        patches.push(lama::infer_pass(
            &request.model_path,
            pass,
            &unfilled,
            &request.raw,
            &request.exposure,
            &scene,
            &lama::NativeSurroundings {
                bounds: surroundings_bounds,
                scene: surroundings,
            },
        )?);
        unfilled.subtract(&pass.target);
        job.progress(RemoveProgress {
            completed: index + 1,
            total: passes.len(),
        });
    }

    Ok(RemoveStroke {
        brush,
        patches,
        retouch: None,
        opacity: request.opacity,
    })
}
