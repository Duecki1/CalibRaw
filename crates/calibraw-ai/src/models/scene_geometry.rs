//! Surface normals, sky and camera-space points from MoGe-2, predicted beside
//! the scene depth (`depth`).
//!
//! The depth model's silhouettes and thin structures are sharper, so it keeps
//! the depth; MoGe-2 adds what depth cannot give well: normals, which taken
//! from relative depth tilt the ground and stripe slopes, a sky mask, and
//! points in its camera, which measure distance and the field of view.

use super::fitted_input_size;
use super::masks::mask_model_retention;
use crate::config::{MemoryClass, CAPABILITIES};
use crate::placement::SessionOptions;
use crate::registry::{AiModel, InputShape, ModelInfo};
use crate::slot::with_model_session;
use anyhow::{Context, Result};
use image::{imageops::FilterType, ImageBuffer, Rgba};
use ort::value::Tensor;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Which MoGe-2 model estimates normals and sky for scene depth. Serialized in
/// the performance settings; keep the names stable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneDepthQuality {
    /// MoGe-2 ViT-S at 2400 patches.
    #[default]
    Low,
    /// MoGe-2 ViT-L at 3600 patches, a 1.3 GB download. Needs
    /// `MemoryClass::Standard`.
    Medium,
}

impl SceneDepthQuality {
    /// The qualities this device offers.
    pub const AVAILABLE: &'static [Self] = Self::available(CAPABILITIES.memory);

    const fn available(memory: MemoryClass) -> &'static [Self] {
        match memory {
            MemoryClass::Constrained => &[Self::Low],
            MemoryClass::Standard => &[Self::Low, Self::Medium],
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
        }
    }

    /// The normal and sky model.
    pub const fn model(self) -> ModelInfo {
        self.ai_model().spec().info()
    }

    pub(super) const fn ai_model(self) -> AiModel {
        match self {
            Self::Low => AiModel::SceneGeometryLow,
            Self::Medium => AiModel::SceneGeometryMedium,
        }
    }

    /// Patches the model resizes the image to: its resolution and cost.
    const fn tokens(self) -> i64 {
        match self {
            Self::Low => 2400,
            Self::Medium => 3600,
        }
    }

    const fn session_options(self, memory: MemoryClass) -> SessionOptions {
        let options = self.ai_model().spec().session;
        match memory {
            // As the depth model: on the CPU beside the RAW preview.
            MemoryClass::Constrained => options.cpu_only(),
            MemoryClass::Standard => options,
        }
    }
}

/// The normal model that runs beside the depth model, and where it is.
#[derive(Clone, Debug)]
pub struct SceneGeometryModel {
    pub path: PathBuf,
    pub quality: SceneDepthQuality,
}

impl SceneGeometryModel {
    pub fn is_verified(&self) -> bool {
        self.quality.ai_model().spec().is_installed(&self.path)
    }
}

/// The model's normals, sky and points at its input size, the photo scaled to
/// the model's `InputShape::Fitted` edge on its longer side.
pub(super) struct SceneGeometry {
    pub(super) width: u32,
    pub(super) height: u32,
    /// Unit normals per pixel by row, in the camera frame of
    /// `calibraw_core::pipeline::SceneNormals`.
    pub(super) normals: Vec<[f32; 3]>,
    /// Probability that a pixel is sky, per pixel by row.
    pub(super) sky: Vec<f32>,
    /// The point each pixel sees, by row, in MoGe-2's pinhole camera with
    /// square pixels: x right, y down, z along the optical axis, in a unit of
    /// its own. Not finite where the model has none, such as in the sky.
    pub(super) points: Vec<[f32; 3]>,
}

