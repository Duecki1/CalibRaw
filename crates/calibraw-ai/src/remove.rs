// SPDX-License-Identifier: GPL-3.0-or-later
// The heal solver includes an adaptation of GIMP 3.0.4 app/paint/gimpheal.c.
// Copyright the GIMP contributors.
// Copyright (C) 2026 CalibRaw contributors (Rust adaptation).

use crate::execution_provider::SessionOptions;
use crate::model_artifact::{ArtifactSize, DownloadOptions, ModelArtifact};
use crate::model_install::ModelInstallSpec;
use crate::model_runtime::{with_model_session, AiModel, AiRuntimeContext, ModelRetention};
use crate::pipeline::{
    adaptive_remove_dilation, pipeline_scene_to_canonical_remove_scene,
    pipeline_scene_to_working_rec2020, plan_remove_context_crop, rasterize_remove_brush,
    remove_model_srgb_to_canonical_scene, remove_model_view_gain, remove_scene_to_model_srgb,
    render_remove_scene_crop, render_remove_scene_crop_resized,
    working_rec2020_to_canonical_remove_scene, DevelopedCropJob, ExposureParams, GeometryTransform,
    GpuProgramPrewarm, LoadedRaw, MaskStack, NativeRect, RemoveBrushStroke, RemoveEditState,
    RemoveMask, RemovePatch, RemoveStroke, ResizedRemoveSceneCrop, RetouchAlignment, RetouchStroke,
    RetouchTool, BIG_LAMA_INPUT_EDGE,
};
use crate::ModelDownloadProgress;
use anyhow::{Context, Result};
use image::{imageops::FilterType, GrayImage, ImageBuffer, Luma, Rgb, Rgb32FImage};
use ort::value::Tensor;
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::Duration,
};

mod quality;
use quality::{feather_mask, fit_dimensions, mask_distance, reflect, resize_mask, sample_cubic};

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
    size: ArtifactSize::Exact(BIG_LAMA_MODEL_BYTES),
    progress_total: BIG_LAMA_MODEL_BYTES,
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
    let crop = plan_remove_context_crop(request.raw.width, request.raw.height, &mask)
        .context("Remove mask produced no context crop")?;
    ensure_not_cancelled(&request.cancellation)?;
    let scene = render_remove_scene_crop_resized(
        DevelopedCropJob {
            device: request.device.clone(),
            queue: request.queue.clone(),
            raw: Arc::clone(&request.raw),
            geometry: request.geometry,
            exposure: request.exposure,
            masks: request.masks.clone(),
            remove: request.existing.clone(),
            crop,
            program_prewarm: request.program_prewarm.clone(),
        },
        if cfg!(target_os = "android") {
            1024
        } else {
            1536
        },
        &request.cancellation,
    )
    .with_context(|| {
        format!(
            "render bounded Big-LaMa scene for native context {}x{} at {},{}",
            crop.width, crop.height, crop.x, crop.y,
        )
    })?;
    ensure_not_cancelled(&request.cancellation)?;

    let patch = infer_crop(
        &request.model_path,
        crop,
        &request.raw,
        &request.exposure,
        &scene,
        &mask,
        &request.cancellation,
    )?;
    let _ = events.send(RemoveEvent::Processing {
        completed: 1,
        total: 1,
    });

    Ok(RemoveStroke {
        brush,
        patches: vec![patch],
        retouch: None,
        opacity: request.opacity,
    })
}

fn run_retouch(request: RetouchRequest) -> Result<RemoveStroke> {
    ensure_not_cancelled(&request.cancellation)?;
    anyhow::ensure!(!request.brush.points.is_empty(), "Retouch brush is empty");
    anyhow::ensure!(
        request.retouch.source.iter().all(|value| value.is_finite())
            && request
                .retouch
                .destination
                .iter()
                .all(|value| value.is_finite()),
        "Retouch source coordinates are invalid"
    );

    let render = |crop: NativeRect, remove: RemoveEditState| {
        render_remove_scene_crop(DevelopedCropJob {
            device: request.device.clone(),
            queue: request.queue.clone(),
            raw: Arc::clone(&request.raw),
            geometry: request.geometry,
            exposure: request.exposure,
            masks: request.masks.clone(),
            remove,
            crop,
            program_prewarm: request.program_prewarm.clone(),
        })
    };
    let chunks = split_retouch_brush(&request.brush);
    anyhow::ensure!(
        chunks.len() <= crate::pipeline::REMOVE_MAX_PATCHES_PER_STROKE,
        "Retouch stroke is too long"
    );
    let source_snapshot = request.existing.clone();
    let mut destination_working = request.existing.clone();
    let mut patches = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        ensure_not_cancelled(&request.cancellation)?;
        let destination_bounds =
            retouch_stroke_bounds(request.raw.width, request.raw.height, &chunk)?;
        let source_bounds = retouch_source_bounds(
            request.raw.width,
            request.raw.height,
            destination_bounds,
            &chunk,
            request.retouch,
        )?;
        let destination_scene = render(destination_bounds, destination_working.clone())
            .context("render retouch destination")?;
        ensure_not_cancelled(&request.cancellation)?;
        let source_scene =
            render(source_bounds, source_snapshot.clone()).context("render retouch source")?;
        ensure_not_cancelled(&request.cancellation)?;
        let patch = build_retouch_patch(
            &request.raw,
            &request.exposure,
            destination_bounds,
            &destination_scene,
            source_bounds,
            &source_scene,
            &chunk,
            request.retouch,
            &request.cancellation,
        )?;
        destination_working.strokes.push(RemoveStroke {
            brush: chunk,
            patches: vec![patch.clone()],
            retouch: Some(request.retouch),
            opacity: request.retouch.opacity,
        });
        patches.push(patch);
    }
    Ok(RemoveStroke {
        brush: request.brush,
        patches,
        retouch: Some(request.retouch),
        opacity: request.retouch.opacity,
    })
}

