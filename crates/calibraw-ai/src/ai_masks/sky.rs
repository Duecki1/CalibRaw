use super::*;

const INPUT_EDGE: u32 = 384;

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
    .context("create SkyWater input tensor")?;
    let (output_width, output_height, sky_probabilities) = with_model_session(
        AiModel::SkyWater,
        model_path,
        SessionOptions::new("SkyWater SegFormer-B2"),
        mask_model_retention(true),
        |session| {
            session.run_with_fallback("SkyWater ONNX inference", |ort_session, _| {
                let outputs = ort_session
                    .run(ort::inputs![&input])
                    .context("run SkyWater ONNX inference")?;
                let output = outputs
                    .values()
                    .next()
                    .context("SkyWater returned no output tensors")?;
                let (shape, logits) = output
                    .try_extract_tensor::<f32>()
                    .context("read SkyWater output tensor")?;
                let (width, height) = validate_output_shape(shape, logits.len())?;
                Ok((
                    width,
                    height,
                    sky_softmax(logits, width as usize * height as usize)?,
                ))
            })
        },
    )?;
    let mask = image::GrayImage::from_raw(output_width, output_height, sky_probabilities)
        .context("SkyWater returned an invalid sky mask")?;
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
        shape.len() == 4 && shape[0] == 1 && shape[1] == 4,
        "unexpected SkyWater output shape {shape:?}; expected [1, 4, H, W]"
    );
    let height = usize::try_from(shape[2]).context("invalid SkyWater output height")?;
    let width = usize::try_from(shape[3]).context("invalid SkyWater output width")?;
    anyhow::ensure!(
        width > 0 && height > 0 && width <= 384 && height <= 384,
        "SkyWater output dimensions are invalid: {shape:?}"
    );
    anyhow::ensure!(
        len == width * height * 4,
        "SkyWater output tensor length does not match {shape:?}"
    );
    Ok((width as u32, height as u32))
}

fn sky_softmax(logits: &[f32], area: usize) -> Result<Vec<u8>> {
    anyhow::ensure!(logits.len() == area * 4, "invalid SkyWater logits length");
    let mut result = Vec::with_capacity(area);
    for pixel in 0..area {
        let classes = [
            logits[pixel],
            logits[area + pixel],
            logits[2 * area + pixel],
            logits[3 * area + pixel],
        ];
        anyhow::ensure!(
            classes.iter().all(|value| value.is_finite()),
            "SkyWater returned non-finite logits"
        );
        let maximum = classes.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let exponentials = classes.map(|value| (value - maximum).exp());
        let probability = exponentials[1] / exponentials.iter().sum::<f32>();
        result.push((probability * 255.0 + 0.5) as u8);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sky_is_class_one_in_nchw_logits() {
        let logits = [0.0, 9.0, 9.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mask = sky_softmax(&logits, 2).unwrap();
        assert!(mask[0] > 250);
        assert!(mask[1] < 5);
    }

    #[test]
    fn rejects_wrong_output_shape() {
        assert!(validate_output_shape(&[1, 3, 384, 384], 3 * 384 * 384).is_err());
    }

    #[test]
    #[ignore = "requires the published model and ONNX Runtime"]
    fn published_model_runs_end_to_end() {
        let path = std::env::var("CALIBRAW_TEST_SKYWATER_MODEL").unwrap();
        super::initialize_runtime(None, None).unwrap();
        let image = ImageBuffer::from_pixel(64, 40, Rgba([100, 160, 220, 255]));
        let mask = sky_mask(Path::new(&path), &image).unwrap();
        assert_eq!(mask.len(), 64 * 40);
    }
}
