//! Running RawNIND over Bayer mosaics and linear rasters, one tile at a time.

use super::*;

pub(super) fn run_model_tile(
    session: &mut FallbackSession,
    channels: usize,
    values: Vec<f32>,
    output_edge: usize,
) -> Result<Vec<f32>> {
    anyhow::ensure!(
        values.len() == channels * TILE_EDGE * TILE_EDGE,
        "RawNIND input tensor has the wrong length"
    );
    let input = Tensor::from_array(([1usize, channels, TILE_EDGE, TILE_EDGE], values))
        .context("create RawNIND input tensor")?;
    session.run_with_fallback(
        "RawNIND ONNX tile inference",
        |ort_session, _accelerated| {
            let outputs = ort_session
                .run(ort::inputs![&input])
                .context("run RawNIND ONNX inference")?;
            let output = outputs
                .values()
                .next()
                .context("RawNIND returned no output tensor")?;
            let (shape, values) = output
                .try_extract_tensor::<f32>()
                .context("read RawNIND output tensor")?;
            anyhow::ensure!(
                shape.as_ref() == [1, 3, output_edge as i64, output_edge as i64],
                "unexpected RawNIND output shape {shape:?}"
            );
            let non_finite = values.iter().filter(|value| !value.is_finite()).count();
            anyhow::ensure!(
                non_finite == 0,
                "RawNIND produced {non_finite} non-finite output values"
            );
            Ok(values.to_vec())
        },
    )
}

