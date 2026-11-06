//! Scene depth (`SceneDepth`): disparity from Distill-Any-Depth Small at the
//! model's own size, and the normals, sky and camera from MoGe-2
//! (`scene_geometry`).
//!
//! Distill's silhouettes and thin structures are sharper, so it gives the
//! disparity. MoGe-2's points lie in a pinhole camera: fitted against them,
//! the disparity becomes distance, and without the photo's focal length they
//! give the field of view. Nothing is resampled, clipped or filtered: each
//! part keeps the size and precision its model predicted it at, and effects
//! read them in full-image coordinates.

use super::masks::mask_model_retention;
use super::scene_geometry::{self, SceneGeometry, SceneGeometryModel};
use super::{fitted_input_size, normalized_rgb_input};
use crate::config::{MemoryClass, CAPABILITIES};
use crate::placement::SessionOptions;
use crate::registry::{AiModel, InputShape, ModelInfo};
use crate::slot::with_model_session;
use anyhow::{Context, Result};
use calibraw_core::pipeline::{DisparityMap, MaskImage, SceneCamera, SceneDepth, SceneNormals};
use image::{imageops::FilterType, ImageBuffer, Rgba};
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
/// Sky probability from which a pixel is sky, not a surface the camera is
/// fitted to.
const SKY: f32 = 0.5;
/// Share of the surface samples beyond each anchor, so that a few stray
/// predictions do not set the scene's depth range.
const ANCHOR_TAIL: f64 = 0.005;
/// Grid step, in disparity pixels, of the samples the camera is fitted to.
const FIT_STEP: usize = 2;
/// Fewest surface samples a camera is fitted to.
const MIN_FIT_SAMPLES: usize = 64;
/// Reweighting rounds of the distance fit.
const FIT_ITERATIONS: usize = 20;
/// Huber threshold in robust standard deviations of the residuals: 95%
/// efficiency for normal residuals, while edges where the two models
/// disagree weigh in only linearly.
const HUBER_K: f64 = 1.345;
/// Half the diagonal of the 35 mm frame, mm.
const FULL_FRAME_HALF_DIAGONAL: f32 = 21.633;

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

/// The depth result of `image`, the whole photo. `focal_length_35mm` is the
/// photo's 35 mm-equivalent focal length when known; otherwise MoGe-2's
/// points give the field of view.
pub(super) fn scene_depth(
    model_path: &Path,
    geometry_model: &SceneGeometryModel,
    image: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    focal_length_35mm: Option<f32>,
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
    let disparity = normalized_disparity(&raw, output_width, output_height)?;
    let geometry = scene_geometry::scene_geometry(geometry_model, image)?;
    let focal_tan = match focal_length_35mm.filter(|focal| focal.is_finite() && *focal > 0.0) {
        Some(focal) => {
            FULL_FRAME_HALF_DIAGONAL * width.min(height) as f32
                / (width as f32).hypot(height as f32)
                / focal
        }
        None => point_focal_tan(&geometry)?,
    };
    let camera = fitted_camera(&disparity, &geometry, focal_tan)?;
    SceneDepth::new(
        disparity,
        sky_image(&geometry)?,
        encode_normals(&geometry)?,
        camera,
    )
    .context("the depth result has no valid camera")
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

/// The model's output, relative disparity (high for near), normalized by its
/// own minimum and maximum.
fn normalized_disparity(raw: &[f32], width: u32, height: u32) -> Result<DisparityMap> {
    let (min, max) = raw
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), &value| {
            (min.min(value), max.max(value))
        });
    anyhow::ensure!(
        max > min,
        "depth model returned a constant depth map; no depth range could be detected"
    );
    // f64, so the span cannot overflow.
    let span = f64::from(max) - f64::from(min);
    let values = raw
        .iter()
        .map(|&value| DisparityMap::encode(((f64::from(value) - f64::from(min)) / span) as f32))
        .collect();
    DisparityMap::new(width, height, values).context("invalid depth output size")
}

