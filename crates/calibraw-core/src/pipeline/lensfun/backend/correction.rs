//! Applying distortion, TCA and vignetting corrections to mosaic and raster data.

use super::*;

#[derive(Clone, Copy)]
pub(super) struct ModifierConfig {
    pub(super) crop: f32,
    pub(super) dimensions: [c_int; 2],
    pub(super) focal: f32,
    pub(super) aperture: f32,
    pub(super) distance: f32,
    pub(super) requested_flags: c_int,
}

pub(super) fn initialize_modifier(
    lens: *const lfLens,
    config: ModifierConfig,
) -> Result<(Modifier, c_int)> {
    let ModifierConfig {
        crop,
        dimensions: [width, height],
        focal,
        aperture,
        distance,
        requested_flags,
    } = config;
    let pointer = unsafe { lf_modifier_new(lens, crop.max(0.1), width, height) };
    if pointer.is_null() {
        return Err(anyhow!("Lensfun could not create a modifier"));
    }
    let modifier = Modifier(pointer);
    let lens_type = lens_fields(lens)
        .ok_or_else(|| anyhow!("Lensfun returned a null lens profile"))?
        .lens_type;
    let flags = unsafe {
        lf_modifier_initialize(
            modifier.0,
            lens,
            ffi::LF_PF_F32,
            focal,
            aperture,
            distance,
            1.0,
            lens_type,
            requested_flags,
            0,
        )
    };
    Ok((modifier, flags))
}

pub(super) fn build_lens_geometry_map(
    raw: &LoadedRaw,
    modifier: &Modifier,
    flags: c_int,
) -> Result<Option<Arc<LensGeometryMap>>> {
    if flags & (LF_MODIFY_DISTORTION | LF_MODIFY_GEOMETRY | LF_MODIFY_SCALE) == 0 {
        return Ok(None);
    }

    const MAX_GRID_STEP: u32 = 32;
    let grid_width = raw.width.saturating_sub(1).div_ceil(MAX_GRID_STEP) + 1;
    let grid_height = raw.height.saturating_sub(1).div_ceil(MAX_GRID_STEP) + 1;
    let grid_width = grid_width.max(2);
    let grid_height = grid_height.max(2);
    let mut coordinates = Vec::with_capacity(grid_width as usize * grid_height as usize);
    let mut mapped = [0.0f32; 6];

    for grid_y in 0..grid_height {
        let y = if grid_height <= 1 {
            0.0
        } else {
            grid_y as f32 * raw.height.saturating_sub(1) as f32 / (grid_height - 1) as f32
        };
        for grid_x in 0..grid_width {
            let x = if grid_width <= 1 {
                0.0
            } else {
                grid_x as f32 * raw.width.saturating_sub(1) as f32 / (grid_width - 1) as f32
            };
            mapped.fill(0.0);
            let filled = unsafe {
                lf_modifier_apply_subpixel_geometry_distortion(
                    modifier.0,
                    x,
                    y,
                    1,
                    1,
                    mapped.as_mut_ptr(),
                )
            };
            coordinates.push(if filled == 0 {
                [x, y]
            } else {
                [mapped[2], mapped[3]]
            });
        }
    }

    let map = LensGeometryMap::new(raw.width, raw.height, grid_width, grid_height, coordinates)
        .ok_or_else(|| anyhow!("Lensfun produced an invalid distortion map"))?;
    Ok(Some(Arc::new(map)))
}