pub(super) fn infer_bayer(
    model_path: &Path,
    raw: &LoadedRaw,
    events: &mpsc::Sender<AiDenoiseEvent>,
    cancellation: &AtomicBool,
) -> Result<AiDenoisedImage> {
    let (origin_x, origin_y) = bayer_rggb_origin(raw)?;
    let packed_width = (raw.width - origin_x) / 2;
    let packed_height = (raw.height - origin_y) / 2;
    anyhow::ensure!(
        packed_width > 0 && packed_height > 0,
        "Bayer RAW is too small for RawNIND"
    );
    let tiles_x = packed_width.div_ceil(CORE_EDGE as u32) as usize;
    let tiles_y = packed_height.div_ceil(CORE_EDGE as u32) as usize;
    let total_tiles = tiles_x * tiles_y;
    let output_elements = u64::from(raw.width)
        .checked_mul(u64::from(raw.height))
        .and_then(|elements| usize::try_from(elements).ok())
        .context("RawNIND Bayer output dimensions overflow")?;
    let mut normalized_cfa = vec![0.0f32; output_elements];
    let mut session = acquire_model_session(
        AiModel::RawNindBayer,
        model_path,
        SessionOptions::new("RawNIND Bayer"),
        ModelRetention::OneShot,
    )?;
    let model_white_balance = raw.rawnind_daylight_white_balance();
    for tile_y in 0..tiles_y {
        for tile_x in 0..tiles_x {
            ensure_not_cancelled(cancellation)?;
            let core_x = tile_x * CORE_EDGE;
            let core_y = tile_y * CORE_EDGE;
            let core_width = CORE_EDGE.min(packed_width as usize - core_x);
            let core_height = CORE_EDGE.min(packed_height as usize - core_y);
            let mut input = vec![0.0f32; 4 * TILE_EDGE * TILE_EDGE];
            for tile_row in 0..TILE_EDGE {
                let packed_y = reflect_index(
                    core_y as i64 + tile_row as i64 - OVERLAP as i64,
                    packed_height as usize,
                );
                for tile_column in 0..TILE_EDGE {
                    let packed_x = reflect_index(
                        core_x as i64 + tile_column as i64 - OVERLAP as i64,
                        packed_width as usize,
                    );
                    for (channel, ([phase_x, phase_y], wb_channel)) in [
                        ([0u32, 0u32], 0usize),
                        ([1, 0], 1),
                        ([0, 1], 1),
                        ([1, 1], 2),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let x = origin_x + packed_x as u32 * 2 + phase_x;
                        let y = origin_y + packed_y as u32 * 2 + phase_y;
                        input[channel * TILE_EDGE * TILE_EDGE
                            + tile_row * TILE_EDGE
                            + tile_column] =
                            normalized_sensor_site(raw, x, y) * model_white_balance[wb_channel];
                    }
                }
            }
            let mut output = run_model_tile(&mut session, 4, input.clone(), TILE_EDGE * 2)?;
            match_gain_tile(&input, &mut output)?;
            let output_edge = TILE_EDGE * 2;
            let output_plane = output_edge * output_edge;
            let sensor_overlap = OVERLAP * 2;
            let core_start_x = origin_x as usize + core_x * 2;
            let core_start_y = origin_y as usize + core_y * 2;
            let core_end_x = core_start_x + core_width * 2;
            let core_end_y = core_start_y + core_height * 2;
            let has_left = tile_x > 0;
            let has_right = tile_x + 1 < tiles_x;
            let has_top = tile_y > 0;
            let has_bottom = tile_y + 1 < tiles_y;
            let working_end_x = origin_x as usize + packed_width as usize * 2;
            let working_end_y = origin_y as usize + packed_height as usize * 2;
            let extended_start_x = if has_left {
                core_start_x - sensor_overlap
            } else {
                core_start_x
            };
            let extended_start_y = if has_top {
                core_start_y - sensor_overlap
            } else {
                core_start_y
            };
            let extended_end_x = if has_right {
                core_end_x + sensor_overlap
            } else {
                core_end_x
            }
            .min(working_end_x);
            let extended_end_y = if has_bottom {
                core_end_y + sensor_overlap
            } else {
                core_end_y
            }
            .min(working_end_y);
            for destination_y in extended_start_y..extended_end_y {
                let model_y = sensor_overlap + destination_y - core_start_y;
                let weight_y = seam_weight(
                    destination_y,
                    core_start_y,
                    core_end_y,
                    sensor_overlap,
                    has_top,
                    has_bottom,
                );
                for destination_x in extended_start_x..extended_end_x {
                    let destination = destination_y * raw.width as usize + destination_x;
                    let model_x = sensor_overlap + destination_x - core_start_x;
                    let model_index = model_y * output_edge + model_x;
                    let weight_x = seam_weight(
                        destination_x,
                        core_start_x,
                        core_end_x,
                        sensor_overlap,
                        has_left,
                        has_right,
                    );
                    let channel = match raw.color_indices[destination] {
                        0 => 0,
                        2 => 2,
                        _ => 1,
                    };
                    let value =
                        output[channel * output_plane + model_index] / model_white_balance[channel];
                    anyhow::ensure!(
                        value.is_finite(),
                        "RawNIND Bayer remosaic produced a non-finite value"
                    );
                    normalized_cfa[destination] += value.clamp(0.0, 1.0) * weight_x * weight_y;
                }
            }
            let completed = tile_y * tiles_x + tile_x + 1;
            let _ = events.send(AiDenoiseEvent::Progress {
                phase: "Denoising Bayer mosaic",
                completed,
                total: total_tiles,
            });
        }
    }
    drop(session);

    fill_bayer_crop_edges(
        &mut normalized_cfa,
        raw.width,
        raw.height,
        origin_x,
        origin_y,
        packed_width * 2,
        packed_height * 2,
    );
    const CLIP_THRESHOLD: f32 = 0.98;
    const HIGH_SNR_SHOULDER_START: f32 = 0.72;
    const CLIP_CORE_RADIUS: u8 = 4;
    const CLIP_FEATHER_RADIUS: u8 = 32;
    let width = raw.width as usize;
    let height = raw.height as usize;
    let mut clip_distance = vec![u8::MAX; output_elements];
    for y in 0..height {
        for x in 0..width {
            if normalized_sensor_site(raw, x as u32, y as u32) >= CLIP_THRESHOLD {
                clip_distance[y * width + x] = 0;
            }
        }
    }
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            let mut distance = clip_distance[index];
            if x > 0 {
                distance = distance.min(clip_distance[index - 1].saturating_add(1));
            }
            if y > 0 {
                distance = distance.min(clip_distance[index - width].saturating_add(1));
            }
            clip_distance[index] = distance;
        }
    }
    for y in (0..height).rev() {
        for x in (0..width).rev() {
            let index = y * width + x;
            let mut distance = clip_distance[index];
            if x + 1 < width {
                distance = distance.min(clip_distance[index + 1].saturating_add(1));
            }
            if y + 1 < height {
                distance = distance.min(clip_distance[index + width].saturating_add(1));
            }
            clip_distance[index] = distance;
        }
    }
    let mut stored = vec![0u16; output_elements];
    for (index, destination) in stored.iter_mut().enumerate() {
        let cfa = raw.color_indices[index] as usize;
        let black = raw.black_levels_per_pixel[index];
        let white = raw.white_levels[cfa].max(black + 1.0);
        let original =
            ((f32::from(raw.raw_pixels[index]) - black) / (white - black)).clamp(0.0, 1.0);
        let distance = clip_distance[index];
        let proximity = if distance <= CLIP_CORE_RADIUS {
            1.0
        } else if distance < CLIP_FEATHER_RADIUS {
            let t = f32::from(CLIP_FEATHER_RADIUS - distance)
                / f32::from(CLIP_FEATHER_RADIUS - CLIP_CORE_RADIUS);
            t * t * (3.0 - 2.0 * t)
        } else {
            0.0
        };
        let shoulder_t = ((original - HIGH_SNR_SHOULDER_START)
            / (CLIP_THRESHOLD - HIGH_SNR_SHOULDER_START))
            .clamp(0.0, 1.0);
        let shoulder = shoulder_t * shoulder_t * (3.0 - 2.0 * shoulder_t);
        let source_weight = proximity.max(shoulder);
        let normalized = normalized_cfa[index].clamp(0.0, 1.0) * (1.0 - source_weight)
            + original * source_weight;
        let code = black + normalized * (white - black);
        *destination = code.round().clamp(0.0, f32::from(u16::MAX)) as u16;
    }
    AiDenoisedImage::new_bayer_cfa(raw.width, raw.height, stored)
}

