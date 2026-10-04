// SPDX-License-Identifier: GPL-3.0-or-later
// The heal solver includes an adaptation of GIMP 3.0.4 app/paint/gimpheal.c.
// Copyright the GIMP contributors.
// Copyright (C) 2026 CalibRaw contributors (Rust adaptation).

use crate::execution_provider::SessionOptions;
use crate::model_artifact::{DownloadOptions, ModelArtifact};
use crate::model_install::ModelInstallSpec;
use crate::model_runtime::{with_model_session, AiModel, ModelRetention};
use crate::pipeline::{
    adaptive_remove_dilation, pipeline_scene_to_canonical_remove_scene,
    pipeline_scene_to_working_rec2020, plan_remove_passes, rasterize_remove_brush,
    remove_model_srgb_to_canonical_scene, remove_model_view_gain, remove_scene_to_model_srgb,
    render_remove_scene_crop, render_remove_scene_crop_resized,
    working_rec2020_to_canonical_remove_scene, DevelopedCropJob, ExposureParams, GeometryTransform,
    GpuProgramPrewarm, LoadedRaw, MaskStack, NativeRect, RemoveBrushStroke, RemoveEditState,
    RemoveMask, RemovePass, RemovePatch, RemoveStroke, ResizedRemoveSceneCrop, RetouchAlignment,
    RetouchStroke, RetouchTool, BIG_LAMA_INPUT_EDGE,
};
use crate::ModelDownloadProgress;

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
    time::Duration,
};

mod retouch;
use retouch::*;

pub const BIG_LAMA_MODEL_FILENAME: &str = "big-lama-places2-fp32-512.onnx";
pub const BIG_LAMA_MODEL_URL: &str =
    "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/91085ce0ec322a4a7cbd20059688690218e52f9a/models/lama/lama_fp32.onnx";
pub const BIG_LAMA_MODEL_SHA256_HEX: &str =
    "1faef5301d78db7dda502fe59966957ec4b79dd64e16f03ed96913c7a4eb68d6";
pub const BIG_LAMA_MODEL_BYTES: u64 = 208_044_816;
pub const BIG_LAMA_MODEL_LICENSE: &str = "Apache-2.0";
pub const BIG_LAMA_MODEL_PROVENANCE: &str =
    "Carve/LaMa-ONNX port of the original PyTorch big-lama inpainting model";

