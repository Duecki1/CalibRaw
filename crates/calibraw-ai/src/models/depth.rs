//! Scene depth from Distill-Any-Depth Small, with the sky at the farthest
//! distance and surface normals from MoGe-2 (`scene_geometry`), aligned to the
//! photo's edges.

use super::masks::mask_model_retention;
use super::scene_geometry::{self, SceneGeometry, SceneGeometryModel};
use super::{depth_refine, fitted_input_size, normalized_rgb_input};
use crate::config::{MemoryClass, CAPABILITIES};
use crate::placement::SessionOptions;
use crate::registry::{AiModel, InputShape, ModelInfo};
use crate::slot::with_model_session;
use anyhow::{Context, Result};
use calibraw_core::pipeline::{MaskImage, SceneNormals};
use image::{imageops::FilterType, ImageBuffer, Luma, Rgba};
use ort::value::Tensor;
use std::path::Path;

pub const MODEL: ModelInfo = AiModel::Depth.spec().info();
const INPUT_FIT: (u32, u32) = match MODEL.input {
    InputShape::Fitted { edge, multiple } => (edge, multiple),
    InputShape::Square { .. }
    | InputShape::Tiled { .. }
    | InputShape::Prompted { .. }
    | InputShape::Stretched { .. } => panic!("the depth model keeps the photo's aspect ratio"),
};
/// The model's input: `INPUT_EDGE` pixels on the photo's longer side, each
/// side a multiple of `INPUT_MULTIPLE`, its patch size.
const INPUT_EDGE: u32 = INPUT_FIT.0;
const INPUT_MULTIPLE: u32 = INPUT_FIT.1;
/// Normalized depth over which a pixel the normal model finds sky becomes
/// far enough to be sky: below it, wires, branches and other near detail
/// that the normal model's coarser mask covers keep their depth.
const SKY_FAR: (f32, f32) = (0.85, 0.92);
/// Sky probability over which a pixel turns into sky.
const SKY_PROBABILITY: (f32, f32) = (0.4, 0.6);

/// Scene depth (8 bits, near 0, far and sky 255) at the photo's size, and the
/// normals predicted with it at the normal model's size.
pub(super) struct SceneDepth {
    pub(super) depth: Vec<u8>,
    pub(super) normals: SceneNormals,
}

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

/// The model's input size, width and height, for a `width` × `height` photo.
/// A 3:2 photo is 1540 × 1036.
fn input_size(width: u32, height: u32) -> (u32, u32) {
    fitted_input_size(INPUT_EDGE, INPUT_MULTIPLE, width, height)
}

pub(super) fn depth_mask(
    model_path: &Path,
    geometry_model: &SceneGeometryModel,
    image: &ImageBuffer<Rgba<u8>, Vec<u8>>,
) -> Result<SceneDepth> {
    let (width, height) = image.dimensions();
    anyhow::ensure!(width > 0 && height > 0, "empty depth-mask input");
    let (input_width, input_height) = input_size(width, height);
    let resized = image::imageops::resize(image, input_width, input_height, FilterType::Lanczos3);

    let pixels = normalized_rgb_input(&resized)?;
    let input = Tensor::from_array((
        [1usize, 3, input_height as usize, input_width as usize],
        pixels,
    ))
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
    let (output_width, output_height) = validate_output(&shape, &raw, input_width, input_height)?;
    calibraw_core::diagnostics::record(format!(
        "AI {}: {width}x{height} source, {output_width}x{output_height} depth in {:.3}s",
        MODEL.name,
        started.elapsed().as_secs_f64()
    ));
    let mut restored = restore_depth(&raw, output_width, output_height, width, height)?;
    let geometry = scene_geometry::scene_geometry(geometry_model, image)?;
    let normals = encode_normals(&geometry)?;
    restored.make_sky_farthest(geometry, width, height)?;
    let started = std::time::Instant::now();
    restored.align_edges(image)?;
    calibraw_core::diagnostics::record(format!(
        "AI depth edges aligned to the {width}x{height} photo in {:.3}s",
        started.elapsed().as_secs_f64()
    ));
    Ok(SceneDepth {
        depth: quantize_depth(&restored.depth),
        normals,
    })
}

/// `geometry`'s normals as stored (`SceneNormals`).
fn encode_normals(geometry: &SceneGeometry) -> Result<SceneNormals> {
    let component = |axis: usize| {
        let pixels = geometry
            .normals
            .iter()
            .map(|normal| SceneNormals::encode(normal[axis]))
            .collect();
        MaskImage::new(geometry.width, geometry.height, pixels).context("invalid normal image")
    };
    SceneNormals::new(component(0)?, component(1)?).context("normal components differ in size")
}

fn smoothstep(edges: (f32, f32), value: f32) -> f32 {
    let t = ((value - edges.0) / (edges.1 - edges.0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The output's width and height, which are the input's.
fn validate_output(
    shape: &[i64],
    values: &[f32],
    input_width: u32,
    input_height: u32,
) -> Result<(u32, u32)> {
    let (height, width) = match shape {
        [1, h, w] => (*h, *w),
        _ => anyhow::bail!("unexpected depth output shape {shape:?}"),
    };
    anyhow::ensure!(
        height == i64::from(input_height) && width == i64::from(input_width),
        "depth output {shape:?} does not match the {input_width}x{input_height} input"
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
/// `model_width` pixels across.
struct RestoredDepth {
    model_width: u32,
    depth: Vec<f32>,
}

impl RestoredDepth {
    /// Moves what `geometry` finds sky, and the depth model agrees is far, to
    /// the farthest depth; the depth is `width` × `height`. Before
    /// `align_edges`, which then moves the sky's outline onto the photo's.
    fn make_sky_farthest(
        &mut self,
        geometry: SceneGeometry,
        width: u32,
        height: u32,
    ) -> Result<()> {
        let sky =
            ImageBuffer::<Luma<f32>, _>::from_raw(geometry.width, geometry.height, geometry.sky)
                .context("invalid sky mask")?;
        let sky = image::imageops::resize(&sky, width, height, FilterType::Triangle).into_raw();
        anyhow::ensure!(sky.len() == self.depth.len(), "sky mask size mismatch");
        for (depth, sky) in self.depth.iter_mut().zip(sky) {
            let weight = smoothstep(SKY_PROBABILITY, sky) * smoothstep(SKY_FAR, *depth);
            *depth += (1.0 - *depth) * weight;
        }
        Ok(())
    }

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
    target_width: u32,
    target_height: u32,
) -> Result<RestoredDepth> {
    // Normalize before resize (image::resize clamps floating-point Luma
    // samples to [0, 1]).
    let normalized = normalize_depth(raw)?;
    // The model predicts inverse depth: high values are near. Invert only
    // after robust normalization to preserve near=0, far=255.
    let normalized: Vec<f32> = normalized.into_iter().map(|value| 1.0 - value).collect();
    let model = ImageBuffer::<Luma<f32>, _>::from_raw(output_width, output_height, normalized)
        .context("invalid depth image")?;
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
