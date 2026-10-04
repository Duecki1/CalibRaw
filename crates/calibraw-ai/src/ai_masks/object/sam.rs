//! SAM 2.1 encoder and decoder runs and their tensor conversions.

use super::*;

pub(super) fn encode_sam_image(
    encoder_path: &Path,
    resized: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    source_width: u32,
    source_height: u32,
    crop: ObjectCropRect,
) -> Result<ObjectInferenceCache> {
    let input = Tensor::from_array((
        [
            1usize,
            3,
            SAM21_MODEL_SIZE as usize,
            SAM21_MODEL_SIZE as usize,
        ],
        normalized_rgb_input(resized)?,
    ))
    .context("create SAM 2.1 encoder input")?;

    let tensors = with_model_session(
        AiModel::SamEncoder,
        encoder_path,
        SessionOptions::new("SAM 2.1 encoder")
            .with_cpu_fallback_profile(CpuFallbackProfile::WindowsSamEncoder),
        mask_model_retention(cache_object_ai_sessions()),
        |session| run_sam_encoder(session, input),
    )?;

    Ok(ObjectInferenceCache {
        source_width,
        source_height,
        crop,
        high_res_feats_0: tensors.0,
        high_res_feats_1: tensors.1,
        image_embedding: tensors.2,
        low_res_logits: vec![0.0; (SAM21_MASK_INPUT_SIZE * SAM21_MASK_INPUT_SIZE) as usize].into(),
        prompt_strokes: Vec::new(),
        prompt_brush_size: 0.0,
    })
}

fn run_sam_encoder(
    session: &mut FallbackSession,
    input: Tensor<f32>,
) -> Result<(SamTensorData, SamTensorData, SamTensorData)> {
    session.run_with_fallback(
        "SAM 2.1 image encoder inference",
        |ort_session, accelerated| {
            let outputs = ort_session
                .run(ort::inputs![&input])
                .context("run SAM 2.1 image encoder")?;
            Ok((
                extract_sam_encoder_output(&outputs, 0, "high-resolution feature 0", accelerated)?,
                extract_sam_encoder_output(&outputs, 1, "high-resolution feature 1", accelerated)?,
                extract_sam_encoder_output(&outputs, 2, "image embedding", accelerated)?,
            ))
        },
    )
}

fn extract_sam_encoder_output(
    outputs: &ort::session::SessionOutputs<'_>,
    index: usize,
    label: &str,
    _accelerated: bool,
) -> Result<SamTensorData> {
    let value = outputs
        .values()
        .nth(index)
        .with_context(|| format!("SAM 2.1 returned no {label}"))?;
    let (shape, data) = value
        .try_extract_tensor::<f32>()
        .with_context(|| format!("read SAM 2.1 {label}"))?;

    let non_finite = data.iter().filter(|value| !value.is_finite()).count();
    #[cfg(target_os = "windows")]
    let values = if non_finite > 0 {
        anyhow::ensure!(
            !_accelerated,
            "SAM 2.1 {label} produced {non_finite} non-finite values on the accelerated execution provider"
        );
        let repair_limit = 64usize.max(data.len() / 100_000);
        anyhow::ensure!(
            non_finite <= repair_limit,
            "SAM 2.1 {label} is numerically corrupted on CPU: {non_finite} of {} values are non-finite. Select a current Microsoft x64 CPU onnxruntime.dll and restart CalibRaw",
            data.len()
        );
        log::warn!(
            "SAM 2.1 {label} contained {non_finite} isolated non-finite values on Windows CPU; replacing them with zero"
        );
        data.iter()
            .map(|value| if value.is_finite() { *value } else { 0.0 })
            .collect::<Vec<_>>()
    } else {
        data.to_vec()
    };
    #[cfg(not(target_os = "windows"))]
    let values = {
        anyhow::ensure!(
            non_finite == 0,
            "SAM 2.1 {label} contains non-finite values"
        );
        data.to_vec()
    };

    let shape = sam_tensor_shape(shape, values.len())?;
    Ok(SamTensorData {
        shape,
        values: values.into(),
    })
}

fn extract_f32_output(
    outputs: &ort::session::SessionOutputs<'_>,
    index: usize,
    label: &str,
) -> Result<SamTensorData> {
    let value = outputs
        .values()
        .nth(index)
        .with_context(|| format!("SAM 2.1 returned no {label}"))?;
    let (shape, data) = value
        .try_extract_tensor::<f32>()
        .with_context(|| format!("read SAM 2.1 {label}"))?;
    anyhow::ensure!(
        data.iter().all(|value| value.is_finite()),
        "SAM 2.1 {label} contains non-finite values"
    );
    let shape = sam_tensor_shape(shape, data.len())?;
    Ok(SamTensorData {
        shape,
        values: data.to_vec().into(),
    })
}

fn sam_tensor_shape(shape: &ort::value::Shape, value_count: usize) -> Result<Vec<usize>> {
    let shape = shape
        .iter()
        .map(|dimension| usize::try_from(*dimension).context("negative SAM tensor dimension"))
        .collect::<Result<Vec<_>>>()?;
    let expected = shape.iter().try_fold(1usize, |product, dimension| {
        product
            .checked_mul(*dimension)
            .context("SAM tensor shape overflow")
    })?;
    anyhow::ensure!(
        expected == value_count,
        "SAM tensor shape does not match its data"
    );
    Ok(shape)
}