const BIG_LAMA_ARTIFACT: ModelArtifact = ModelArtifact {
    name: "Big-LaMa Places2 ONNX",
    url: Some(BIG_LAMA_MODEL_URL),
    sha256: BIG_LAMA_MODEL_SHA256_HEX,
    bytes: BIG_LAMA_MODEL_BYTES,
};
const BIG_LAMA_DOWNLOAD: DownloadOptions = DownloadOptions {
    connect_timeout: Duration::from_secs(30),
    response_timeout: Duration::from_secs(60),
    body_timeout: Duration::from_secs(30 * 60),
    attempts: 5,
    resume: true,
};
const BIG_LAMA_INSTALL: ModelInstallSpec = ModelInstallSpec {
    artifact: BIG_LAMA_ARTIFACT,
    download: BIG_LAMA_DOWNLOAD,
    progress_label: "Big-LaMa Remove model",
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
    pub runtime_path: Option<PathBuf>,
    pub runtime_sha256: Option<String>,
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

#[derive(Debug)]
pub enum RemoveEvent {
    DownloadProgress(ModelDownloadProgress),
    Processing { completed: usize, total: usize },
    Finished(Result<RemoveStroke, String>),
}

pub fn big_lama_model_is_verified(path: &Path) -> bool {
    BIG_LAMA_INSTALL.is_installed(path)
}

pub fn spawn_remove(request: RemoveRequest) -> mpsc::Receiver<RemoveEvent> {
    let (sender, receiver) = mpsc::channel();
    let worker = sender.clone();
    let spawn = std::thread::Builder::new()
        .name("calibraw-big-lama-remove".to_owned())
        .spawn(move || {
            let result = run_remove(request, &worker).map_err(|error| format!("{error:#}"));
            let _ = worker.send(RemoveEvent::Finished(result));
        });
    if let Err(error) = spawn {
        let _ = sender.send(RemoveEvent::Finished(Err(format!(
            "could not start Remove worker: {error}"
        ))));
    }
    receiver
}

/// Local clone/heal path; never initializes or downloads an ONNX model.
pub fn spawn_retouch(request: RetouchRequest) -> mpsc::Receiver<RemoveEvent> {
    let (sender, receiver) = mpsc::channel();
    let worker = sender.clone();
    let spawn = std::thread::Builder::new()
        .name("calibraw-retouch-brush".to_owned())
        .spawn(move || {
            let result = run_retouch(request).map_err(|error| format!("{error:#}"));
            let _ = worker.send(RemoveEvent::Finished(result));
        });
    if let Err(error) = spawn {
        let _ = sender.send(RemoveEvent::Finished(Err(format!(
            "could not start retouch worker: {error}"
        ))));
    }
    receiver
}

fn ensure_not_cancelled(cancellation: &AtomicBool) -> Result<()> {
    anyhow::ensure!(
        !cancellation.load(Ordering::Acquire),
        "Remove operation cancelled"
    );
    Ok(())
}

fn run_remove(request: RemoveRequest, events: &mpsc::Sender<RemoveEvent>) -> Result<RemoveStroke> {
    ensure_not_cancelled(&request.cancellation)?;
    crate::ai_masks::initialize_runtime(
        request.runtime_path.as_deref(),
        request.runtime_sha256.as_deref(),
    )?;
    BIG_LAMA_INSTALL.ensure_installed(
        &request.model_path,
        request.allow_download,
        |progress| {
            let _ = events.send(RemoveEvent::DownloadProgress(progress));
        },
        || ensure_not_cancelled(&request.cancellation),
    )?;
    ensure_not_cancelled(&request.cancellation)?;

    let mut brush = request.brush;
    if brush.dilation_radius == 0 {
        brush.dilation_radius = adaptive_remove_dilation(&brush.points);
    }
    let mask = rasterize_remove_brush(request.raw.width, request.raw.height, &brush)
        .context("Remove brush produced no native image mask")?;
    let passes = plan_remove_passes(request.raw.width, request.raw.height, &mask);
    anyhow::ensure!(!passes.is_empty(), "Remove mask produced no inference pass");

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
        let surroundings = render_remove_scene_crop(DevelopedCropJob {
            device: request.device.clone(),
            queue: request.queue.clone(),
            raw: Arc::clone(&request.raw),
            geometry: request.geometry,
            exposure: request.exposure,
            masks: request.masks.clone(),
            remove: context.clone(),
            crop: surroundings_bounds,
            program_prewarm: request.program_prewarm.clone(),
        })
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
        let _ = events.send(RemoveEvent::Processing {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{RemoveBrushPoint, RemoveBrushStroke};

    #[test]
    fn retouch_hardness_keeps_a_soft_outer_ring() {
        assert_eq!(retouch_brush_coverage(0.0, 0.0, 10.0, 0.5), 1.0);
        assert_eq!(retouch_brush_coverage(4.0, 0.0, 10.0, 0.5), 1.0);
        let feather = retouch_brush_coverage(7.5, 0.0, 10.0, 0.5);
        assert!(feather > 0.0 && feather < 1.0);
        assert_eq!(retouch_brush_coverage(10.0, 0.0, 10.0, 0.5), 0.0);
    }

    #[test]
    fn gimp_heal_solver_relaxes_difference_inside_mask() {
        let mut difference = vec![0.0f32; 3 * 3 * 3 + 3];
        difference[(4 * 3)..(4 * 3 + 3)].fill(1.0);
        let mut mask = vec![false; 9];
        mask[4] = true;
        gimp_heal_laplace_loop(&mut difference, 3, 3, &mask, &AtomicBool::new(false)).unwrap();
        assert!(difference[12..15].iter().all(|value| value.abs() < 1e-4));
    }

    fn uniform_retouch_patch(tool: RetouchTool, opacity: f32) -> RemovePatch {
        let raw = LoadedRaw::from_scene_linear_rec2020(8, 4, vec![0.2; 8 * 4 * 3]).unwrap();
        let destination_bounds = NativeRect {
            x: 4,
            y: 0,
            width: 4,
            height: 4,
        };
        let source_bounds = NativeRect {
            x: 0,
            y: 0,
            width: 4,
            height: 4,
        };
        build_retouch_patch(
            &raw,
            &ExposureParams::default(),
            destination_bounds,
            &[0.2; 4 * 4 * 3],
            source_bounds,
            &[0.8; 4 * 4 * 3],
            &RemoveBrushStroke {
                points: vec![RemoveBrushPoint {
                    x: 5.5,
                    y: 1.5,
                    radius: 1.25,
                }],
                dilation_radius: 0,
            },
            RetouchStroke {
                tool,
                alignment: RetouchAlignment::None,
                source: [1.5, 1.5],
                destination: [5.5, 1.5],
                hardness: 1.0,
                opacity,
            },
            &AtomicBool::new(false),
        )
        .unwrap()
    }

    #[test]
    fn clone_copies_source_scene_pixels() {
        let patch = uniform_retouch_patch(RetouchTool::Clone, 1.0);
        let maximum = patch
            .rgb_scene16f
            .iter()
            .map(|bits| half::f16::from_bits(*bits).to_f32())
            .fold(0.0f32, f32::max);
        assert!((maximum - 0.8).abs() < 0.002);
    }

    #[test]
    fn heal_preserves_uniform_destination_light_and_color() {
        let patch = uniform_retouch_patch(RetouchTool::Heal, 1.0);
        let maximum = patch
            .rgb_scene16f
            .iter()
            .map(|bits| half::f16::from_bits(*bits).to_f32())
            .fold(0.0f32, f32::max);
        assert!((maximum - 0.2).abs() < 0.002);
    }

    #[test]
    fn retouch_opacity_is_live_instead_of_baked_into_cached_pixels() {
        for tool in [RetouchTool::Clone, RetouchTool::Heal] {
            assert_eq!(
                uniform_retouch_patch(tool, 0.2),
                uniform_retouch_patch(tool, 1.0),
                "{tool:?} cached different pixels at different live opacities"
            );
        }
    }
}