fn split_retouch_brush(brush: &RemoveBrushStroke) -> Vec<RemoveBrushStroke> {
    const MAX_CHUNK_EDGE: f32 = 2_048.0;
    const MAX_CHUNK_POINTS: usize = 512;
    let mut chunks = Vec::new();
    let mut points = Vec::new();
    let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for point in &brush.points {
        let radius = point.radius.max(0.5) + 2.0;
        let candidate = [
            bounds[0].min(point.x - radius),
            bounds[1].min(point.y - radius),
            bounds[2].max(point.x + radius),
            bounds[3].max(point.y + radius),
        ];
        let too_large = !points.is_empty()
            && (candidate[2] - candidate[0] > MAX_CHUNK_EDGE
                || candidate[3] - candidate[1] > MAX_CHUNK_EDGE
                || points.len() >= MAX_CHUNK_POINTS);
        if too_large {
            chunks.push(RemoveBrushStroke {
                points: std::mem::take(&mut points),
                dilation_radius: 0,
            });
            bounds = [
                point.x - radius,
                point.y - radius,
                point.x + radius,
                point.y + radius,
            ];
        } else {
            bounds = candidate;
        }
        points.push(*point);
    }
    if !points.is_empty() {
        chunks.push(RemoveBrushStroke {
            points,
            dilation_radius: 0,
        });
    }
    chunks
}

fn retouch_stroke_bounds(
    image_width: u32,
    image_height: u32,
    brush: &RemoveBrushStroke,
) -> Result<NativeRect> {
    let mut left = image_width as f32;
    let mut top = image_height as f32;
    let mut right = 0.0f32;
    let mut bottom = 0.0f32;
    for point in &brush.points {
        if !point.x.is_finite() || !point.y.is_finite() || !point.radius.is_finite() {
            continue;
        }
        let radius = point.radius.max(0.5) + 2.0;
        left = left.min(point.x - radius);
        top = top.min(point.y - radius);
        right = right.max(point.x + radius);
        bottom = bottom.max(point.y + radius);
    }
    clipped_native_rect(image_width, image_height, left, top, right, bottom)
        .context("Retouch stroke lies outside the image")
}

fn retouch_source_bounds(
    image_width: u32,
    image_height: u32,
    destination: NativeRect,
    brush: &RemoveBrushStroke,
    retouch: RetouchStroke,
) -> Result<NativeRect> {
    if retouch.alignment == RetouchAlignment::Fixed {
        let radius = brush
            .points
            .iter()
            .map(|point| point.radius.max(0.5))
            .fold(0.5f32, f32::max)
            + 3.0;
        return clipped_native_rect(
            image_width,
            image_height,
            retouch.source[0] - radius,
            retouch.source[1] - radius,
            retouch.source[0] + radius,
            retouch.source[1] + radius,
        )
        .context("Retouch source lies outside the image");
    }
    let offset = retouch_source_offset(retouch);
    clipped_native_rect(
        image_width,
        image_height,
        destination.x as f32 + offset[0] - 2.0,
        destination.y as f32 + offset[1] - 2.0,
        destination.right() as f32 + offset[0] + 2.0,
        destination.bottom() as f32 + offset[1] + 2.0,
    )
    .context("Retouch source lies outside the image")
}

