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
    let stroke_seed = remove_stroke_seed(&brush);
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
        BIG_LAMA_INPUT_EDGE,
    )
    .with_context(|| {
        format!(
            "render bounded Big-LaMa scene for native context {}x{} at {},{}",
            crop.width, crop.height, crop.x, crop.y,
        )
    })?;
    ensure_not_cancelled(&request.cancellation)?;

    // The model sees a 512px proxy. Render the area around the actual repair
    // at native resolution so its boundary and grain match the final image.
    let native_bounds =
        padded_remove_bounds(mask.bounds, request.raw.width, request.raw.height, 12);
    let native_scene = if native_bounds.width <= 2_048 && native_bounds.height <= 2_048 {
        let pixels = render_remove_scene_crop(DevelopedCropJob {
            device: request.device.clone(),
            queue: request.queue.clone(),
            raw: Arc::clone(&request.raw),
            geometry: request.geometry,
            exposure: request.exposure,
            masks: request.masks.clone(),
            remove: request.existing,
            crop: native_bounds,
            program_prewarm: request.program_prewarm.clone(),
        })
        .context("render native Remove boundary")?;
        Some((
            native_bounds,
            ImageBuffer::from_raw(native_bounds.width, native_bounds.height, pixels)
                .context("construct native Remove boundary scene")?,
        ))
    } else {
        None
    };
    ensure_not_cancelled(&request.cancellation)?;

    let patch = infer_crop(
        &request.model_path,
        crop,
        &request.raw,
        &request.exposure,
        &scene,
        &mask,
        native_scene.as_ref(),
        stroke_seed,
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

fn padded_remove_bounds(bounds: NativeRect, width: u32, height: u32, padding: u32) -> NativeRect {
    let x = bounds.x.saturating_sub(padding);
    let y = bounds.y.saturating_sub(padding);
    NativeRect {
        x,
        y,
        width: bounds.right().saturating_add(padding).min(width) - x,
        height: bounds.bottom().saturating_add(padding).min(height) - y,
    }
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
    const EPSILON: f32 = 0.1 / 255.0;
    const MAX_ITERATIONS: usize = 500;
    let nmask = mask.iter().filter(|value| **value).count();
    if nmask == 0 {
        return Ok(());
    }
    let relaxation = 2.0 - 1.0 / (0.1575 * (nmask as f32).sqrt() + 0.8);
    let w = relaxation * 0.25;
    for iteration in 0..MAX_ITERATIONS {
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
    native_scene: Option<&(NativeRect, Rgb32FImage)>,
    stroke_seed: u32,
    cancellation: &AtomicBool,
) -> Result<RemovePatch> {
    anyhow::ensure!(
        scene.width <= BIG_LAMA_INPUT_EDGE && scene.height <= BIG_LAMA_INPUT_EDGE,
        "Big-LaMa working scene {}x{} exceeds {}px",
        scene.width,
        scene.height,
        BIG_LAMA_INPUT_EDGE,
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
    let resized = image::imageops::resize(
        &source,
        BIG_LAMA_INPUT_EDGE,
        BIG_LAMA_INPUT_EDGE,
        FilterType::Lanczos3,
    );
    let source_mask = crop_binary_mask(crop, mask);
    let resized_mask = image::imageops::resize(
        &source_mask,
        BIG_LAMA_INPUT_EDGE,
        BIG_LAMA_INPUT_EDGE,
        FilterType::Nearest,
    );

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
    let model_output: Rgb32FImage =
        ImageBuffer::from_raw(BIG_LAMA_INPUT_EDGE, BIG_LAMA_INPUT_EDGE, output_interleaved)
            .context("construct Big-LaMa output image")?;
    let source_scene: Rgb32FImage =
        ImageBuffer::from_raw(scene.width, scene.height, scene.pixels.clone())
            .context("construct bounded Big-LaMa source scene")?;

    build_cached_patch(
        crop,
        raw,
        exposure,
        &source_scene,
        view_gain,
        &model_output,
        &source_mask,
        native_scene,
        stroke_seed,
        cancellation,
    )
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
    source_scene: &Rgb32FImage,
    view_gain: f32,
    model_output: &Rgb32FImage,
    binary_mask: &GrayImage,
    native_scene: Option<&(NativeRect, Rgb32FImage)>,
    stroke_seed: u32,
    cancellation: &AtomicBool,
) -> Result<RemovePatch> {
    let scale = crop.width.max(crop.height) as f32 / BIG_LAMA_INPUT_EDGE as f32;
    let sigma = (1.25 * scale).clamp(1.25, 5.0);
    let distance = distance_transform_inside(binary_mask);
    let feather_width = (sigma * 2.0).max(1.0);
    let mut left = crop.width;
    let mut top = crop.height;
    let mut right = 0u32;
    let mut bottom = 0u32;
    for y in 0..crop.height {
        for x in 0..crop.width {
            let binary = binary_mask.get_pixel(x, y)[0];
            let index = y as usize * crop.width as usize + x as usize;
            let coverage = if binary != 0 {
                ((distance[index] / feather_width).clamp(0.0, 1.0) * 255.0).round() as u8
            } else {
                0
            };
            let alpha = coverage;
            if alpha >= 2 {
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
    let work_left = left.saturating_sub(12);
    let work_top = top.saturating_sub(12);
    let work_right = (right + 12).min(crop.width);
    let work_bottom = (bottom + 12).min(crop.height);
    let width = (work_right - work_left) as usize;
    let height = (work_bottom - work_top) as usize;
    let pixels = width * height;
    let mut generated_perceptual = vec![[0.0f32; 3]; pixels];
    let mut source_perceptual = vec![[0.0f32; 3]; pixels];
    let mut source_linear = vec![[0.0f32; 3]; pixels];
    let mut coverage_values = vec![0u8; pixels];
    for y in work_top..work_bottom {
        ensure_not_cancelled(cancellation)?;
        for x in work_left..work_right {
            let local_index = (y - work_top) as usize * width + (x - work_left) as usize;
            let u = (x as f32 + 0.5) / crop.width.max(1) as f32;
            let v = (y as f32 + 0.5) / crop.height.max(1) as f32;
            let pixel: Rgb<f32> = image::imageops::sample_bilinear(model_output, u, v)
                .context("sample upscaled Big-LaMa output")?;
            let generated = remove_model_srgb_to_canonical_scene(
                raw,
                exposure,
                [pixel[0], pixel[1], pixel[2]],
                view_gain,
            );
            let source_pixel = if let Some((native_bounds, native_pixels)) = native_scene {
                let global_x = crop.x + x;
                let global_y = crop.y + y;
                if global_x >= native_bounds.x
                    && global_x < native_bounds.right()
                    && global_y >= native_bounds.y
                    && global_y < native_bounds.bottom()
                {
                    *native_pixels.get_pixel(global_x - native_bounds.x, global_y - native_bounds.y)
                } else {
                    image::imageops::sample_bilinear(source_scene, u, v)
                        .context("sample bounded Big-LaMa source scene")?
                }
            } else {
                image::imageops::sample_bilinear(source_scene, u, v)
                    .context("sample bounded Big-LaMa source scene")?
            };
            let source = pipeline_scene_to_canonical_remove_scene(
                raw,
                exposure,
                [source_pixel[0], source_pixel[1], source_pixel[2]],
            );
            let binary = binary_mask.get_pixel(x, y)[0];
            let crop_index = y as usize * crop.width as usize + x as usize;
            let coverage = if binary != 0 {
                ((distance[crop_index] / feather_width).clamp(0.0, 1.0) * 255.0).round() as u8
            } else {
                0
            };
            generated_perceptual[local_index] = generated.map(perceptual_encode_signed);
            source_perceptual[local_index] = source.map(perceptual_encode_signed);
            source_linear[local_index] = source;
            coverage_values[local_index] = coverage;
        }
    }

    // Anchor the color correction in clean pixels outside the painted mask.
    // Sampling inside the mask can carry the object being removed into LaMa's
    // otherwise plausible output.
    let mut mask = vec![false; pixels];
    let mut difference = vec![0.0f32; pixels * 3];
    for index in 0..pixels {
        let x = index % width;
        let y = index / width;
        mask[index] = binary_mask.get_pixel(work_left + x as u32, work_top + y as u32)[0] != 0;
    }
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            if mask[index] {
                continue;
            }
            let touches_interior = (x > 0 && mask[index - 1])
                || (x + 1 < width && mask[index + 1])
                || (y > 0 && mask[index - width])
                || (y + 1 < height && mask[index + width]);
            if !touches_interior {
                continue;
            }
            for channel in 0..3 {
                difference[index * 3 + channel] =
                    source_perceptual[index][channel] - generated_perceptual[index][channel];
            }
        }
    }
    gimp_heal_laplace_loop(&mut difference, width, height, &mask, cancellation)?;
    let noise_sigma = if native_scene.is_some() {
        boundary_noise_sigma(&source_perceptual, width, height, &mask)
    } else {
        [0.0; 3]
    };

    let patch_pixels = bounds.width as usize * bounds.height as usize;
    let mut rgb16f = Vec::with_capacity(patch_pixels * 3);
    let mut alpha = Vec::with_capacity(patch_pixels);
    for y in top..bottom {
        for x in left..right {
            let index = (y - work_top) as usize * width + (x - work_left) as usize;
            let coverage = coverage_values[index];
            let mix = coverage as f32 / 255.0;
            let global_x = crop.x + x;
            let global_y = crop.y + y;
            let corrected_generated: [f32; 3] = std::array::from_fn(|channel| {
                perceptual_decode_signed(
                    generated_perceptual[index][channel]
                        + difference[index * 3 + channel]
                        + pixel_noise(global_x, global_y, 0, stroke_seed) * noise_sigma[channel],
                )
            });
            for channel in 0..3 {
                let source = source_linear[index][channel];
                let value = source * (1.0 - mix) + corrected_generated[channel] * mix;
                let finite = if value.is_finite() { value } else { source };
                rgb16f.push(half::f16::from_f32(finite.clamp(-65_504.0, 65_504.0)).to_bits());
            }
            alpha.push(coverage);
        }
    }
    RemovePatch::new_scene(bounds, rgb16f, alpha).map_err(anyhow::Error::msg)
}

fn distance_transform_inside(mask: &GrayImage) -> Vec<f32> {
    let (width, height) = (mask.width() as usize, mask.height() as usize);
    const INF: f32 = 1.0e6;
    const DIAGONAL: f32 = std::f32::consts::SQRT_2;
    let mut distance = vec![INF; width * height];
    for y in 0..height {
        for x in 0..width {
            if mask.get_pixel(x as u32, y as u32)[0] == 0 {
                distance[y * width + x] = 0.0;
            }
        }
    }

    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            if x > 0 {
                distance[index] = distance[index].min(distance[index - 1] + 1.0);
            }
            if y > 0 {
                distance[index] = distance[index].min(distance[index - width] + 1.0);
            }
            if x > 0 && y > 0 {
                distance[index] = distance[index].min(distance[index - width - 1] + DIAGONAL);
            }
            if x + 1 < width && y > 0 {
                distance[index] = distance[index].min(distance[index - width + 1] + DIAGONAL);
            }
        }
    }

    for y in (0..height).rev() {
        for x in (0..width).rev() {
            let index = y * width + x;
            if x + 1 < width {
                distance[index] = distance[index].min(distance[index + 1] + 1.0);
            }
            if y + 1 < height {
                distance[index] = distance[index].min(distance[index + width] + 1.0);
            }
            if x + 1 < width && y + 1 < height {
                distance[index] = distance[index].min(distance[index + width + 1] + DIAGONAL);
            }
            if x > 0 && y + 1 < height {
                distance[index] = distance[index].min(distance[index + width - 1] + DIAGONAL);
            }
        }
    }
    distance
}

fn remove_stroke_seed(brush: &RemoveBrushStroke) -> u32 {
    let mut seed = 0xA511_E9B3u32 ^ brush.dilation_radius;
    for point in &brush.points {
        for value in [point.x.to_bits(), point.y.to_bits(), point.radius.to_bits()] {
            seed ^= value;
            seed = seed.rotate_left(13).wrapping_mul(0x9E37_79B1);
        }
    }
    seed
}

fn pixel_noise(x: u32, y: u32, channel: u32, seed: u32) -> f32 {
    let mut hash = x.wrapping_mul(0x9E37_79B1)
        ^ y.wrapping_mul(0x85EB_CA77)
        ^ channel.wrapping_mul(0xC2B2_AE3D)
        ^ seed;
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(0x2C1B_3C6D);
    hash ^= hash >> 12;
    hash = hash.wrapping_mul(0x297A_2D39);
    hash ^= hash >> 15;
    let u1 = ((hash & 0xFFFF) as f32 + 0.5) / 65_536.0;
    let u2 = (((hash >> 16) & 0xFFFF) as f32 + 0.5) / 65_536.0;
    (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
}

fn boundary_noise_sigma(
    source: &[[f32; 3]],
    width: usize,
    height: usize,
    mask: &[bool],
) -> [f32; 3] {
    let mut clean = GrayImage::new(width as u32, height as u32);
    for (index, masked) in mask.iter().enumerate() {
        clean.as_mut()[index] = if *masked { 0 } else { 255 };
    }
    let outside_distance = distance_transform_inside(&clean);
    let mut sum = [0.0f64; 3];
    let mut sum_squared = [0.0f64; 3];
    let mut count = 0u64;
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            if !(3.0..=12.0).contains(&outside_distance[index])
                || x == 0
                || y == 0
                || x + 1 == width
                || y + 1 == height
            {
                continue;
            }
            for channel in 0..3 {
                let mut low_pass = 0.0f32;
                for sample_y in (y - 1)..=(y + 1) {
                    for sample_x in (x - 1)..=(x + 1) {
                        low_pass += source[sample_y * width + sample_x][channel];
                    }
                }
                let residual = source[index][channel] - low_pass / 9.0;
                sum[channel] += residual as f64;
                sum_squared[channel] += (residual as f64).powi(2);
            }
            count += 1;
        }
    }
    if count == 0 {
        return [0.0; 3];
    }
    std::array::from_fn(|channel| {
        let mean = sum[channel] / count as f64;
        ((sum_squared[channel] / count as f64 - mean * mean)
            .max(0.0)
            .sqrt() as f32
            * 1.06)
            .min(0.03)
    })
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
        let source_scene = Rgb32FImage::from_pixel(8, 8, Rgb([0.18, 0.18, 0.18]));
        let patch = build_cached_patch(
            crop,
            &raw,
            &exposure,
            &source_scene,
            1.0,
            &restored,
            &binary,
            None,
            0,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(patch.bounds.x, crop.x + 18);
        assert_eq!(patch.bounds.y, crop.y + 20);
        assert_eq!(patch.bounds.right(), crop.x + 46);
        assert_eq!(patch.bounds.bottom(), crop.y + 44);
        assert!(patch.alpha.iter().all(|alpha| *alpha > 0));
    }

    #[test]
    fn remove_correction_does_not_pull_masked_object_color_into_interior() {
        let crop = NativeRect {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
        };
        let mut binary = GrayImage::new(64, 64);
        let mut object_scene = Rgb32FImage::from_pixel(64, 64, Rgb([0.2; 3]));
        let clean_scene = Rgb32FImage::from_pixel(64, 64, Rgb([0.2; 3]));
        for y in 16..48 {
            for x in 16..48 {
                binary.put_pixel(x, y, Luma([255]));
                object_scene.put_pixel(x, y, Rgb([0.9; 3]));
            }
        }
        let raw = LoadedRaw::from_scene_linear_rec2020(1, 1, vec![0.2; 3]).unwrap();
        let model = Rgb32FImage::from_pixel(64, 64, Rgb([0.5; 3]));
        let render = |scene: &Rgb32FImage| {
            build_cached_patch(
                crop,
                &raw,
                &ExposureParams::default(),
                scene,
                1.0,
                &model,
                &binary,
                Some(&(crop, scene.clone())),
                1,
                &AtomicBool::new(false),
            )
            .unwrap()
        };
        let clean = render(&clean_scene);
        let object = render(&object_scene);
        let interior = ((32 - 16) * 32 + (19 - 16)) * 3;
        assert_eq!(
            &clean.rgb_scene16f[interior..interior + 3],
            &object.rgb_scene16f[interior..interior + 3]
        );
    }

    #[test]
    fn grain_estimate_uses_clean_pixels_outside_mask() {
        let width = 32;
        let height = 32;
        let mut mask = vec![false; width * height];
        let mut scene = vec![[0.5; 3]; width * height];
        for y in 12..20 {
            for x in 12..20 {
                let index = y * width + x;
                mask[index] = true;
                scene[index] = [0.95; 3];
            }
        }
        assert_eq!(boundary_noise_sigma(&scene, width, height, &mask), [0.0; 3]);
        for y in 0..height {
            for x in 0..width {
                if !mask[y * width + x] {
                    scene[y * width + x] = [0.5 + pixel_noise(x as u32, y as u32, 0, 7) * 0.01; 3];
                }
            }
        }
        let sigma = boundary_noise_sigma(&scene, width, height, &mask);
        assert!(sigma.iter().all(|value| *value > 0.005 && *value < 0.02));
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