/// `geometry`'s sky probability as stored, at the normal model's size.
fn sky_image(geometry: &SceneGeometry) -> Result<MaskImage> {
    let pixels = geometry
        .sky
        .iter()
        .map(|sky| (sky.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    MaskImage::new(geometry.width, geometry.height, pixels).context("invalid sky image")
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

/// A disparity at a surface and the inverse distance MoGe-2 measured there.
#[derive(Clone, Copy)]
struct FitSample {
    disparity: f64,
    inverse_distance: f64,
}

/// The camera of `disparity`: its distance fitted to `geometry`'s points
/// where neither is sky, its anchors at the nearest and farthest of those
/// samples (`ANCHOR_TAIL`), and the true frame, `frame_relief = Q − 1`.
fn fitted_camera(
    disparity: &DisparityMap,
    geometry: &SceneGeometry,
    focal_tan: f32,
) -> Result<SceneCamera> {
    let samples = fit_samples(disparity, geometry);
    anyhow::ensure!(
        samples.len() >= MIN_FIT_SAMPLES,
        "the photo shows too little besides sky to measure its depth"
    );
    let mut disparities: Vec<f64> = samples.iter().map(|sample| sample.disparity).collect();
    let far = quantile(&mut disparities, ANCHOR_TAIL);
    let near = quantile(&mut disparities, 1.0 - ANCHOR_TAIL);
    anyhow::ensure!(
        near > far,
        "depth model returned a constant depth map; no depth range could be detected"
    );
    let candidate = |fit: [f64; 2]| {
        let camera = SceneCamera {
            focal_tan,
            fit: fit.map(|value| value as f32),
            anchors: [near as f32, far as f32],
            frame_relief: 1.0,
        };
        let range = f64::from(camera.distance(far as f32) / camera.distance(near as f32));
        let camera = SceneCamera {
            frame_relief: (range - 1.0) as f32,
            ..camera
        };
        camera.is_valid().then_some(camera)
    };
    huber_fit(&samples)
        .and_then(candidate)
        .or_else(|| candidate(range_fit(&samples, [near, far])?))
        .context("the depth and normal models disagree on the photo's depth")
}

/// Samples on a `FIT_STEP` grid of `disparity` where `geometry` has a point
/// in front of the camera and no sky, all of it bilinear at the sample.
fn fit_samples(disparity: &DisparityMap, geometry: &SceneGeometry) -> Vec<FitSample> {
    let (width, height) = (disparity.width() as usize, disparity.height() as usize);
    let (geometry_width, geometry_height) = (geometry.width as usize, geometry.height as usize);
    let scale_x = geometry.width as f32 / disparity.width() as f32;
    let scale_y = geometry.height as f32 / disparity.height() as f32;
    let axis = |position: usize, scale: f32, extent: usize| {
        let p = ((position as f32 + 0.5) * scale - 0.5).clamp(0.0, (extent - 1) as f32);
        let low = p.floor() as usize;
        (low, (low + 1).min(extent - 1), p - low as f32)
    };
    let mut samples = Vec::new();
    for y in (0..height).step_by(FIT_STEP) {
        let (y0, y1, fy) = axis(y, scale_y, geometry_height);
        for x in (0..width).step_by(FIT_STEP) {
            let (x0, x1, fx) = axis(x, scale_x, geometry_width);
            let taps =
                [(x0, y0), (x1, y0), (x0, y1), (x1, y1)].map(|(x, y)| y * geometry_width + x);
            let weights = [
                (1.0 - fx) * (1.0 - fy),
                fx * (1.0 - fy),
                (1.0 - fx) * fy,
                fx * fy,
            ];
            let bilinear = |value: &dyn Fn(usize) -> f32| {
                taps.iter()
                    .zip(weights)
                    .map(|(&tap, weight)| value(tap) * weight)
                    .sum::<f32>()
            };
            if bilinear(&|tap| geometry.sky[tap]) >= SKY {
                continue;
            }
            let depths = taps.map(|tap| geometry.points[tap][2]);
            if depths.iter().any(|z| !z.is_finite() || *z <= 0.0) {
                continue;
            }
            samples.push(FitSample {
                disparity: f64::from(DisparityMap::decode(disparity.values()[y * width + x])),
                inverse_distance: f64::from(bilinear(&|tap| 1.0 / geometry.points[tap][2])),
            });
        }
    }
    samples
}

/// `(s, t)` of `inverse_distance ≈ s · disparity + t` by iteratively
/// reweighted least squares with Huber weights. `None` when the samples do not
/// determine a line.
fn huber_fit(samples: &[FitSample]) -> Option<[f64; 2]> {
    let mut weights = vec![1.0; samples.len()];
    let mut fit = None;
    let mut residuals = vec![0.0; samples.len()];
    for _ in 0..FIT_ITERATIONS {
        let line = weighted_line(samples, &weights)?;
        for (residual, sample) in residuals.iter_mut().zip(samples) {
            *residual = (sample.inverse_distance - line[0] * sample.disparity - line[1]).abs();
        }
        let converged = fit.is_some_and(|previous: [f64; 2]| {
            (previous[0] - line[0]).abs() <= 1e-9 * line[0].abs().max(1e-12)
                && (previous[1] - line[1]).abs() <= 1e-9 * line[1].abs().max(1e-12)
        });
        fit = Some(line);
        if converged {
            break;
        }
        // Median absolute deviation as a robust standard deviation.
        let mut sorted = residuals.clone();
        let deviation = 1.4826 * quantile(&mut sorted, 0.5);
        if deviation <= 0.0 {
            break;
        }
        let threshold = HUBER_K * deviation;
        for (weight, &residual) in weights.iter_mut().zip(&residuals) {
            *weight = if residual <= threshold {
                1.0
            } else {
                threshold / residual
            };
        }
    }
    fit
}

/// The weighted least-squares line through the samples.
fn weighted_line(samples: &[FitSample], weights: &[f64]) -> Option<[f64; 2]> {
    let (mut w, mut x, mut y, mut xx, mut xy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (sample, &weight) in samples.iter().zip(weights) {
        w += weight;
        x += weight * sample.disparity;
        y += weight * sample.inverse_distance;
        xx += weight * sample.disparity * sample.disparity;
        xy += weight * sample.disparity * sample.inverse_distance;
    }
    let determinant = w * xx - x * x;
    if !determinant.is_finite() || determinant.abs() <= f64::EPSILON * w * xx {
        return None;
    }
    let slope = (w * xy - x * y) / determinant;
    Some([slope, (y - slope * x) / w])
}

/// `(s, t)` that maps the anchors `[near, far]` onto the same shares of the
/// measured inverse distances: what remains when the fit fails.
fn range_fit(samples: &[FitSample], [near, far]: [f64; 2]) -> Option<[f64; 2]> {
    let mut inverse: Vec<f64> = samples
        .iter()
        .map(|sample| sample.inverse_distance)
        .collect();
    let inverse_far = quantile(&mut inverse, ANCHOR_TAIL);
    let inverse_near = quantile(&mut inverse, 1.0 - ANCHOR_TAIL);
    let slope = (inverse_near - inverse_far) / (near - far);
    (slope > 0.0 && inverse_far > 0.0).then_some([slope, inverse_far - slope * far])
}

/// The value at `share` of `values` from the smallest, reordering them.
fn quantile(values: &mut [f64], share: f64) -> f64 {
    let index = ((values.len() - 1) as f64 * share).round() as usize;
    *values.select_nth_unstable_by(index, f64::total_cmp).1
}

/// Tangent of half the field of view across the shorter edge of MoGe-2's
/// camera: the least-squares `k` of `(x, y) = k · (u, v) · z` over its points
/// off the sky, with `(u, v)` the pixel's offset from the centre in
/// shorter-edge units, is twice it.
fn point_focal_tan(geometry: &SceneGeometry) -> Result<f32> {
    let (width, height) = (geometry.width as usize, geometry.height as usize);
    let shorter = width.min(height) as f64;
    let (mut numerator, mut denominator) = (0.0, 0.0);
    for (index, (point, &sky)) in geometry.points.iter().zip(&geometry.sky).enumerate() {
        if sky >= SKY || point.iter().any(|value| !value.is_finite()) || point[2] <= 0.0 {
            continue;
        }
        let u = ((index % width) as f64 + 0.5 - 0.5 * width as f64) / shorter;
        let v = ((index / width) as f64 + 0.5 - 0.5 * height as f64) / shorter;
        let [x, y, z] = point.map(f64::from);
        numerator += x * u * z + y * v * z;
        denominator += (u * z).powi(2) + (v * z).powi(2);
    }
    let focal_tan = (0.5 * numerator / denominator) as f32;
    anyhow::ensure!(
        focal_tan.is_finite() && focal_tan > 0.0,
        "the normal model gave no field of view"
    );
    Ok(focal_tan)
}
