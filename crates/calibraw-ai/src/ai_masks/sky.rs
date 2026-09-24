use super::*;

const INPUT_EDGE: u32 = 320;

pub(super) fn sky_mask(
    model_path: &Path,
    image: &ImageBuffer<Rgba<u8>, Vec<u8>>,
) -> Result<Vec<u8>> {
    let resized = image::imageops::resize(image, INPUT_EDGE, INPUT_EDGE, FilterType::Lanczos3);
    let area = (INPUT_EDGE * INPUT_EDGE) as usize;
    let mut normalized = vec![0.0f32; area * 3];
    for (index, pixel) in resized.pixels().enumerate() {
        for channel in 0..3 {
            normalized[channel * area + index] =
                (pixel[channel] as f32 / 255.0 - IMAGENET_MEAN[channel]) / IMAGENET_STD[channel];
        }
    }
    let input = Tensor::from_array((
        [1usize, 3, INPUT_EDGE as usize, INPUT_EDGE as usize],
        normalized,
    ))
    .context("create sky segmentation input tensor")?;
    let (output_width, output_height, sky_probabilities) = with_model_session(
        AiModel::SkySeg,
        model_path,
        SessionOptions::new("SkySeg U2Net"),
        mask_model_retention(true),
        |session| {
            session.run_with_fallback("SkySeg ONNX inference", |ort_session, _| {
                let outputs = ort_session
                    .run(ort::inputs![&input])
                    .context("run SkySeg ONNX inference")?;
                let output = outputs
                    .get("1959")
                    .context("SkySeg returned no primary sky output")?;
                let (shape, probabilities) = output
                    .try_extract_tensor::<f32>()
                    .context("read SkySeg output tensor")?;
                let (width, height) = validate_output_shape(shape, probabilities.len())?;
                Ok((width, height, sky_probabilities_to_mask(probabilities)?))
            })
        },
    )?;
    let mask = image::GrayImage::from_raw(output_width, output_height, sky_probabilities)
        .context("SkySeg returned an invalid sky mask")?;
    let mut mask =
        image::imageops::resize(&mask, image.width(), image.height(), FilterType::Triangle)
            .into_raw();
    mask_refine::guided_filter_color(
        image.as_raw(),
        &mut mask,
        image.width(),
        image.height(),
        8,
        1e-4,
    )?;
    Ok(mask)
}

fn validate_output_shape(shape: &[i64], len: usize) -> Result<(u32, u32)> {
    anyhow::ensure!(
        shape.len() == 4 && shape[0] == 1 && shape[1] == 1,
        "unexpected SkySeg output shape {shape:?}; expected [1, 1, H, W]"
    );
    let height = usize::try_from(shape[2]).context("invalid SkySeg output height")?;
    let width = usize::try_from(shape[3]).context("invalid SkySeg output width")?;
    anyhow::ensure!(
        width > 0 && height > 0 && width <= INPUT_EDGE as usize && height <= INPUT_EDGE as usize,
        "SkySeg output dimensions are invalid: {shape:?}"
    );
    anyhow::ensure!(
        len == width * height,
        "SkySeg output tensor length does not match {shape:?}"
    );
    Ok((width as u32, height as u32))
}

fn sky_probabilities_to_mask(probabilities: &[f32]) -> Result<Vec<u8>> {
    let mut result = Vec::with_capacity(probabilities.len());
    for &probability in probabilities {
        anyhow::ensure!(probability.is_finite(), "SkySeg returned non-finite values");
        result.push((probability.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_absolute_sky_probabilities() {
        let mask = sky_probabilities_to_mask(&[0.0, 0.5, 1.0]).unwrap();
        assert_eq!(mask, [0, 128, 255]);
    }

    #[test]
    fn rejects_wrong_output_shape() {
        assert!(validate_output_shape(&[1, 3, 320, 320], 3 * 320 * 320).is_err());
    }

    #[test]
    #[ignore = "requires the published model and ONNX Runtime"]
    fn published_model_runs_end_to_end() {
        let path = std::env::var("CALIBRAW_TEST_SKYSEG_MODEL").unwrap();
        super::initialize_runtime(None, None).unwrap();
        let image = ImageBuffer::from_pixel(64, 40, Rgba([100, 160, 220, 255]));
        let mask = sky_mask(Path::new(&path), &image).unwrap();
        assert_eq!(mask.len(), 64 * 40);
    }
}
