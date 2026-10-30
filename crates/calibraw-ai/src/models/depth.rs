//! Scene depth from Depth Anything V2 Small, aligned to the photo's edges.

use super::masks::mask_model_retention;
use super::{depth_refine, normalized_rgb_input};
use crate::config::{MemoryClass, CAPABILITIES};
use crate::placement::SessionOptions;
use crate::registry::{AiModel, ModelInfo};
use crate::slot::with_model_session;
use anyhow::{Context, Result};
use image::{imageops::FilterType, ImageBuffer, Luma, Rgba};
use ort::value::Tensor;
use std::path::Path;

pub const MODEL: ModelInfo = AiModel::Depth.spec().info();
/// The page listing the depth model's files.
pub const ARTIFACT_PAGE_URL: &str =
    "https://huggingface.co/Duecki/CalibRaw-Artifacts/tree/main/models/da2";
const INPUT_EDGE: u32 = MODEL.input.edge();

pub fn depth_model_is_verified(path: &Path) -> bool {
    AiModel::Depth.spec().is_installed(path)
}

fn session_options() -> SessionOptions {
    session_options_for(CAPABILITIES.memory)
}

const fn session_options_for(memory: MemoryClass) -> SessionOptions {
    let options = AiModel::Depth.spec().session;
    match memory {
        // Depth runs on the CPU beside the RAW preview, in the lean session
        // every model gets there (`placement::session`).
        MemoryClass::Constrained => options.cpu_only(),
        MemoryClass::Standard => options,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Letterbox {
    width: u32,
    height: u32,
    left: u32,
    top: u32,
    edge: u32,
}

impl Letterbox {
    fn new(width: u32, height: u32, edge: u32) -> Self {
        let scale = edge as f64 / width.max(height) as f64;
        let fitted_width = ((width as f64 * scale).round() as u32).clamp(1, edge);
        let fitted_height = ((height as f64 * scale).round() as u32).clamp(1, edge);
        Self {
            width: fitted_width,
            height: fitted_height,
            left: (edge - fitted_width) / 2,
            top: (edge - fitted_height) / 2,
            edge,
        }
    }

    fn output_rect(self, output_width: u32, output_height: u32) -> (u32, u32, u32, u32) {
        let x0 = (self.left as f64 * output_width as f64 / self.edge as f64).round() as u32;
        let y0 = (self.top as f64 * output_height as f64 / self.edge as f64).round() as u32;
        let x1 = ((self.left + self.width) as f64 * output_width as f64 / self.edge as f64).round()
            as u32;
        let y1 = ((self.top + self.height) as f64 * output_height as f64 / self.edge as f64).round()
            as u32;
        let x0 = x0.min(output_width - 1);
        let y0 = y0.min(output_height - 1);
        (
            x0,
            y0,
            x1.clamp(x0 + 1, output_width),
            y1.clamp(y0 + 1, output_height),
        )
    }
}

pub(super) fn depth_mask(
    model_path: &Path,
    image: &ImageBuffer<Rgba<u8>, Vec<u8>>,
) -> Result<Vec<u8>> {
    let edge = INPUT_EDGE;
    let (width, height) = image.dimensions();
    anyhow::ensure!(width > 0 && height > 0, "empty depth-mask input");
    let box_rect = Letterbox::new(width, height, edge);
    let resized =
        image::imageops::resize(image, box_rect.width, box_rect.height, FilterType::Lanczos3);
    let mut canvas = ImageBuffer::from_pixel(edge, edge, Rgba([124, 116, 104, 255]));
    image::imageops::overlay(
        &mut canvas,
        &resized,
        box_rect.left as i64,
        box_rect.top as i64,
    );

    let pixels = normalized_rgb_input(&canvas)?;
    let input = Tensor::from_array(([1usize, 3, edge as usize, edge as usize], pixels))
        .context("create depth input tensor")?;
    let started = std::time::Instant::now();
    let (shape, raw) = with_model_session(
        AiModel::Depth,
        model_path,
        session_options(),
        mask_model_retention(true),
        |session| {
            session.run_with_fallback("depth ONNX inference", |ort_session, _| {
                let outputs = ort_session
                    .run(ort::inputs!["pixel_values" => &input])
                    .context("run depth inference")?;
                let output = outputs
                    .get("predicted_depth")
                    .context("depth model returned no depth output")?;
                let (shape, values) = output
                    .try_extract_tensor::<f32>()
                    .context("read depth tensor")?;
                Ok((shape.as_ref().to_vec(), values.to_vec()))
            })
        },
    )?;
    let (output_width, output_height) = validate_output(&shape, &raw)?;
    calibraw_core::diagnostics::record(format!(
        "AI {}: {width}x{height} source, {output_width}x{output_height} depth in {:.3}s",
        MODEL.name,
        started.elapsed().as_secs_f64()
    ));
    let mut restored = restore_depth(&raw, output_width, output_height, box_rect, width, height)?;
    let started = std::time::Instant::now();
    restored.align_edges(image)?;
    calibraw_core::diagnostics::record(format!(
        "AI depth edges aligned to the {width}x{height} photo in {:.3}s",
        started.elapsed().as_secs_f64()
    ));
    Ok(quantize_depth(&restored.depth))
}

fn validate_output(shape: &[i64], values: &[f32]) -> Result<(u32, u32)> {
    let (height, width) = match shape {
        [1, h, w] => (*h, *w),
        _ => anyhow::bail!("unexpected depth output shape {shape:?}"),
    };
    anyhow::ensure!(
        height > 0 && width > 0 && height <= INPUT_EDGE as i64 && width <= INPUT_EDGE as i64,
        "invalid depth output dimensions {shape:?}"
    );
    anyhow::ensure!(
        values.len() == (height * width) as usize,
        "depth output size does not match its shape"
    );
    anyhow::ensure!(
        values.iter().all(|value| value.is_finite()),
        "depth output contains non-finite depth values"
    );
    Ok((width as u32, height as u32))
}

fn normalize_depth(pixels: &[f32]) -> Result<Vec<f32>> {
    anyhow::ensure!(
        !pixels.is_empty(),
        "depth model returned an empty depth map"
    );
    anyhow::ensure!(
        pixels.iter().all(|value| value.is_finite()),
        "depth output contains non-finite depth values"
    );
    let mut sorted = pixels.to_vec();
    sorted.sort_unstable_by(f32::total_cmp);
    // Clip the outer 1% so a few extreme predictions cannot collapse most
    // of the scene into the same 8-bit value. Polarity is applied afterward.
    let tail = sorted.len() / 100;
    let mut min = sorted[tail];
    let mut max = sorted[sorted.len() - 1 - tail];
    if max <= min {
        min = sorted[0];
        max = sorted[sorted.len() - 1];
    }
    anyhow::ensure!(
        max > min,
        "depth model returned a constant depth map; no depth range could be detected"
    );
    // Use f64 here to retain scale invariance and avoid overflow in max-min.
    let span = max as f64 - min as f64;
    Ok(pixels
        .iter()
        .map(|&depth| ((depth as f64 - min as f64) / span).clamp(0.0, 1.0) as f32)
        .collect())
}

/// Normalized depth (near 0, far 1) enlarged to the photo from a prediction
/// `model_width` pixels across, without the letterbox.
struct RestoredDepth {
    model_width: u32,
    depth: Vec<f32>,
}

impl RestoredDepth {
    /// Aligns the depth's edges with `photo`, the image it was predicted from
    /// (`depth_refine`).
    fn align_edges(&mut self, photo: &ImageBuffer<Rgba<u8>, Vec<u8>>) -> Result<()> {
        let (width, height) = photo.dimensions();
        depth_refine::align_depth_edges(
            &mut self.depth,
            photo.as_raw(),
            width,
            height,
            self.model_width,
        )
    }
}

fn restore_depth(
    raw: &[f32],
    output_width: u32,
    output_height: u32,
    box_rect: Letterbox,
    target_width: u32,
    target_height: u32,
) -> Result<RestoredDepth> {
    let image = ImageBuffer::<Luma<f32>, _>::from_raw(output_width, output_height, raw.to_vec())
        .context("invalid depth image")?;
    let (x0, y0, x1, y1) = box_rect.output_rect(output_width, output_height);
    let cropped = image::imageops::crop_imm(&image, x0, y0, x1 - x0, y1 - y0).to_image();
    let pixels = cropped.into_raw();
    // Remove padding before estimating the range, and normalize before resize
    // (image::resize clamps floating-point Luma samples to [0, 1]).
    let normalized = normalize_depth(&pixels)?;
    // The model predicts inverse depth: high values are near. Invert only
    // after robust normalization to preserve near=0, far=255.
    let normalized: Vec<f32> = normalized.into_iter().map(|value| 1.0 - value).collect();
    let model = ImageBuffer::<Luma<f32>, _>::from_raw(x1 - x0, y1 - y0, normalized)
        .context("invalid cropped depth image")?;
    let depth = image::imageops::resize(&model, target_width, target_height, FilterType::Triangle)
        .into_raw();
    Ok(RestoredDepth {
        model_width: model.width(),
        depth,
    })
}

fn quantize_depth(depth: &[f32]) -> Vec<u8> {
    depth
        .iter()
        .map(|depth| (depth.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect()
}
