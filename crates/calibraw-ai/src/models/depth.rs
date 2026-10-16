//! Scene depth from Depth Anything V2 Small, aligned to the photo's edges.

use super::masks::mask_model_retention;
use super::{depth_refine, normalized_rgb_input};
use crate::config::{MemoryClass, CAPABILITIES};
use crate::placement::{CpuFallbackProfile, SessionOptions};
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
/// The depth model desktop builds downloaded before `MODEL`, in the model
/// cache.
///
/// migration: remove in v1.4.0. Listed in `calibraw_core`'s `migrations`.
pub const RETIRED_DEPTH_MODEL_FILENAME: &str = "da3mono_large_700x700.onnx";
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
        // Depth runs on the CPU beside the RAW preview.
        MemoryClass::Constrained => options
            .cpu_only()
            .with_cpu_fallback_profile(CpuFallbackProfile::LowMemory),
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

#[cfg(test)]
mod tests {
    use super::*;

    const EDGE: u32 = INPUT_EDGE;

    /// The enlarged depth as stored, without edge alignment.
    fn restored_bytes(
        raw: &[f32],
        output_width: u32,
        output_height: u32,
        box_rect: Letterbox,
        target_width: u32,
        target_height: u32,
    ) -> Vec<u8> {
        let restored = restore_depth(
            raw,
            output_width,
            output_height,
            box_rect,
            target_width,
            target_height,
        )
        .unwrap();
        quantize_depth(&restored.depth)
    }

    #[test]
    #[ignore = "requires CALIBRAW_DEPTH_MODEL and ONNX Runtime via ORT_DYLIB_PATH"]
    fn published_model_produces_depth() {
        let path = std::env::var_os("CALIBRAW_DEPTH_MODEL").expect("CALIBRAW_DEPTH_MODEL");
        let runtime = std::env::var_os("ORT_DYLIB_PATH").expect("ORT_DYLIB_PATH");
        ort::init_from(Path::new(&runtime)).unwrap().commit();
        if std::env::var("CALIBRAW_TEST_DEPTH_PROVIDER").as_deref() == Ok("CPU") {
            crate::update(crate::AiConfigChange::Acceleration(false));
        }
        assert!(depth_model_is_verified(Path::new(&path)));
        let image = ImageBuffer::from_fn(96, 64, |x, y| {
            Rgba([(x * 255 / 95) as u8, (y * 255 / 63) as u8, 128, 255])
        });
        let mask = depth_mask(Path::new(&path), &image).unwrap();
        assert_eq!(mask.len(), 96 * 64);
        assert!(mask.iter().max().unwrap() - mask.iter().min().unwrap() > 32);
        let providers = crate::active_execution_providers();
        eprintln!("Depth inference providers: {providers:?}");
        if let Ok(expected) = std::env::var("CALIBRAW_TEST_DEPTH_PROVIDER") {
            assert!(providers.iter().any(|status| {
                status.model_name == MODEL.name
                    && status.active_provider == expected
                    && !status.degraded
            }));
        }
    }

    #[test]
    fn only_constrained_memory_runs_depth_on_a_low_memory_cpu_session() {
        let constrained = session_options_for(MemoryClass::Constrained);
        assert_eq!(constrained.model_name, MODEL.name);
        assert!(!constrained.allow_acceleration);
        assert_eq!(
            constrained.cpu_fallback_profile,
            CpuFallbackProfile::LowMemory
        );
        let standard = session_options_for(MemoryClass::Standard);
        assert!(standard.allow_acceleration);
        assert_eq!(standard.cpu_fallback_profile, CpuFallbackProfile::Default);
    }

    #[test]
    fn rejects_invalid_depth_outputs() {
        assert_eq!(validate_output(&[1, 2, 3], &[1.0; 6]).unwrap(), (3, 2));
        assert!(validate_output(&[1, 1, 2, 3], &[1.0; 6]).is_err());
        for shape in [[2, 2, 3], [1, 0, 3], [1, -1, 3], [1, 519, 3]] {
            assert!(validate_output(&shape, &[1.0; 6]).is_err());
        }
        assert!(validate_output(&[1, 2, 3], &[1.0; 5]).is_err());
        assert!(validate_output(&[1, 1, 1], &[f32::NAN]).is_err());
        assert!(validate_output(&[1, 1, 1], &[f32::INFINITY]).is_err());
    }

    #[test]
    fn inverse_depth_normalizes_before_resize_and_maps_near_to_zero() {
        let rect = Letterbox::new(3, 3, 518);
        let raw = [100.0, 50.0, 0.0, 100.0, 50.0, 0.0, 100.0, 50.0, 0.0];
        assert_eq!(
            restored_bytes(&raw, 3, 3, rect, 3, 3),
            [0, 128, 255, 0, 128, 255, 0, 128, 255]
        );
        let scaled: Vec<_> = raw.iter().map(|v| v * 1e-9).collect();
        assert_eq!(
            restored_bytes(&scaled, 3, 3, rect, 3, 3),
            [0, 128, 255, 0, 128, 255, 0, 128, 255]
        );
    }