fn clipped_native_rect(
    image_width: u32,
    image_height: u32,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
) -> Option<NativeRect> {
    let left = left.floor().clamp(0.0, image_width as f32) as u32;
    let top = top.floor().clamp(0.0, image_height as f32) as u32;
    let right = right.ceil().clamp(0.0, image_width as f32) as u32;
    let bottom = bottom.ceil().clamp(0.0, image_height as f32) as u32;
    (right > left && bottom > top).then_some(NativeRect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

fn retouch_source_offset(retouch: RetouchStroke) -> [f32; 2] {
    if retouch.alignment == RetouchAlignment::Registered {
        [0.0, 0.0]
    } else {
        [
            retouch.source[0] - retouch.destination[0],
            retouch.source[1] - retouch.destination[1],
        ]
    }
}

#[allow(clippy::too_many_arguments)]
fn build_retouch_patch(
    raw: &LoadedRaw,
    exposure: &ExposureParams,
    destination_bounds: NativeRect,
    destination_scene: &[f32],
    source_bounds: NativeRect,
    source_scene: &[f32],
    brush: &RemoveBrushStroke,
    retouch: RetouchStroke,
    cancellation: &AtomicBool,
) -> Result<RemovePatch> {
    let pixels = destination_bounds.width as usize * destination_bounds.height as usize;
    anyhow::ensure!(
        destination_scene.len() == pixels * 3,
        "Retouch destination has an invalid RGB length"
    );
    anyhow::ensure!(
        source_scene.len() == source_bounds.width as usize * source_bounds.height as usize * 3,
        "Retouch source has an invalid RGB length"
    );

    let width = destination_bounds.width as usize;
    let height = destination_bounds.height as usize;
    let hardness = retouch.hardness.clamp(0.0, 1.0);
    let offset = retouch_source_offset(retouch);
    let mut stroke_coverage = vec![0.0f32; pixels];
    let output = match retouch.tool {
        RetouchTool::Clone => {
            let mut output_scene = destination_scene.to_vec();
            for (dab_index, point) in brush.points.iter().enumerate() {
                if dab_index % 16 == 0 {
                    ensure_not_cancelled(cancellation)?;
                }
                let Some([left, top, right, bottom]) =
                    retouch_dab_bounds(destination_bounds, *point)
                else {
                    continue;
                };
                for y in top..bottom {
                    let destination_y = destination_bounds.y as f32 + y as f32 + 0.5;
                    for x in left..right {
                        let destination_x = destination_bounds.x as f32 + x as f32 + 0.5;
                        let coverage = retouch_brush_coverage(
                            destination_x - point.x,
                            destination_y - point.y,
                            point.radius.max(0.5),
                            hardness,
                        );
                        if coverage <= 0.0 {
                            continue;
                        }
                        let source_position = retouch_dab_source_position(
                            [destination_x, destination_y],
                            *point,
                            retouch,
                            offset,
                        );
                        let Some(source) =
                            sample_scene_bilinear(source_scene, source_bounds, source_position)
                        else {
                            continue;
                        };
                        let index = y * width + x;
                        for channel in 0..3 {
                            let destination = output_scene[index * 3 + channel];
                            output_scene[index * 3 + channel] =
                                destination + (source[channel] - destination) * coverage;
                        }
                        stroke_coverage[index] = stroke_coverage[index].max(coverage);
                    }
                }
            }
            output_scene
        }
        RetouchTool::Heal => {
            // GIMP solves and composites every dab independently. Solving one
            // large Poisson field for the whole stroke smears texture along the
            // stroke and produces noticeably flatter results.
            let mut output_working = destination_scene
                .chunks_exact(3)
                .map(|pixel| pipeline_scene_to_working_rec2020(raw, [pixel[0], pixel[1], pixel[2]]))
                .collect::<Vec<_>>();
            for point in &brush.points {
                ensure_not_cancelled(cancellation)?;
                let Some([left, top, right, bottom]) =
                    retouch_dab_bounds(destination_bounds, *point)
                else {
                    continue;
                };
                let dab_width = right - left;
                let dab_height = bottom - top;
                let dab_pixels = dab_width * dab_height;
                let mut difference = vec![0.0f32; dab_pixels * 3 + 3];
                let mut source_perceptual = vec![[0.0f32; 3]; dab_pixels];
                let mut dab_coverage = vec![0.0f32; dab_pixels];
                let mut binary_mask = vec![false; dab_pixels];
                let mut source_rect_is_valid = true;

                for local_y in 0..dab_height {
                    let y = top + local_y;
                    let destination_y = destination_bounds.y as f32 + y as f32 + 0.5;
                    for local_x in 0..dab_width {
                        let x = left + local_x;
                        let destination_x = destination_bounds.x as f32 + x as f32 + 0.5;
                        let local_index = local_y * dab_width + local_x;
                        let source_position = retouch_dab_source_position(
                            [destination_x, destination_y],
                            *point,
                            retouch,
                            offset,
                        );
                        let Some(source_scene_pixel) =
                            sample_scene_bilinear(source_scene, source_bounds, source_position)
                        else {
                            source_rect_is_valid = false;
                            continue;
                        };
                        let source = pipeline_scene_to_working_rec2020(raw, source_scene_pixel)
                            .map(perceptual_encode_signed);
                        source_perceptual[local_index] = source;
                        let destination =
                            output_working[y * width + x].map(perceptual_encode_signed);
                        for channel in 0..3 {
                            difference[local_index * 3 + channel] =
                                destination[channel] - source[channel];
                        }
                        let coverage = retouch_brush_coverage(
                            destination_x - point.x,
                            destination_y - point.y,
                            point.radius.max(0.5),
                            hardness,
                        );
                        dab_coverage[local_index] = coverage;
                        binary_mask[local_index] = coverage > 0.0;
                    }
                }
                // GIMP skips a dab when its source rectangle falls off-canvas,
                // instead of solving it against synthetic edge pixels.
                if !source_rect_is_valid || !binary_mask.iter().any(|value| *value) {
                    continue;
                }
                gimp_heal_laplace_loop(
                    &mut difference,
                    dab_width,
                    dab_height,
                    &binary_mask,
                    cancellation,
                )?;
                for local_y in 0..dab_height {
                    let y = top + local_y;
                    for local_x in 0..dab_width {
                        let local_index = local_y * dab_width + local_x;
                        let coverage = dab_coverage[local_index];
                        if coverage <= 0.0 {
                            continue;
                        }
                        let x = left + local_x;
                        let index = y * width + x;
                        let source = source_perceptual[local_index];
                        let healed: [f32; 3] = std::array::from_fn(|channel| {
                            perceptual_decode_signed(
                                source[channel] + difference[local_index * 3 + channel],
                            )
                        });
                        for channel in 0..3 {
                            let destination = output_working[index][channel];
                            output_working[index][channel] =
                                destination + (healed[channel] - destination) * coverage;
                        }
                        stroke_coverage[index] = stroke_coverage[index].max(coverage);
                    }
                }
            }
            output_working
                .into_iter()
                .flat_map(|working| {
                    working_rec2020_to_canonical_remove_scene(raw, exposure, working)
                })
                .collect()
        }
    };

    anyhow::ensure!(
        stroke_coverage.iter().any(|value| *value > 0.0),
        "Retouch source does not overlap the brush"
    );

    let output_is_canonical = retouch.tool == RetouchTool::Heal;
    let mut left = width;
    let mut top = height;
    let mut right = 0usize;
    let mut bottom = 0usize;
    for y in 0..height {
        for x in 0..width {
            if stroke_coverage[y * width + x] > 0.0 {
                left = left.min(x);
                top = top.min(y);
                right = right.max(x + 1);
                bottom = bottom.max(y + 1);
            }
        }
    }
    anyhow::ensure!(
        right > left && bottom > top,
        "Retouch brush has no coverage"
    );
    let patch_bounds = NativeRect {
        x: destination_bounds.x + left as u32,
        y: destination_bounds.y + top as u32,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
    };
    let mut rgb16f = Vec::with_capacity((right - left) * (bottom - top) * 3);
    let mut alpha = Vec::with_capacity((right - left) * (bottom - top));
    for y in top..bottom {
        for x in left..right {
            let index = y * width + x;
            let canonical = if output_is_canonical {
                [
                    output[index * 3],
                    output[index * 3 + 1],
                    output[index * 3 + 2],
                ]
            } else {
                pipeline_scene_to_canonical_remove_scene(
                    raw,
                    exposure,
                    [
                        output[index * 3],
                        output[index * 3 + 1],
                        output[index * 3 + 2],
                    ],
                )
            };
            for value in canonical {
                let finite = if value.is_finite() { value } else { 0.0 };
                rgb16f.push(half::f16::from_f32(finite.clamp(-65_504.0, 65_504.0)).to_bits());
            }
            alpha.push(if stroke_coverage[index] > 0.0 { 255 } else { 0 });
        }
    }
    RemovePatch::new_scene(patch_bounds, rgb16f, alpha).map_err(anyhow::Error::msg)
}

fn retouch_dab_bounds(
    bounds: NativeRect,
    point: crate::pipeline::RemoveBrushPoint,
) -> Option<[usize; 4]> {
    let radius = point.radius.max(0.5);
    let left = (point.x - radius - bounds.x as f32).floor().max(0.0) as usize;
    let top = (point.y - radius - bounds.y as f32).floor().max(0.0) as usize;
    let right = (point.x + radius - bounds.x as f32)
        .ceil()
        .clamp(0.0, bounds.width as f32) as usize;
    let bottom = (point.y + radius - bounds.y as f32)
        .ceil()
        .clamp(0.0, bounds.height as f32) as usize;
    (right > left && bottom > top).then_some([left, top, right, bottom])
}

fn retouch_dab_source_position(
    destination: [f32; 2],
    point: crate::pipeline::RemoveBrushPoint,
    retouch: RetouchStroke,
    offset: [f32; 2],
) -> [f32; 2] {
    if retouch.alignment == RetouchAlignment::Fixed {
        [
            retouch.source[0] + destination[0] - point.x,
            retouch.source[1] + destination[1] - point.y,
        ]
    } else {
        [destination[0] + offset[0], destination[1] + offset[1]]
    }
}

fn retouch_brush_coverage(dx: f32, dy: f32, radius: f32, hardness: f32) -> f32 {
    let distance = dx.hypot(dy);
    if distance >= radius {
        return 0.0;
    }
    let inner = radius * hardness.clamp(0.0, 1.0);
    if distance <= inner || radius - inner <= f32::EPSILON {
        return 1.0;
    }
    let t = ((radius - distance) / (radius - inner)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn sample_scene_bilinear(
    scene: &[f32],
    bounds: NativeRect,
    position: [f32; 2],
) -> Option<[f32; 3]> {
    let local_x = position[0] - bounds.x as f32 - 0.5;
    let local_y = position[1] - bounds.y as f32 - 0.5;
    if local_x < -0.5
        || local_y < -0.5
        || local_x > bounds.width as f32 - 0.5
        || local_y > bounds.height as f32 - 0.5
    {
        return None;
    }
    let width = bounds.width as usize;
    let height = bounds.height as usize;
    let x = local_x.clamp(0.0, width.saturating_sub(1) as f32);
    let y = local_y.clamp(0.0, height.saturating_sub(1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let sample = |x: usize, y: usize, channel: usize| scene[(y * width + x) * 3 + channel];
    Some(std::array::from_fn(|channel| {
        let top = sample(x0, y0, channel) * (1.0 - tx) + sample(x1, y0, channel) * tx;
        let bottom = sample(x0, y1, channel) * (1.0 - tx) + sample(x1, y1, channel) * tx;
        top * (1.0 - ty) + bottom * ty
    }))
}

fn perceptual_encode_signed(value: f32) -> f32 {
    let sign = value.signum();
    let value = value.abs();
    sign * if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

fn perceptual_decode_signed(value: f32) -> f32 {
    let sign = value.signum();
    let value = value.abs();
    sign * if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// Port of GIMP 3.0.4's `app/paint/gimpheal.c` solver.
fn gimp_heal_laplace_loop(
    pixels: &mut [f32],
    width: usize,
    height: usize,
    mask: &[bool],
    cancellation: &AtomicBool,
) -> Result<()> {
    relax_harmonic_field(pixels, width, height, mask, cancellation, 500)
}

fn relax_harmonic_field(
    pixels: &mut [f32],
    width: usize,
    height: usize,
    mask: &[bool],
    cancellation: &AtomicBool,
    iterations: usize,
) -> Result<()> {
    const EPSILON: f32 = 0.1 / 255.0;
    let nmask = mask.iter().filter(|value| **value).count();
    if nmask == 0 {
        return Ok(());
    }
    let relaxation = 2.0 - 1.0 / (0.1575 * (nmask as f32).sqrt() + 0.8);
    let w = relaxation * 0.25;
    for iteration in 0..iterations {
        if iteration % 8 == 0 {
            ensure_not_cancelled(cancellation)?;
        }
        let mut error = 0.0f32;
        for parity in 0..2 {
            for y in 0..height {
                let first_x = (y & 1) ^ parity;
                for x in (first_x..width).step_by(2) {
                    let index = y * width + x;
                    if !mask[index] {
                        continue;
                    }
                    let degree = 4
                        - usize::from(x == 0)
                        - usize::from(x + 1 == width)
                        - usize::from(y == 0)
                        - usize::from(y + 1 == height);
                    for channel in 0..3 {
                        let mut neighbors = 0.0;
                        if x > 0 {
                            neighbors += pixels[(index - 1) * 3 + channel];
                        }
                        if x + 1 < width {
                            neighbors += pixels[(index + 1) * 3 + channel];
                        }
                        if y > 0 {
                            neighbors += pixels[(index - width) * 3 + channel];
                        }
                        if y + 1 < height {
                            neighbors += pixels[(index + width) * 3 + channel];
                        }
                        let location = index * 3 + channel;
                        let residual = degree as f32 * w * pixels[location] - w * neighbors;
                        pixels[location] -= residual;
                        error += residual * residual;
                    }
                }
            }
        }
        if error < EPSILON * EPSILON * w * w {
            break;
        }
    }
    Ok(())
}

fn infer_crop(
    model_path: &Path,
    crop: NativeRect,
    raw: &LoadedRaw,
    exposure: &ExposureParams,
    scene: &ResizedRemoveSceneCrop,
    mask: &RemoveMask,
    cancellation: &AtomicBool,
) -> Result<RemovePatch> {
    anyhow::ensure!(
        scene.width > 0 && scene.height > 0,
        "Remove reference is empty"
    );
    let expected = scene.width as usize * scene.height as usize * 3;
    anyhow::ensure!(
        scene.pixels.len() == expected,
        "scene Remove crop has an invalid RGB length"
    );

    let view_gain = remove_model_view_gain(raw, &scene.pixels);
    let mut srgb = Vec::with_capacity(expected);
    for pixel in scene.pixels.chunks_exact(3) {
        let converted = remove_scene_to_model_srgb(raw, [pixel[0], pixel[1], pixel[2]], view_gain);
        srgb.extend_from_slice(&converted);
    }
    let source: Rgb32FImage = ImageBuffer::from_raw(scene.width, scene.height, srgb)
        .context("construct developed Remove crop")?;
    let (model_width, model_height) =
        fit_dimensions(scene.width, scene.height, BIG_LAMA_INPUT_EDGE);
    let resized_source =
        image::imageops::resize(&source, model_width, model_height, FilterType::Lanczos3);
    let source_mask = crop_binary_mask(crop, mask);
    let initial_mask = resize_mask(&source_mask, model_width, model_height);
    let guard = model_mask_guard_radius(&initial_mask);
    let native_guard = (guard as f64 * crop.width as f64 / model_width as f64).ceil() as u32;
    let repair_mask = dilate_model_mask(&source_mask, native_guard);
    let model_mask = resize_mask(&repair_mask, model_width, model_height);
    // Reflect-pad both image and mask. Stretching a non-square crop changes
    // structures; treating reflected subject pixels as known causes ghosts.
    let resized = Rgb32FImage::from_fn(BIG_LAMA_INPUT_EDGE, BIG_LAMA_INPUT_EDGE, |x, y| {
        *resized_source.get_pixel(reflect(x, model_width), reflect(y, model_height))
    });
    let resized_mask = GrayImage::from_fn(BIG_LAMA_INPUT_EDGE, BIG_LAMA_INPUT_EDGE, |x, y| {
        *model_mask.get_pixel(reflect(x, model_width), reflect(y, model_height))
    });

    let plane = (BIG_LAMA_INPUT_EDGE * BIG_LAMA_INPUT_EDGE) as usize;
    let mut image_values = vec![0.0f32; plane * 3];
    let mut mask_values = vec![0.0f32; plane];
    for y in 0..BIG_LAMA_INPUT_EDGE {
        for x in 0..BIG_LAMA_INPUT_EDGE {
            let index = (y * BIG_LAMA_INPUT_EDGE + x) as usize;
            let pixel = resized.get_pixel(x, y);
            image_values[index] = pixel[0].clamp(0.0, 1.0);
            image_values[plane + index] = pixel[1].clamp(0.0, 1.0);
            image_values[plane * 2 + index] = pixel[2].clamp(0.0, 1.0);
            mask_values[index] = if resized_mask.get_pixel(x, y)[0] >= 128 {
                1.0
            } else {
                0.0
            };
        }
    }
    anyhow::ensure!(
        mask_values.iter().any(|value| *value > 0.5),
        "Remove mask vanished during resize"
    );

    let image_tensor = Tensor::from_array((
        [
            1usize,
            3,
            BIG_LAMA_INPUT_EDGE as usize,
            BIG_LAMA_INPUT_EDGE as usize,
        ],
        image_values,
    ))
    .context("create Big-LaMa image tensor")?;
    let mask_tensor = Tensor::from_array((
        [
            1usize,
            1,
            BIG_LAMA_INPUT_EDGE as usize,
            BIG_LAMA_INPUT_EDGE as usize,
        ],
        mask_values,
    ))
    .context("create Big-LaMa mask tensor")?;

    let output_values = with_model_session(
        AiModel::BigLama,
        model_path,
        SessionOptions::new("Big-LaMa Remove"),
        ModelRetention::Interactive(AiRuntimeContext::Remove),
        |session| {
            session.run_with_fallback(
                "Big-LaMa Remove ONNX inference",
                |ort_session, _accelerated| {
                    let outputs = ort_session
                        .run(ort::inputs![&image_tensor, &mask_tensor])
                        .context("run Big-LaMa ONNX inference")?;
                    let output = outputs
                        .values()
                        .next()
                        .context("Big-LaMa returned no output tensor")?;
                    let (shape, values) = output
                        .try_extract_tensor::<f32>()
                        .context("read Big-LaMa output tensor")?;
                    anyhow::ensure!(
                        shape.as_ref()
                            == [1, 3, BIG_LAMA_INPUT_EDGE as i64, BIG_LAMA_INPUT_EDGE as i64,],
                        "unexpected Big-LaMa output shape {shape:?}"
                    );
                    anyhow::ensure!(
                        values.len() == plane * 3 && values.iter().all(|value| value.is_finite()),
                        "Big-LaMa output tensor is invalid"
                    );
                    Ok(values.to_vec())
                },
            )
        },
    )?;

    let mut output_interleaved = vec![0.0f32; plane * 3];
    for index in 0..plane {
        output_interleaved[index * 3] = (output_values[index] / 255.0).clamp(0.0, 1.0);
        output_interleaved[index * 3 + 1] = (output_values[plane + index] / 255.0).clamp(0.0, 1.0);
        output_interleaved[index * 3 + 2] =
            (output_values[plane * 2 + index] / 255.0).clamp(0.0, 1.0);
    }
    let padded: Rgb32FImage =
        ImageBuffer::from_raw(BIG_LAMA_INPUT_EDGE, BIG_LAMA_INPUT_EDGE, output_interleaved)
            .context("construct Big-LaMa output image")?;
    let unpadded = image::imageops::crop_imm(&padded, 0, 0, model_width, model_height).to_image();
    let mut model_output =
        image::imageops::resize(&unpadded, scene.width, scene.height, FilterType::CatmullRom);
    let reference_mask = resize_mask(&repair_mask, scene.width, scene.height);
    harmonize_model_fill(&source, &reference_mask, &mut model_output, cancellation)?;
    ensure_not_cancelled(cancellation)?;
    build_cached_patch(crop, raw, exposure, view_gain, &model_output, &repair_mask)
}

/// Match the fill to the clean context at the mask edge. The offset is solved
/// smoothly inward, so local structure made by LaMa is kept while broad sky
/// or water colour differences do not form a visible patch.
fn harmonize_model_fill(
    source: &Rgb32FImage,
    mask: &GrayImage,
    output: &mut Rgb32FImage,
    cancellation: &AtomicBool,
) -> Result<()> {
    let width = mask.width() as usize;
    let height = mask.height() as usize;
    anyhow::ensure!(
        source.dimensions() == mask.dimensions() && output.dimensions() == mask.dimensions(),
        "Big-LaMa seam inputs have different dimensions"
    );
    let mut correction = vec![0.0f32; width * height * 3];
    let mut interior = vec![false; width * height];
    let mut has_boundary = false;
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            if mask.get_pixel(x as u32, y as u32)[0] == 0 {
                continue;
            }
            let boundary = [
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x, y.wrapping_sub(1)),
                (x, y + 1),
            ]
            .into_iter()
            .any(|(nx, ny)| {
                nx < width && ny < height && mask.get_pixel(nx as u32, ny as u32)[0] == 0
            });
            if boundary {
                has_boundary = true;
                // The native guard band puts this boundary in clean context.
                // Compare at the SAME location: averaging outside neighbors
                // biases gradients and pulls horizon edges into the repair.
                let target = source.get_pixel(x as u32, y as u32);
                let generated = output.get_pixel(x as u32, y as u32);
                for channel in 0..3 {
                    correction[index * 3 + channel] = target[channel] - generated[channel];
                }
            } else {
                interior[index] = true;
            }
        }
    }
    if !has_boundary {
        return Ok(());
    }
    harmonize_field_pyramid(&mut correction, width, height, &interior, cancellation)?;
    for (index, pixel) in output.pixels_mut().enumerate() {
        if mask.as_raw()[index] != 0 {
            for channel in 0..3 {
                pixel[channel] = (pixel[channel] + correction[index * 3 + channel]).clamp(0.0, 1.0);
            }
        }
    }
    Ok(())
}

/// Solve broad color shifts on a coarse grid first, then refine the boundary.
/// A zero-initialized, fixed-iteration solve at large sizes leaves the center
/// uncorrected. Restriction only uses fixed samples for coarse boundary values.
fn harmonize_field_pyramid(
    field: &mut [f32],
    width: usize,
    height: usize,
    interior: &[bool],
    cancellation: &AtomicBool,
) -> Result<()> {
    ensure_not_cancelled(cancellation)?;
    if width.max(height) <= 32 {
        return gimp_heal_laplace_loop(field, width, height, interior, cancellation);
    }
    let cw = width.div_ceil(2);
    let ch = height.div_ceil(2);
    let mut coarse = vec![0.0; cw * ch * 3];
    let mut coarse_interior = vec![true; cw * ch];
    for y in 0..ch {
        for x in 0..cw {
            let ci = y * cw + x;
            let mut count = 0;
            for fy in y * 2..(y * 2 + 2).min(height) {
                for fx in x * 2..(x * 2 + 2).min(width) {
                    let fi = fy * width + fx;
                    if !interior[fi] {
                        count += 1;
                        for c in 0..3 {
                            coarse[ci * 3 + c] += field[fi * 3 + c];
                        }
                    }
                }
            }
            if count > 0 {
                coarse_interior[ci] = false;
                for c in 0..3 {
                    coarse[ci * 3 + c] /= count as f32;
                }
            }
        }
    }
    harmonize_field_pyramid(&mut coarse, cw, ch, &coarse_interior, cancellation)?;
    let coarse = Rgb32FImage::from_raw(cw as u32, ch as u32, coarse).unwrap();
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            if interior[i] {
                // Pixel coordinates are tied to the 2x2 restriction cells,
                // including odd dimensions. Corrections are signed floats.
                let pixel = image::imageops::interpolate_bilinear(
                    &coarse,
                    ((x as f32 - 0.5) * 0.5).clamp(0.0, (cw - 1) as f32),
                    ((y as f32 - 0.5) * 0.5).clamp(0.0, (ch - 1) as f32),
                )
                .unwrap();
                field[i * 3..i * 3 + 3].copy_from_slice(&pixel.0);
            }
        }
    }
    relax_harmonic_field(field, width, height, interior, cancellation, 100)
}

fn dilate_model_mask(mask: &GrayImage, radius: u32) -> GrayImage {
    let distance = mask_distance(mask, true);
    GrayImage::from_fn(mask.width(), mask.height(), |x, y| {
        Luma([
            if distance[(y * mask.width() + x) as usize] <= radius as f32 {
                255
            } else {
                0
            },
        ])
    })
}

fn model_mask_guard_radius(mask: &GrayImage) -> u32 {
    let mut left = mask.width();
    let mut top = mask.height();
    let mut right = 0;
    let mut bottom = 0;
    for (x, y, pixel) in mask.enumerate_pixels() {
        if pixel[0] != 0 {
            left = left.min(x);
            top = top.min(y);
            right = right.max(x + 1);
            bottom = bottom.max(y + 1);
        }
    }
    if right <= left || bottom <= top {
        return 4;
    }
    ((right - left).max(bottom - top) as f32 * 0.10)
        .round()
        .clamp(4.0, 24.0) as u32
}

fn crop_binary_mask(crop: NativeRect, mask: &RemoveMask) -> GrayImage {
    let mut out = GrayImage::new(crop.width, crop.height);
    if let Some(intersection) = mask.bounds.intersect(crop) {
        for y in intersection.y..intersection.bottom() {
            for x in intersection.x..intersection.right() {
                if mask.contains_global(x, y) {
                    out.put_pixel(x - crop.x, y - crop.y, Luma([255]));
                }
            }
        }
    }
    out
}

fn build_cached_patch(
    crop: NativeRect,
    raw: &LoadedRaw,
    exposure: &ExposureParams,
    view_gain: f32,
    model_output: &Rgb32FImage,
    binary_mask: &GrayImage,
) -> Result<RemovePatch> {
    let scale = crop.width.max(crop.height) as f32 / BIG_LAMA_INPUT_EDGE as f32;
    let coverage = feather_mask(binary_mask, (3.0 * scale).max(2.0));
    let mut left = crop.width;
    let mut top = crop.height;
    let mut right = 0u32;
    let mut bottom = 0u32;
    for y in 0..crop.height {
        for x in 0..crop.width {
            let alpha = coverage[(y * crop.width + x) as usize];
            if alpha > 0 {
                left = left.min(x);
                top = top.min(y);
                right = right.max(x + 1);
                bottom = bottom.max(y + 1);
            }
        }
    }
    anyhow::ensure!(
        right > left && bottom > top,
        "Big-LaMa patch has no compositing coverage"
    );
    let bounds = NativeRect {
        x: crop.x + left,
        y: crop.y + top,
        width: right - left,
        height: bottom - top,
    };
    let pixels = bounds.width as usize * bounds.height as usize;
    let mut rgb16f = Vec::with_capacity(pixels * 3);
    let mut alpha = Vec::with_capacity(pixels);
    for y in top..bottom {
        for x in left..right {
            let u = (x as f32 + 0.5) / crop.width.max(1) as f32;
            let v = (y as f32 + 0.5) / crop.height.max(1) as f32;
            let pixel = sample_cubic(model_output, u, v);
            let generated = remove_model_srgb_to_canonical_scene(
                raw,
                exposure,
                [pixel[0], pixel[1], pixel[2]],
                view_gain,
            );
            let coverage = coverage[(y * crop.width + x) as usize];
            for channel in 0..3 {
                let finite = if generated[channel].is_finite() {
                    generated[channel]
                } else {
                    0.0
                };
                rgb16f.push(half::f16::from_f32(finite.clamp(-65_504.0, 65_504.0)).to_bits());
            }
            alpha.push(coverage);
        }
    }
    RemovePatch::new_scene(bounds, rgb16f, alpha).map_err(anyhow::Error::msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{RemoveBrushPoint, RemoveBrushStroke};

    #[test]
    fn big_lama_download_retries_and_resumes() {
        const {
            assert!(BIG_LAMA_DOWNLOAD.attempts > 1);
            assert!(BIG_LAMA_DOWNLOAD.resume);
        }
    }

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

    #[test]
    fn cached_patch_feather_stays_inside_binary_mask() {
        let crop = NativeRect {
            x: 100,
            y: 200,
            width: 64,
            height: 64,
        };
        let mut binary = GrayImage::new(64, 64);
        for y in 20..44 {
            for x in 18..46 {
                binary.put_pixel(x, y, Luma([255]));
            }
        }
        let restored = Rgb32FImage::from_pixel(16, 16, Rgb([0.4, 0.5, 0.6]));
        let raw = LoadedRaw::from_scene_linear_rec2020(1, 1, vec![0.18, 0.18, 0.18]).unwrap();
        let exposure = ExposureParams::default();
        let patch = build_cached_patch(crop, &raw, &exposure, 1.0, &restored, &binary).unwrap();
        assert_eq!(patch.bounds.x, crop.x + 18);
        assert_eq!(patch.bounds.y, crop.y + 20);
        assert_eq!(patch.bounds.right(), crop.x + 46);
        assert_eq!(patch.bounds.bottom(), crop.y + 44);
        assert!(patch.alpha.iter().all(|alpha| *alpha > 0));
    }

    #[test]
    fn remove_patch_stores_unblended_generated_color() {
        let raw = LoadedRaw::from_scene_linear_rec2020(1, 1, vec![0.18; 3]).unwrap();
        let exposure = ExposureParams::default();
        let target = [0.30, 0.25, 0.20];
        let model_rgb = remove_scene_to_model_srgb(&raw, target, 1.0);
        let output = Rgb32FImage::from_pixel(8, 8, Rgb(model_rgb));
        let mut mask = GrayImage::new(8, 8);
        for y in 2..6 {
            for x in 2..6 {
                mask.put_pixel(x, y, Luma([255]));
            }
        }
        let patch = build_cached_patch(
            NativeRect {
                x: 0,
                y: 0,
                width: 8,
                height: 8,
            },
            &raw,
            &exposure,
            1.0,
            &output,
            &mask,
        )
        .unwrap();
        assert!(!patch.coverage_baked);
        assert!(patch.alpha.iter().any(|value| *value < 255));
        for rgb in patch.rgb_scene16f.chunks_exact(3) {
            for (bits, expected) in rgb.iter().zip(target) {
                assert!((half::f16::from_bits(*bits).to_f32() - expected).abs() < 0.002);
            }
        }
    }

    #[test]
    fn model_mask_guard_band_covers_resampling_fringe() {
        let mut mask = GrayImage::new(32, 32);
        mask.put_pixel(16, 16, Luma([255]));
        let expanded = dilate_model_mask(&mask, model_mask_guard_radius(&mask));
        assert_eq!(expanded.get_pixel(12, 16)[0], 255);
        assert_eq!(expanded.get_pixel(16, 12)[0], 255);
        assert_eq!(expanded.get_pixel(11, 16)[0], 0);
    }

    #[test]
    fn model_mask_guard_grows_with_subject_size() {
        let mut mask = GrayImage::new(512, 512);
        for y in 150..350 {
            for x in 200..300 {
                mask.put_pixel(x, y, Luma([255]));
            }
        }
        assert_eq!(model_mask_guard_radius(&mask), 20);
        let expanded = dilate_model_mask(&mask, 20);
        assert_eq!(expanded.get_pixel(180, 250)[0], 255);
        assert_eq!(expanded.get_pixel(179, 250)[0], 0);
    }

    #[test]
    fn model_fill_matches_two_different_background_tones() {
        let mut source = Rgb32FImage::new(48, 48);
        let mut output = Rgb32FImage::new(48, 48);
        let mut mask = GrayImage::new(48, 48);
        for y in 0..48 {
            let background = if y < 24 { 0.60 } else { 0.28 };
            for x in 0..48 {
                source.put_pixel(x, y, Rgb([background; 3]));
                output.put_pixel(x, y, Rgb([background + 0.07; 3]));
                if (10..38).contains(&x) && (8..40).contains(&y) {
                    mask.put_pixel(x, y, Luma([255]));
                }
            }
        }
        harmonize_model_fill(&source, &mask, &mut output, &AtomicBool::new(false)).unwrap();
        for (x, y, expected) in [(24, 16, 0.60), (24, 32, 0.28)] {
            let actual = output.get_pixel(x, y)[0];
            assert!((actual - expected).abs() < 0.01, "{x},{y}: {actual}");
        }
        // Pixels beyond the mask remain exactly as returned by the model.
        assert_eq!(output.get_pixel(0, 0)[0], 0.67);
    }

    #[test]
    fn large_fill_color_correction_reaches_center_without_flattening_texture() {
        let (width, height) = (769, 513);
        let clean = Rgb32FImage::from_fn(width, height, |x, y| {
            let ramp = 0.2 + 0.2 * x as f32 / width as f32 + 0.1 * y as f32 / height as f32;
            Rgb([ramp, ramp + 0.02, ramp + 0.04])
        });
        let mask = GrayImage::from_fn(width, height, |x, y| {
            Luma([if (40..730).contains(&x) && (30..480).contains(&y) {
                255
            } else {
                0
            }])
        });
        let mut source = clean.clone();
        let mut output = clean.clone();
        for (x, y, pixel) in output.enumerate_pixels_mut() {
            let drift = 0.20 + 0.08 * x as f32 / width as f32;
            for c in 0..3 {
                pixel[c] += drift;
            }
            if (100..650).contains(&x) && (100..410).contains(&y) {
                source.put_pixel(x, y, Rgb([0.9, 0.05, 0.1])); // original subject must never leak back
            }
        }
        harmonize_model_fill(&source, &mask, &mut output, &AtomicBool::new(false)).unwrap();
        let mut max_error = 0.0f32;
        for (x, y, pixel) in output.enumerate_pixels() {
            if mask.get_pixel(x, y)[0] != 0 {
                for c in 0..3 {
                    max_error = max_error.max((pixel[c] - clean.get_pixel(x, y)[c]).abs());
                }
            }
        }
        assert!(max_error < 0.003, "remaining color mismatch: {max_error}");
    }

    #[test]
    fn color_correction_is_cancellable() {
        let source = Rgb32FImage::from_pixel(64, 64, Rgb([0.4; 3]));
        let mut output = Rgb32FImage::from_pixel(64, 64, Rgb([0.5; 3]));
        let mask = GrayImage::from_fn(64, 64, |x, y| {
            Luma([if (10..54).contains(&x) && (10..54).contains(&y) {
                255
            } else {
                0
            }])
        });
        assert!(harmonize_model_fill(&source, &mask, &mut output, &AtomicBool::new(true)).is_err());
    }

    /// Run explicitly with CALIBRAW_TEST_LAMA_MODEL and CALIBRAW_TEST_ORT_LIBRARY
    /// pointing to installed artifacts. No model download occurs in this test.
    #[test]
    #[ignore = "requires installed Big-LaMa and ONNX Runtime artifacts"]
    fn installed_lama_removes_subject_from_smooth_gradient() {
        let model = PathBuf::from(std::env::var("CALIBRAW_TEST_LAMA_MODEL").unwrap());
        let runtime = PathBuf::from(std::env::var("CALIBRAW_TEST_ORT_LIBRARY").unwrap());
        assert!(big_lama_model_is_verified(&model));
        let runtime_bytes = std::fs::read(&runtime).unwrap();
        let hash = hex::encode(ring::digest::digest(&ring::digest::SHA256, &runtime_bytes));
        crate::ai_masks::initialize_runtime(Some(&runtime), Some(&hash)).unwrap();
        let width = 768;
        let height = 512;
        let crop = NativeRect {
            x: 0,
            y: 0,
            width,
            height,
        };
        let clean = Rgb32FImage::from_fn(width, height, |x, y| {
            let value = 0.15 + x as f32 * 0.00008 + y as f32 * 0.00005;
            Rgb([value, value * 1.05, value * 1.12])
        });
        let mut source = clean.clone();
        for y in 160..352 {
            for x in 320..448 {
                source.put_pixel(x, y, Rgb([0.7, 0.15, 0.03]));
            }
        }
        let raw =
            LoadedRaw::from_scene_linear_rec2020(width, height, source.clone().into_raw()).unwrap();
        let brush = RemoveBrushStroke {
            points: vec![RemoveBrushPoint {
                x: 384.0,
                y: 256.0,
                radius: 130.0,
            }],
            dilation_radius: 0,
        };
        let mask = rasterize_remove_brush(width, height, &brush).unwrap();
        let scene = ResizedRemoveSceneCrop {
            width,
            height,
            pixels: source.into_raw(),
        };
        let patch = infer_crop(
            &model,
            crop,
            &raw,
            &ExposureParams::default(),
            &scene,
            &mask,
            &AtomicBool::new(false),
        )
        .unwrap();
        let mut repaired = scene.pixels.clone();
        crate::pipeline::composite_patch_into_linear_region(&patch, crop, &mut repaired);
        let mut error = 0.0f32;
        let mut count = 0;
        for y in 160..352 {
            for x in 320..448 {
                let i = (y * width + x) as usize * 3;
                for c in 0..3 {
                    error += (repaired[i + c] - clean.get_pixel(x, y)[c]).abs();
                    count += 1;
                }
            }
        }
        let mean_error = error / count as f32;
        eprintln!("LaMa smooth-gradient mean absolute linear RGB error: {mean_error}");
        assert!(
            mean_error < 0.025,
            "visible residual subject or color shift: {mean_error}"
        );
        assert_eq!(&repaired[..3], &scene.pixels[..3]);
        if let Ok(directory) = std::env::var("CALIBRAW_TEST_REMOVE_OUTPUT") {
            let directory = PathBuf::from(directory);
            std::fs::create_dir_all(&directory).unwrap();
            let gain = remove_model_view_gain(&raw, &scene.pixels);
            for (name, pixels) in [
                ("input", &scene.pixels),
                ("repaired", &repaired),
                ("reference", clean.as_raw()),
            ] {
                let display = image::RgbImage::from_fn(width, height, |x, y| {
                    let i = (y * width + x) as usize * 3;
                    Rgb(remove_scene_to_model_srgb(
                        &raw,
                        [pixels[i], pixels[i + 1], pixels[i + 2]],
                        gain,
                    )
                    .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8))
                });
                display.save(directory.join(format!("{name}.png"))).unwrap();
            }
        }
    }

    #[test]
    fn nearest_model_mask_stays_binary() {
        let brush = RemoveBrushStroke {
            points: vec![RemoveBrushPoint {
                x: 50.0,
                y: 40.0,
                radius: 8.0,
            }],
            dilation_radius: 2,
        };
        let mask = rasterize_remove_brush(100, 80, &brush).unwrap();
        let crop = NativeRect {
            x: 10,
            y: 0,
            width: 80,
            height: 80,
        };
        let source = crop_binary_mask(crop, &mask);
        let resized = image::imageops::resize(
            &source,
            BIG_LAMA_INPUT_EDGE,
            BIG_LAMA_INPUT_EDGE,
            FilterType::Nearest,
        );
        assert!(resized
            .pixels()
            .all(|pixel| pixel[0] == 0 || pixel[0] == 255));
    }
}