pub(super) fn correct_mosaic(
    raw: &LoadedRaw,
    modifier: &Modifier,
    flags: c_int,
    lens_geometry: Option<Arc<LensGeometryMap>>,
) -> Result<LoadedRaw> {
    if raw.is_pre_demosaiced_raster() {
        return correct_raster(raw, modifier, flags, lens_geometry);
    }
    let width = raw.width as usize;
    let height = raw.height as usize;
    let mut raw_pixels = vec![0u16; raw.raw_pixels.len()];
    let uniform_black = raw
        .black_levels_per_pixel
        .storage_slice()
        .first()
        .copied()
        .filter(|first| {
            raw.black_levels_per_pixel
                .storage_slice()
                .iter()
                .all(|value| value == first)
        });
    let mut black_levels_per_pixel = uniform_black
        .is_none()
        .then(|| vec![0.0f32; raw.black_levels_per_pixel.len()]);
    let coordinate_enabled =
        flags & (LF_MODIFY_DISTORTION | LF_MODIFY_GEOMETRY | LF_MODIFY_TCA | LF_MODIFY_SCALE) != 0;
    let vignette_enabled = flags & LF_MODIFY_VIGNETTING != 0;
    const ROW_BATCH: usize = 32;
    let coordinate_row_len = width.saturating_mul(6);
    let mut coordinates = vec![0.0f32; coordinate_row_len.saturating_mul(ROW_BATCH)];
    let vignette_started = std::time::Instant::now();
    let vignette_gains = if vignette_enabled {
        build_vignette_gain_map(raw, modifier)?
    } else {
        Vec::new()
    };
    if vignette_enabled {
        crate::diagnostics::record(format!(
            "Lensfun vignette gain map prepared in {:.3}s",
            vignette_started.elapsed().as_secs_f64()
        ));
    }

    let warp_started = std::time::Instant::now();
    for batch_y in (0..height).step_by(ROW_BATCH) {
        let batch_rows = (height - batch_y).min(ROW_BATCH);
        let coordinate_len = batch_rows.saturating_mul(coordinate_row_len);
        let coordinate_batch = &mut coordinates[..coordinate_len];
        if coordinate_enabled {
            fill_source_coordinates(modifier, raw.width, batch_y, batch_rows, coordinate_batch);
        } else {
            fill_identity_rows(coordinate_batch, batch_y, batch_rows, width);
        }
        let coordinate_batch = &*coordinate_batch;

        let pixel_start = batch_y * width;
        let pixel_end = pixel_start + batch_rows * width;
        // Returns the corrected sample (rounded and clamped to u16) and its black level.
        let correct_sample = |local_index: usize| {
            let local_y = local_index / width;
            let x = local_index % width;
            let y = batch_y + local_y;
            let output_index = pixel_start + local_index;
            let cfa_index = raw.color_indices[output_index];
            let channel = lensfun_rgb_channel(cfa_index);
            let coordinate_index = local_y * coordinate_row_len + x * 6 + channel * 2;
            let source_x = coordinate_batch[coordinate_index];
            let source_y = coordinate_batch[coordinate_index + 1];
            let (corrected, black) = sample_corrected_cfa_subpixel(
                CfaCorrectionContext {
                    raw,
                    vignette_enabled,
                    vignette_gains: &vignette_gains,
                },
                CfaSample {
                    position: [source_x, source_y],
                    channel: cfa_index,
                    output: [x, y],
                },
            );
            (
                corrected.round().clamp(0.0, f32::from(u16::MAX)) as u16,
                black,
            )
        };
        if let Some(output_black_map) = black_levels_per_pixel.as_mut() {
            raw_pixels[pixel_start..pixel_end]
                .par_iter_mut()
                .zip(output_black_map[pixel_start..pixel_end].par_iter_mut())
                .enumerate()
                .for_each(|(local_index, (output_sample, output_black))| {
                    (*output_sample, *output_black) = correct_sample(local_index);
                });
        } else {
            raw_pixels[pixel_start..pixel_end]
                .par_iter_mut()
                .enumerate()
                .for_each(|(local_index, output_sample)| {
                    *output_sample = correct_sample(local_index).0;
                });
        }
    }
    crate::diagnostics::record(format!(
        "Lensfun CFA TCA/shading correction finished in {:.3}s",
        warp_started.elapsed().as_secs_f64()
    ));

    let black_levels_per_pixel = if let Some(black) = uniform_black {
        CompactPixelMap::repeating(raw.width, raw.height, 1, 1, vec![black])
    } else {
        CompactPixelMap::compact_from_dense(
            raw.width,
            raw.height,
            black_levels_per_pixel.expect("non-uniform black map must be materialized"),
            64,
        )
    };
    // The corrected mosaic replaces the source as the full-sensor reference, so it starts its
    // own opposed-chroma cache and source identity instead of sharing the uncorrected ones.
    Ok(LoadedRaw {
        lens_geometry,
        opposed_chroma_cache: Default::default(),
        opposed_chroma_source_identity: Default::default(),
        opposed_chroma_reference_source: true,
        ..raw.derive_with(
            raw.width,
            raw.height,
            raw_pixels,
            raw.color_indices.clone(),
            black_levels_per_pixel,
        )
    })
}

