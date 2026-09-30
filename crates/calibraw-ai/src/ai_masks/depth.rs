use super::*;

const EDGE: u32 = 700;

pub(super) const MODEL_FILENAME: &str = "da3mono_large_700x700.onnx";
const MODEL_BYTES: u64 = 731_358_963;
pub(super) const MODEL_INSTALL: ModelInstallSpec = ModelInstallSpec {
    artifact: ModelArtifact {
        name: "Depth Anything 3 Mono Large",
        url: Some("https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/4b82010fd8654fc3a1c33311ff305d793ec2f511/models/da3/da3mono_large_700x700.onnx"),
        sha256: "71079fb3c7d3b04e9df9d157e0b3ee0e6614cb5198f0729e7b490512c4f8d667",
        bytes: MODEL_BYTES,
    },
    download: MASK_MODEL_DOWNLOAD,
    progress_label: "Depth Anything 3 Mono Large",
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Letterbox {
    width: u32,
    height: u32,
    left: u32,
    top: u32,
}

impl Letterbox {
    fn new(width: u32, height: u32) -> Self {
        let scale = EDGE as f64 / width.max(height) as f64;
        let fitted_width = ((width as f64 * scale).round() as u32).clamp(1, EDGE);
        let fitted_height = ((height as f64 * scale).round() as u32).clamp(1, EDGE);
        Self {
            width: fitted_width,
            height: fitted_height,
            left: (EDGE - fitted_width) / 2,
            top: (EDGE - fitted_height) / 2,
        }
    }

    fn output_rect(self, output_width: u32, output_height: u32) -> (u32, u32, u32, u32) {
        let x0 = (self.left as f64 * output_width as f64 / EDGE as f64).round() as u32;
        let y0 = (self.top as f64 * output_height as f64 / EDGE as f64).round() as u32;
        let x1 =
            ((self.left + self.width) as f64 * output_width as f64 / EDGE as f64).round() as u32;
        let y1 =
            ((self.top + self.height) as f64 * output_height as f64 / EDGE as f64).round() as u32;
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
    let (width, height) = image.dimensions();
    anyhow::ensure!(width > 0 && height > 0, "empty depth-mask input");
    let box_rect = Letterbox::new(width, height);
    let resized =
        image::imageops::resize(image, box_rect.width, box_rect.height, FilterType::Lanczos3);
    let mut canvas = ImageBuffer::from_pixel(EDGE, EDGE, Rgba([124, 116, 104, 255]));
    image::imageops::overlay(
        &mut canvas,
        &resized,
        box_rect.left as i64,
        box_rect.top as i64,
    );

    // The pinned DA3 export takes [batch, views, channels, height, width],
    // including a singleton view dimension even for monocular inference.
    // Its interface is float32 (the internal weights use mixed precision).
    let input = Tensor::from_array((
        [1usize, 1, 3, EDGE as usize, EDGE as usize],
        normalized_rgb_input(&canvas)?,
    ))
    .context("create DA3 input tensor")?;
    let started = std::time::Instant::now();
    let (shape, raw) = with_model_session(
        AiModel::DepthAnything3,
        model_path,
        SessionOptions::new("Depth Anything 3 Mono Large"),
        mask_model_retention(true),
        |session| {
            session.run_with_fallback("DA3 depth ONNX inference", |ort_session, _| {
                let outputs = ort_session
                    .run(ort::inputs!["image" => &input])
                    .context("run DA3 inference")?;
                let output = outputs
                    .get("depth")
                    .context("DA3 returned no depth output")?;
                let (shape, values) = output
                    .try_extract_tensor::<f32>()
                    .context("read DA3 depth tensor")?;
                Ok((shape.as_ref().to_vec(), values.to_vec()))
            })
        },
    )?;
    let (output_width, output_height) = validate_output(&shape, &raw)?;
    calibraw_core::diagnostics::record(format!(
        "AI Depth Anything 3: {width}x{height} source, {output_width}x{output_height} depth in {:.3}s",
        started.elapsed().as_secs_f64()
    ));
    restore_depth(&raw, output_width, output_height, box_rect, width, height)
}

fn validate_output(shape: &[i64], values: &[f32]) -> Result<(u32, u32)> {
    let (height, width) = match shape {
        [1, 1, h, w] => (*h, *w),
        _ => anyhow::bail!("unexpected DA3 output shape {shape:?}; expected [1, 1, H, W]"),
    };
    anyhow::ensure!(
        height > 0 && width > 0 && height <= EDGE as i64 && width <= EDGE as i64,
        "invalid DA3 output dimensions {shape:?}"
    );
    anyhow::ensure!(
        values.len() == (height * width) as usize,
        "DA3 output size does not match its shape"
    );
    anyhow::ensure!(
        values.iter().all(|value| value.is_finite()),
        "DA3 output contains non-finite depth values"
    );
    Ok((width as u32, height as u32))
}

fn normalize_depth(pixels: &[f32]) -> Result<Vec<f32>> {
    anyhow::ensure!(!pixels.is_empty(), "DA3 returned an empty depth map");
    anyhow::ensure!(
        pixels.iter().all(|value| value.is_finite()),
        "DA3 output contains non-finite depth values"
    );
    let mut sorted = pixels.to_vec();
    sorted.sort_unstable_by(f32::total_cmp);
    // Clip the outer 1% so a few extreme predictions cannot collapse most
    // of the scene into the same 8-bit value. Keep near=0 and far=1.
    let tail = sorted.len() / 100;
    let mut min = sorted[tail];
    let mut max = sorted[sorted.len() - 1 - tail];
    if max <= min {
        min = sorted[0];
        max = sorted[sorted.len() - 1];
    }
    anyhow::ensure!(
        max > min,
        "DA3 returned a constant depth map; no depth range could be detected"
    );
    // Use f64 here to retain scale invariance and avoid overflow in max-min.
    let span = max as f64 - min as f64;
    Ok(pixels
        .iter()
        .map(|&depth| ((depth as f64 - min as f64) / span).clamp(0.0, 1.0) as f32)
        .collect())
}

fn restore_depth(
    raw: &[f32],
    output_width: u32,
    output_height: u32,
    box_rect: Letterbox,
    target_width: u32,
    target_height: u32,
) -> Result<Vec<u8>> {
    let image = ImageBuffer::<Luma<f32>, _>::from_raw(output_width, output_height, raw.to_vec())
        .context("invalid DA3 depth image")?;
    let (x0, y0, x1, y1) = box_rect.output_rect(output_width, output_height);
    let cropped = image::imageops::crop_imm(&image, x0, y0, x1 - x0, y1 - y0).to_image();
    let pixels = cropped.into_raw();
    // Remove padding before estimating the range, and normalize before resize
    // (image::resize clamps floating-point Luma samples to [0, 1]).
    let normalized = normalize_depth(&pixels)?;
    let cropped = ImageBuffer::<Luma<f32>, _>::from_raw(x1 - x0, y1 - y0, normalized)
        .context("invalid cropped DA3 depth image")?;
    let aligned =
        image::imageops::resize(&cropped, target_width, target_height, FilterType::Triangle);
    Ok(aligned
        .into_raw()
        .into_iter()
        .map(|depth| (depth.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires CALIBRAW_DA3_MODEL and ONNX Runtime via ORT_DYLIB_PATH"]
    fn published_model_produces_depth() {
        let path = std::env::var_os("CALIBRAW_DA3_MODEL").expect("CALIBRAW_DA3_MODEL");
        let runtime = std::env::var_os("ORT_DYLIB_PATH").expect("ORT_DYLIB_PATH");
        ort::init_from(Path::new(&runtime)).unwrap().commit();
        if std::env::var("CALIBRAW_TEST_DEPTH_PROVIDER").as_deref() == Ok("CPU") {
            crate::set_ai_acceleration_enabled(false);
        }
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
                status.model_name == "Depth Anything 3 Mono Large"
                    && status.active_provider == expected
                    && !status.degraded
            }));
        }
    }

    #[test]
    fn letterbox_preserves_landscape_and_portrait_geometry() {
        assert_eq!(
            Letterbox::new(1400, 700),
            Letterbox {
                width: 700,
                height: 350,
                left: 0,
                top: 175
            }
        );
        assert_eq!(
            Letterbox::new(700, 1400),
            Letterbox {
                width: 350,
                height: 700,
                left: 175,
                top: 0
            }
        );
        assert_eq!(
            Letterbox::new(1400, 700).output_rect(350, 350),
            (0, 88, 350, 263)
        );
        assert_eq!(
            Letterbox::new(700, 1400).output_rect(350, 350),
            (88, 0, 263, 350)
        );
    }

    #[test]
    fn unpadding_discards_extreme_padding_values() {
        let rectangle = Letterbox::new(4, 2);
        let mut raw = vec![1000.0; (EDGE * EDGE) as usize];
        for y in rectangle.top..rectangle.top + rectangle.height {
            for x in 0..EDGE {
                raw[(y * EDGE + x) as usize] = x as f32;
            }
        }
        let result = restore_depth(&raw, EDGE, EDGE, rectangle, 4, 2).unwrap();
        assert_eq!(result.len(), 8);
        assert!(result[0] < result[3]);
        assert_eq!(result[0], result[4]);
    }

    #[test]
    fn rejects_invalid_depth_outputs() {
        assert_eq!(validate_output(&[1, 1, 2, 3], &[1.0; 6]).unwrap(), (3, 2));
        for shape in [[1, 3, 2, 3], [1, 1, 0, 3], [1, 1, 701, 3]] {
            assert!(validate_output(&shape, &[1.0; 6]).is_err());
        }
        assert!(validate_output(&[1, 1, 2, 3], &[1.0; 5]).is_err());
        assert!(validate_output(&[1, 1, 1, 1], &[f32::NAN]).is_err());
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
        let rectangle = Letterbox::new(2, 4);
        let mut raw = vec![-10000.0; (EDGE * EDGE) as usize];
        for y in 0..EDGE {
            for x in rectangle.left..rectangle.left + rectangle.width {
                raw[(y * EDGE + x) as usize] = y as f32;
            }
        }
        let result = restore_depth(&raw, EDGE, EDGE, rectangle, 2, 4).unwrap();
        assert_eq!(result.len(), 8);
        assert!(result[0] < 64);
        assert!(result[6] > 191);
        assert!(result.chunks_exact(2).all(|row| row[0] == row[1]));
        assert_eq!(Letterbox::new(1, 10000).output_rect(1, 1), (0, 0, 1, 1));
    }
}