pub(super) struct DecodedSamMask {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) probabilities: Vec<f32>,
    pub(super) selected_logits: Vec<f32>,
}

pub(super) fn decode_sam_mask(
    decoder_path: &Path,
    cache: &ObjectInferenceCache,
    prompt_set: &ObjectPromptSet,
    use_previous_mask: bool,
) -> Result<DecodedSamMask> {
    let prompts = &prompt_set.prompts;
    anyhow::ensure!(!prompts.is_empty(), "SAM 2.1 requires at least one prompt");
    anyhow::ensure!(
        prompts.len() <= SAM21_MAX_PROMPTS,
        "SAM 2.1 prompt count exceeds the fixed decoder budget"
    );
    let mut coords = vec![0.0f32; SAM21_MAX_PROMPTS * 2];
    let mut labels = vec![-1.0f32; SAM21_MAX_PROMPTS];
    for (index, prompt) in prompts.iter().enumerate() {
        let source_x = prompt.point[0].clamp(0.0, 1.0) * cache.source_width as f32;
        let source_y = prompt.point[1].clamp(0.0, 1.0) * cache.source_height as f32;
        coords[index * 2] = ((source_x - cache.crop.x as f32) / cache.crop.width.max(1) as f32
            * SAM21_MODEL_SIZE as f32)
            .clamp(0.0, SAM21_MODEL_SIZE as f32 - 1.0);
        coords[index * 2 + 1] = ((source_y - cache.crop.y as f32)
            / cache.crop.height.max(1) as f32
            * SAM21_MODEL_SIZE as f32)
            .clamp(0.0, SAM21_MODEL_SIZE as f32 - 1.0);
        labels[index] = prompt.kind.sam_label();
    }

    let image_embedding = tensor_from_sam_data(&cache.image_embedding, "image embedding")?;
    let high_res_0 = tensor_from_sam_data(&cache.high_res_feats_0, "high-resolution feature 0")?;
    let high_res_1 = tensor_from_sam_data(&cache.high_res_feats_1, "high-resolution feature 1")?;
    let point_coords = Tensor::from_array(([1usize, SAM21_MAX_PROMPTS, 2usize], coords))
        .context("create SAM point coordinates")?;
    let point_labels = Tensor::from_array(([1usize, SAM21_MAX_PROMPTS], labels))
        .context("create SAM point labels")?;
    let mask_values = if use_previous_mask {
        cache.low_res_logits.to_vec()
    } else {
        vec![0.0; (SAM21_MASK_INPUT_SIZE * SAM21_MASK_INPUT_SIZE) as usize]
    };
    let mask_input = Tensor::from_array((
        [
            1usize,
            1,
            SAM21_MASK_INPUT_SIZE as usize,
            SAM21_MASK_INPUT_SIZE as usize,
        ],
        mask_values,
    ))
    .context("create SAM previous-mask input")?;
    let has_mask = Tensor::from_array(([1usize], vec![if use_previous_mask { 1.0 } else { 0.0 }]))
        .context("create SAM previous-mask flag")?;

    let (masks, scores) = with_model_session(
        AiModel::SamDecoder,
        decoder_path,
        SessionOptions::new("SAM 2.1 decoder"),
        mask_model_retention(cache_object_ai_sessions()),
        |session| {
            run_sam_decoder(
                session,
                SamDecoderInputs {
                    image_embedding,
                    high_res_0,
                    high_res_1,
                    point_coords,
                    point_labels,
                    mask_input,
                    has_mask,
                },
            )
        },
    )?;
    select_sam_candidate(masks, scores, prompt_set, cache)
}

fn tensor_from_sam_data(data: &SamTensorData, label: &str) -> Result<Tensor<f32>> {
    Tensor::from_array((data.shape.clone(), data.values.to_vec()))
        .with_context(|| format!("create SAM {label} input"))
}

struct SamDecoderInputs {
    image_embedding: Tensor<f32>,
    high_res_0: Tensor<f32>,
    high_res_1: Tensor<f32>,
    point_coords: Tensor<f32>,
    point_labels: Tensor<f32>,
    mask_input: Tensor<f32>,
    has_mask: Tensor<f32>,
}

fn run_sam_decoder(
    session: &mut FallbackSession,
    inputs: SamDecoderInputs,
) -> Result<(SamTensorData, SamTensorData)> {
    let SamDecoderInputs {
        image_embedding,
        high_res_0,
        high_res_1,
        point_coords,
        point_labels,
        mask_input,
        has_mask,
    } = inputs;
    session.run_with_fallback(
        "SAM 2.1 mask decoder inference",
        |ort_session, _accelerated| {
            let outputs = ort_session
                .run(ort::inputs![
                    &image_embedding,
                    &high_res_0,
                    &high_res_1,
                    &point_coords,
                    &point_labels,
                    &mask_input,
                    &has_mask
                ])
                .context("run SAM 2.1 mask decoder")?;
            Ok((
                extract_f32_output(&outputs, 0, "mask logits")?,
                extract_f32_output(&outputs, 1, "mask scores")?,
            ))
        },
    )
}