fn correct_raster(
    raw: &LoadedRaw,
    modifier: &Modifier,
    flags: c_int,
    lens_geometry: Option<Arc<LensGeometryMap>>,
) -> Result<LoadedRaw> {
    let width = raw.width as usize;
    let height = raw.height as usize;
    let source = raw
        .scene_linear_raster()
        .ok_or_else(|| anyhow!("rendered TIFF is missing its scene-linear RGB raster"))?;
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or_else(|| anyhow!("rendered TIFF dimensions overflow"))?;
    if source.len() != expected {
        return Err(anyhow!(
            "rendered TIFF RGB buffer has {} values, expected {expected}",
            source.len()
        ));
    }

    let vignette_enabled = flags & LF_MODIFY_VIGNETTING != 0;
    let coordinate_enabled = flags & LF_MODIFY_TCA != 0;
    let mut shaded = source.to_vec();
    if vignette_enabled {
        apply_raster_vignette(raw, modifier, &mut shaded)?;
    }

    let output = if coordinate_enabled {
        let mut output = vec![0.0f32; expected];
        const ROW_BATCH: usize = 32;
        let coordinate_row_len = width.saturating_mul(6);
        let mut coordinates = vec![0.0f32; coordinate_row_len.saturating_mul(ROW_BATCH)];

        for batch_y in (0..height).step_by(ROW_BATCH) {
            let batch_rows = (height - batch_y).min(ROW_BATCH);
            let coordinate_len = batch_rows.saturating_mul(coordinate_row_len);
            let coordinate_batch = &mut coordinates[..coordinate_len];
            fill_source_coordinates(modifier, raw.width, batch_y, batch_rows, coordinate_batch);

            let pixel_start = batch_y * width;
            let pixel_end = pixel_start + batch_rows * width;
            output[pixel_start * 3..pixel_end * 3]
                .par_chunks_exact_mut(3)
                .enumerate()
                .for_each(|(local_index, pixel)| {
                    let local_y = local_index / width;
                    let x = local_index % width;
                    let coordinate_base = local_y * coordinate_row_len + x * 6;
                    for (channel, value) in pixel.iter_mut().enumerate() {
                        let coordinate_index = coordinate_base + channel * 2;
                        let source_x = coordinate_batch[coordinate_index];
                        let source_y = coordinate_batch[coordinate_index + 1];
                        *value = sample_raster_channel_bilinear(
                            &shaded,
                            [width, height],
                            RasterChannelSample {
                                position: [source_x, source_y],
                                fallback: [x, batch_y + local_y],
                                channel,
                            },
                        );
                    }
                });
        }
        output
    } else {
        shaded
    };

    let mut corrected = raw.clone();
    corrected.scene_linear_raster = Some(Arc::from(output));
    corrected.lens_geometry = lens_geometry;
    corrected.ai_denoised = Arc::new(std::sync::RwLock::new(None));
    corrected.opposed_chroma_cache = Default::default();
    Ok(corrected)
}

fn apply_raster_vignette(raw: &LoadedRaw, modifier: &Modifier, raster: &mut [f32]) -> Result<()> {
    for_each_vignette_gain_batch(
        raw,
        modifier,
        "Lensfun TIFF vignette row stride overflow",
        |pixel_start, rgba_batch| {
            raster[pixel_start * 3..(pixel_start + rgba_batch.len()) * 3]
                .par_chunks_exact_mut(3)
                .zip(rgba_batch.par_iter())
                .for_each(|(pixel, rgba)| {
                    pixel[0] *= rgba.0[0];
                    pixel[1] *= rgba.0[1];
                    pixel[2] *= rgba.0[2];
                });
        },
    )
}

#[derive(Clone, Copy)]
struct RasterChannelSample {
    pub(super) position: [f32; 2],
    fallback: [usize; 2],
    pub(super) channel: usize,
}