pub(super) fn infer_linear(
    model_path: &Path,
    raw: &LoadedRaw,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    events: &mpsc::Sender<AiDenoiseEvent>,
    cancellation: &AtomicBool,
) -> Result<AiDenoisedImage> {
    let tiles_x = raw.width.div_ceil(CORE_EDGE as u32) as usize;
    let tiles_y = raw.height.div_ceil(CORE_EDGE as u32) as usize;
    let total_tiles = tiles_x * tiles_y;
    let output_elements = u64::from(raw.width)
        .checked_mul(u64::from(raw.height))
        .and_then(|pixels| pixels.checked_mul(3))
        .and_then(|elements| usize::try_from(elements).ok())
        .context("RawNIND linear output dimensions overflow")?;
    let mut stored = vec![0u16; output_elements];
    let mut session = acquire_model_session(
        AiModel::RawNindLinear,
        model_path,
        SessionOptions::new("RawNIND linear"),
        ModelRetention::OneShot,
    )?;
    let mut neutral = ExposureParams::scene_referred_default();
    neutral.ai_denoise_enabled = false;
    neutral.luminance_denoise = 0.0;
    neutral.chroma_denoise = 0.0;
    neutral.ca_red = 0.0;
    neutral.ca_blue = 0.0;
    let masks = MaskStack::default();
    if raw.uses_opposed_chroma(&neutral) {
        raw.inpaint_opposed_chroma_for_exposure(&neutral);
    }
    let mut pipeline: Option<RawGpuPipeline> = None;
    let cam_to_rec2020 = matrix::multiply(LINEAR_SRGB_TO_REC2020, rows3(raw.cam_to_srgb));
    let rec2020_to_cam =
        matrix::invert(cam_to_rec2020).context("camera-to-Rec.2020 matrix is singular")?;
    for tile_y in 0..tiles_y {
        for tile_x in 0..tiles_x {
            ensure_not_cancelled(cancellation)?;
            let core_x = tile_x * CORE_EDGE;
            let core_y = tile_y * CORE_EDGE;
            let core_width = CORE_EDGE.min(raw.width as usize - core_x);
            let core_height = CORE_EDGE.min(raw.height as usize - core_y);
            let origin_x = core_x as i32 - OVERLAP as i32;
            let origin_y = core_y as i32 - OVERLAP as i32;
            let tile_raw = reflected_raw_tile(raw, origin_x, origin_y)?;
            let params = GpuParams::new_for_tile(
                &neutral, &masks, &tile_raw, origin_x, origin_y, raw.width, raw.height,
            );
            if let Some(existing) = &pipeline {
                existing.upload_raw_tile(queue, &tile_raw)?;
            } else {
                pipeline = Some(RawGpuPipeline::new(
                    device,
                    queue,
                    &tile_raw,
                    &params,
                    PipelineOptions::new(ProcessingQuality::High),
                )?);
            }
            let camera = pipeline
                .as_ref()
                .context("X-Trans demosaic pipeline was not created")?
                .render_camera_scene_blocking(device, queue, &params)?;
            let mut input = vec![0.0f32; 3 * TILE_EDGE * TILE_EDGE];
            for pixel_index in 0..TILE_EDGE * TILE_EDGE {
                let rgb = matrix::transform(
                    cam_to_rec2020,
                    [
                        camera[pixel_index * 3],
                        camera[pixel_index * 3 + 1],
                        camera[pixel_index * 3 + 2],
                    ],
                );
                for channel in 0..3 {
                    input[channel * TILE_EDGE * TILE_EDGE + pixel_index] = rgb[channel];
                }
            }
            let mut output = run_model_tile(&mut session, 3, input.clone(), TILE_EDGE)?;
            match_gain_tile(&input, &mut output)?;
            let has_left = tile_x > 0;
            let has_right = tile_x + 1 < tiles_x;
            let has_top = tile_y > 0;
            let has_bottom = tile_y + 1 < tiles_y;
            let core_end_x = core_x + core_width;
            let core_end_y = core_y + core_height;
            let extended_start_x = if has_left { core_x - OVERLAP } else { core_x };
            let extended_start_y = if has_top { core_y - OVERLAP } else { core_y };
            let extended_end_x = if has_right {
                core_end_x + OVERLAP
            } else {
                core_end_x
            }
            .min(raw.width as usize);
            let extended_end_y = if has_bottom {
                core_end_y + OVERLAP
            } else {
                core_end_y
            }
            .min(raw.height as usize);
            for destination_y in extended_start_y..extended_end_y {
                let model_y = OVERLAP + destination_y - core_y;
                let weight_y = seam_weight(
                    destination_y,
                    core_y,
                    core_end_y,
                    OVERLAP,
                    has_top,
                    has_bottom,
                );
                for destination_x in extended_start_x..extended_end_x {
                    let destination = (destination_y * raw.width as usize + destination_x) * 3;
                    let model_x = OVERLAP + destination_x - core_x;
                    let model_index = model_y * TILE_EDGE + model_x;
                    let weight_x = seam_weight(
                        destination_x,
                        core_x,
                        core_end_x,
                        OVERLAP,
                        has_left,
                        has_right,
                    );
                    accumulate_half_rgb(
                        &mut stored,
                        destination,
                        [
                            output[model_index],
                            output[TILE_EDGE * TILE_EDGE + model_index],
                            output[2 * TILE_EDGE * TILE_EDGE + model_index],
                        ],
                        weight_x * weight_y,
                        "RawNIND linear output",
                    )?;
                }
            }
            let completed = tile_y * tiles_x + tile_x + 1;
            let _ = events.send(AiDenoiseEvent::Progress {
                phase: "Demosaicing and denoising X-Trans",
                completed,
                total: total_tiles,
            });
        }
    }
    drop(session);

    for pixel in stored.chunks_exact_mut(3) {
        let rec2020 = [
            half::f16::from_bits(pixel[0]).to_f32(),
            half::f16::from_bits(pixel[1]).to_f32(),
            half::f16::from_bits(pixel[2]).to_f32(),
        ];
        let camera = matrix::transform(rec2020_to_cam, rec2020);
        for channel in 0..3 {
            anyhow::ensure!(
                camera[channel].is_finite() && camera[channel].abs() <= half::f16::MAX.to_f32(),
                "RawNIND linear colour conversion overflowed"
            );
            pixel[channel] = half::f16::from_f32(camera[channel]).to_bits();
        }
    }
    AiDenoisedImage::new(raw.width, raw.height, stored)
}