/// Runs `geometry_model` over `image`.
pub(super) fn scene_geometry(
    geometry_model: &SceneGeometryModel,
    image: &ImageBuffer<Rgba<u8>, Vec<u8>>,
) -> Result<SceneGeometry> {
    let quality = geometry_model.quality;
    let (width, height) = image.dimensions();
    anyhow::ensure!(width > 0 && height > 0, "empty normal-model input");
    let model = quality.ai_model();
    let InputShape::Fitted { edge, multiple } = model.spec().input else {
        anyhow::bail!("the normal model keeps the photo's aspect ratio");
    };
    let (input_width, input_height) = fitted_input_size(edge, multiple, width, height);
    let resized = image::imageops::resize(image, input_width, input_height, FilterType::Lanczos3);
    let plane = resized.pixels().len();
    let mut pixels = vec![0.0; plane * 3];
    for (index, pixel) in resized.pixels().enumerate() {
        for channel in 0..3 {
            pixels[channel * plane + index] = f32::from(pixel[channel]) / 255.0;
        }
    }
    let input = Tensor::from_array((
        [1usize, 3, input_height as usize, input_width as usize],
        pixels,
    ))
    .context("create normal-model input tensor")?;
    let tokens = Tensor::from_array(((), vec![quality.tokens()]))
        .context("create normal-model token count")?;
    let started = std::time::Instant::now();
    let (normal_shape, normals, mask_shape, mask, point_shape, points) = with_model_session(
        model,
        &geometry_model.path,
        quality.session_options(CAPABILITIES.memory),
        mask_model_retention(true),
        |session| {
            session.run_with_fallback("normal-model ONNX inference", |ort_session, _| {
                let outputs = ort_session
                    .run(ort::inputs!["image" => &input, "num_tokens" => &tokens])
                    .context("run normal-model inference")?;
                let (normal_shape, normals) = outputs
                    .get("normal")
                    .context("normal model returned no normals")?
                    .try_extract_tensor::<f32>()
                    .context("read normal tensor")?;
                let (mask_shape, mask) = outputs
                    .get("mask")
                    .context("normal model returned no sky mask")?
                    .try_extract_tensor::<f32>()
                    .context("read sky-mask tensor")?;
                let (point_shape, points) = outputs
                    .get("points")
                    .context("normal model returned no points")?
                    .try_extract_tensor::<f32>()
                    .context("read point tensor")?;
                Ok((
                    normal_shape.as_ref().to_vec(),
                    normals.to_vec(),
                    mask_shape.as_ref().to_vec(),
                    mask.to_vec(),
                    point_shape.as_ref().to_vec(),
                    points.to_vec(),
                ))
            })
        },
    )?;
    let expected_normals = [1, i64::from(input_height), i64::from(input_width), 3];
    let expected_mask = [1, i64::from(input_height), i64::from(input_width)];
    anyhow::ensure!(
        normal_shape == expected_normals
            && point_shape == expected_normals
            && mask_shape == expected_mask,
        "normal-model outputs {normal_shape:?}, {point_shape:?} and {mask_shape:?} do not \
         match the {input_width}x{input_height} input"
    );
    calibraw_core::diagnostics::record(format!(
        "AI {}: {width}x{height} source, {input_width}x{input_height} normals in {:.3}s",
        model.spec().artifact.name,
        started.elapsed().as_secs_f64()
    ));
    let sky = mask
        .iter()
        .map(|&geometry| {
            anyhow::ensure!(
                geometry.is_finite(),
                "normal model returned a non-finite sky mask"
            );
            Ok(1.0 - geometry.clamp(0.0, 1.0))
        })
        .collect::<Result<Vec<_>>>()?;
    let normals = normals
        .chunks_exact(3)
        .zip(&sky)
        .map(|(normal, &sky)| camera_facing_normal([normal[0], normal[1], normal[2]], sky))
        .collect();
    let points = points
        .chunks_exact(3)
        .map(|point| [point[0], point[1], point[2]])
        .collect();
    Ok(SceneGeometry {
        width: input_width,
        height: input_height,
        normals,
        sky,
        points,
    })
}

/// `normal` made unit length. Sky, and anything the model could not orient,
/// faces the camera straight on. A surface seen nearly edge-on, such as the
/// ground toward the horizon, may point slightly away from the camera (z > 0);
/// flipping it would turn the ground upside down, so it is kept as is.
fn camera_facing_normal(normal: [f32; 3], sky: f32) -> [f32; 3] {
    const TOWARD_CAMERA: [f32; 3] = [0.0, 0.0, -1.0];
    let length = normal.iter().map(|value| value * value).sum::<f32>().sqrt();
    if sky >= 0.5 || !length.is_finite() || length < 1e-6 {
        return TOWARD_CAMERA;
    }
    normal.map(|value| value / length)
}