fn sample_raster_channel_bilinear(
    source: &[f32],
    dimensions: [usize; 2],
    sample: RasterChannelSample,
) -> f32 {
    let [width, height] = dimensions;
    let RasterChannelSample {
        position: [x, y],
        fallback: [fallback_x, fallback_y],
        channel,
    } = sample;
    if !x.is_finite() || !y.is_finite() || width == 0 || height == 0 {
        return source[(fallback_y * width + fallback_x) * 3 + channel];
    }
    let x = x.clamp(0.0, width.saturating_sub(1) as f32);
    let y = y.clamp(0.0, height.saturating_sub(1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let sample = |sx: usize, sy: usize| source[(sy * width + sx) * 3 + channel];
    let top = sample(x0, y0) * (1.0 - tx) + sample(x1, y0) * tx;
    let bottom = sample(x0, y1) * (1.0 - tx) + sample(x1, y1) * tx;
    top * (1.0 - ty) + bottom * ty
}

fn build_vignette_gain_map(raw: &LoadedRaw, modifier: &Modifier) -> Result<Vec<f32>> {
    let mut gains = vec![1.0f32; raw.raw_pixels.len()];
    for_each_vignette_gain_batch(
        raw,
        modifier,
        "Lensfun vignette row stride overflow",
        |pixel_start, rgba_batch| {
            let pixel_end = pixel_start + rgba_batch.len();
            gains[pixel_start..pixel_end]
                .par_iter_mut()
                .zip(rgba_batch.par_iter())
                .enumerate()
                .for_each(|(local_index, (gain, rgba_gain))| {
                    let index = pixel_start + local_index;
                    let channel = lensfun_rgb_channel(raw.color_indices[index]);
                    *gain = rgba_gain.0[channel];
                });
        },
    )?;
    Ok(gains)
}

/// Evaluates Lensfun's vignetting model in batches of image rows and calls
/// `apply(pixel_start, gains)` with each batch's row-major RGBA gains (unity before Lensfun
/// applies its model), where `pixel_start` is the batch's first pixel index in the image.
fn for_each_vignette_gain_batch(
    raw: &LoadedRaw,
    modifier: &Modifier,
    stride_overflow_context: &'static str,
    mut apply: impl FnMut(usize, &[AlignedRgba]),
) -> Result<()> {
    let width = raw.width as usize;
    let height = raw.height as usize;
    let row_stride = c_int::try_from(width.saturating_mul(std::mem::size_of::<AlignedRgba>()))
        .context(stride_overflow_context)?;
    const ROW_BATCH: usize = 32;
    let mut rgba_gains = vec![AlignedRgba([1.0; 4]); width.saturating_mul(ROW_BATCH)];

    for batch_y in (0..height).step_by(ROW_BATCH) {
        let batch_rows = (height - batch_y).min(ROW_BATCH);
        let batch_len = batch_rows * width;
        let rgba_batch = &mut rgba_gains[..batch_len];
        rgba_batch.fill(AlignedRgba([1.0; 4]));
        // SAFETY: `modifier` holds a live, initialized Lensfun modifier. `rgba_batch` is
        // `batch_rows` rows of `width` contiguous RGBA f32 pixels and `row_stride` is exactly
        // one such row in bytes, so Lensfun stays inside the batch.
        let _applied = unsafe {
            lf_modifier_apply_color_modification(
                modifier.0,
                rgba_batch.as_mut_ptr().cast(),
                0.0,
                batch_y as f32,
                raw.width as c_int,
                batch_rows as c_int,
                LF_CR_RGBA,
                row_stride,
            )
        };
        apply(batch_y * width, rgba_batch);
    }
    Ok(())
}

fn lensfun_rgb_channel(cfa_index: u8) -> usize {
    match cfa_index {
        0 => 0,
        2 => 2,
        _ => 1,
    }
}

/// Fills `coordinate_batch` with Lensfun's per-channel source positions for `batch_rows` image
/// rows starting at `batch_y`: per pixel, `[x, y]` pairs for R, G and B (6 floats), in source
/// pixel coordinates. Falls back to identity coordinates when Lensfun reports no mapping.
fn fill_source_coordinates(
    modifier: &Modifier,
    image_width: u32,
    batch_y: usize,
    batch_rows: usize,
    coordinate_batch: &mut [f32],
) {
    debug_assert_eq!(
        coordinate_batch.len(),
        batch_rows * image_width as usize * 6
    );
    // SAFETY: `modifier` holds a live, initialized Lensfun modifier. Callers pass a batch of
    // exactly `batch_rows * image_width * 6` floats, which is what Lensfun writes for
    // `batch_rows` rows of `image_width` pixels of subpixel (3 channel x/y) coordinates.
    let filled = unsafe {
        lf_modifier_apply_subpixel_geometry_distortion(
            modifier.0,
            0.0,
            batch_y as f32,
            image_width as c_int,
            batch_rows as c_int,
            coordinate_batch.as_mut_ptr(),
        )
    };
    if filled == 0 {
        fill_identity_rows(coordinate_batch, batch_y, batch_rows, image_width as usize);
    }
}

/// Fills `batch_rows` rows of per-channel coordinates, starting at image row `batch_y`, with
/// each pixel's own position.
fn fill_identity_rows(
    coordinate_batch: &mut [f32],
    batch_y: usize,
    batch_rows: usize,
    width: usize,
) {
    let coordinate_row_len = width.saturating_mul(6);
    for local_y in 0..batch_rows {
        let y = batch_y + local_y;
        let row_coordinates =
            &mut coordinate_batch[local_y * coordinate_row_len..(local_y + 1) * coordinate_row_len];
        fill_identity_coordinates(row_coordinates, y, width);
    }
}

fn fill_identity_coordinates(coordinates: &mut [f32], y: usize, width: usize) {
    for x in 0..width {
        let base = x * 6;
        for channel in 0..3 {
            coordinates[base + channel * 2] = x as f32;
            coordinates[base + channel * 2 + 1] = y as f32;
        }
    }
}