    #[test]
    fn unpadding_preserves_landscape_portrait_and_thin_geometry() {
        let edge = EDGE;
        assert_eq!(
            Letterbox::new(4, 2, edge).output_rect(edge, edge),
            (0, 129, 518, 388)
        );
        assert_eq!(
            Letterbox::new(2, 4, edge).output_rect(edge, edge),
            (129, 0, 388, 518)
        );
        assert_eq!(
            Letterbox::new(1, 10000, edge).output_rect(1, 1),
            (0, 0, 1, 1)
        );
        for (width, height) in [(4, 2), (2, 4)] {
            let rect = Letterbox::new(width, height, edge);
            let mut raw = vec![-10000.0; (edge * edge) as usize];
            for y in rect.top..rect.top + rect.height {
                for x in rect.left..rect.left + rect.width {
                    raw[(y * edge + x) as usize] = if width > height { x } else { y } as f32;
                }
            }
            let mask = restored_bytes(&raw, edge, edge, rect, width, height);
            assert_eq!(mask.len(), (width * height) as usize);
            assert!(mask[0] > 191);
            assert!(mask[mask.len() - 1] < 64);
            if width > height {
                assert_eq!(&mask[..4], &mask[4..]);
            } else {
                assert!(mask.chunks_exact(2).all(|row| row[0] == row[1]));
            }
        }
    }

    #[test]
    fn letterbox_preserves_landscape_and_portrait_geometry() {
        assert_eq!(
            Letterbox::new(1400, 700, 700),
            Letterbox {
                width: 700,
                height: 350,
                left: 0,
                top: 175,
                edge: 700,
            }
        );
        assert_eq!(
            Letterbox::new(700, 1400, 700),
            Letterbox {
                width: 350,
                height: 700,
                left: 175,
                top: 0,
                edge: 700,
            }
        );
        assert_eq!(
            Letterbox::new(1400, 700, 700).output_rect(350, 350),
            (0, 88, 350, 263)
        );
        assert_eq!(
            Letterbox::new(700, 1400, 700).output_rect(350, 350),
            (88, 0, 263, 350)
        );
    }

    #[test]
    fn unpadding_discards_extreme_padding_values() {
        let rectangle = Letterbox::new(4, 2, EDGE);
        let mut raw = vec![1000.0; (EDGE * EDGE) as usize];
        for y in rectangle.top..rectangle.top + rectangle.height {
            for x in 0..EDGE {
                raw[(y * EDGE + x) as usize] = x as f32;
            }
        }
        let result = restored_bytes(&raw, EDGE, EDGE, rectangle, 4, 2);
        assert_eq!(result.len(), 8);
        assert!(result[0] > result[3]);
        assert_eq!(result[0], result[4]);
    }

    #[test]
    fn normalization_preserves_scene_contrast_despite_outliers() {
        let mut pixels: Vec<_> = (0..1000).map(|i| 10.0 + i as f32 / 100.0).collect();
        pixels[0] = -10000.0;
        pixels[999] = 10000.0;
        let normalized = normalize_depth(&pixels).unwrap();
        assert_eq!(normalized[0], 0.0);
        assert_eq!(normalized[999], 1.0);
        assert!(normalized[750] - normalized[250] > 0.5);
        assert!(normalized.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn normalization_is_scale_invariant_and_retains_small_subjects() {
        assert_eq!(
            normalize_depth(&[0.0, 1e-9, 2e-9]).unwrap(),
            [0.0, 0.5, 1.0]
        );
        let mut pixels = vec![1.0; 1000];
        pixels[0] = 2.0;
        let normalized = normalize_depth(&pixels).unwrap();
        assert_eq!(normalized[0], 1.0);
        assert_eq!(normalized[1], 0.0);
    }

    #[test]
    fn unusable_maps_report_an_error_instead_of_selecting_everything() {
        for pixels in [
            &[][..],
            &[5.0, 5.0],
            &[0.0, f32::INFINITY],
            &[0.0, f32::NAN],
        ] {
            assert!(normalize_depth(pixels).is_err());
        }
    }

    #[test]
    fn restoring_portrait_depth_keeps_near_and_far_aligned() {
        let rectangle = Letterbox::new(2, 4, EDGE);
        let mut raw = vec![-10000.0; (EDGE * EDGE) as usize];
        for y in 0..EDGE {
            for x in rectangle.left..rectangle.left + rectangle.width {
                raw[(y * EDGE + x) as usize] = y as f32;
            }
        }
        let result = restored_bytes(&raw, EDGE, EDGE, rectangle, 2, 4);
        assert_eq!(result.len(), 8);
        assert!(result[0] > 191);
        assert!(result[6] < 64);
        assert!(result.chunks_exact(2).all(|row| row[0] == row[1]));
        assert_eq!(
            Letterbox::new(1, 10000, EDGE).output_rect(1, 1),
            (0, 0, 1, 1)
        );
    }
}
