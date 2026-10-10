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
    // Returns the corrected sample (rounded and clamped to u16) and its black level.
    let correct_sample = |batch: RowBatch, coordinates: &[f32], local_index: usize| {
        let local_y = local_index / width;
        let x = local_index % width;
        let y = batch.y + local_y;
        let output_index = y * width + x;
        let cfa_index = raw.color_indices[output_index];
        let channel = lensfun_rgb_channel(cfa_index);
        let coordinate_index = local_y * width * 6 + x * 6 + channel * 2;
        let source_x = coordinates[coordinate_index];
        let source_y = coordinates[coordinate_index + 1];
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
    let coordinate_modifier = coordinate_enabled.then_some(modifier);
    let batch_len = ROW_BATCH * width.max(1);
    if let Some(output_black_map) = black_levels_per_pixel.as_mut() {
        for_each_coordinate_batch(
            raw_pixels
                .par_chunks_mut(batch_len)
                .zip(output_black_map.par_chunks_mut(batch_len)),
            coordinate_modifier,
            [width, height],
            |(samples, blacks), batch, coordinates| {
                for (local_index, (sample, black)) in
                    samples.iter_mut().zip(blacks.iter_mut()).enumerate()
                {
                    (*sample, *black) = correct_sample(batch, coordinates, local_index);
                }
            },
        );
    } else {
        for_each_coordinate_batch(
            raw_pixels.par_chunks_mut(batch_len),
            coordinate_modifier,
            [width, height],
            |samples, batch, coordinates| {
                for (local_index, sample) in samples.iter_mut().enumerate() {
                    *sample = correct_sample(batch, coordinates, local_index).0;
                }
            },
        );
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
        for_each_coordinate_batch(
            output.par_chunks_mut(ROW_BATCH * width.max(1) * 3),
            Some(modifier),
            [width, height],
            |pixels, batch, coordinates| {
                for (local_index, pixel) in pixels.chunks_exact_mut(3).enumerate() {
                    let local_y = local_index / width;
                    let x = local_index % width;
                    let coordinate_base = local_y * width * 6 + x * 6;
                    for (channel, value) in pixel.iter_mut().enumerate() {
                        let coordinate_index = coordinate_base + channel * 2;
                        *value = sample_raster_channel_bilinear(
                            &shaded,
                            [width, height],
                            RasterChannelSample {
                                position: [
                                    coordinates[coordinate_index],
                                    coordinates[coordinate_index + 1],
                                ],
                                fallback: [x, batch.y + local_y],
                                channel,
                            },
                        );
                    }
                }
            },
        );
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
    let batch_len = ROW_BATCH * (raw.width as usize).max(1) * 3;
    for_each_vignette_batch(
        raster.par_chunks_mut(batch_len),
        raw,
        modifier,
        "Lensfun TIFF vignette row stride overflow",
        |pixels, _, gains| {
            for (pixel, gain) in pixels.chunks_exact_mut(3).zip(gains) {
                pixel[0] *= gain.0[0];
                pixel[1] *= gain.0[1];
                pixel[2] *= gain.0[2];
            }
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
    let width = raw.width as usize;
    let mut gains = vec![1.0f32; raw.raw_pixels.len()];
    for_each_vignette_batch(
        gains.par_chunks_mut(ROW_BATCH * width.max(1)),
        raw,
        modifier,
        "Lensfun vignette row stride overflow",
        |gains, batch, rgba_gains| {
            let pixel_start = batch.y * width;
            for (local_index, (gain, rgba_gain)) in gains.iter_mut().zip(rgba_gains).enumerate() {
                let channel = lensfun_rgb_channel(raw.color_indices[pixel_start + local_index]);
                *gain = rgba_gain.0[channel];
            }
        },
    )?;
    Ok(gains)
}

/// Image rows per Lensfun call. Batches are independent, so they run in
/// parallel; each rayon worker reuses one scratch buffer for its batches.
const ROW_BATCH: usize = 32;

/// `rows` consecutive image rows starting at row `y`.
#[derive(Clone, Copy)]
struct RowBatch {
    y: usize,
    rows: usize,
}

impl RowBatch {
    /// The `index`-th batch of an image `height` rows tall.
    fn nth(index: usize, height: usize) -> Self {
        let y = index * ROW_BATCH;
        Self {
            y,
            rows: height.saturating_sub(y).min(ROW_BATCH),
        }
    }
}

/// Runs `process(output, batch, coordinates)` in parallel for every
/// `ROW_BATCH`-row batch of a `width`×`height` image. `outputs` yields one item
/// per batch, in row order. `coordinates` holds the batch's per-channel source
/// positions: per pixel, `[x, y]` pairs for R, G and B (6 floats), from
/// `modifier`, or each pixel's own position when it is `None`.
fn for_each_coordinate_batch<T: Send>(
    outputs: impl IndexedParallelIterator<Item = T>,
    modifier: Option<&Modifier>,
    [width, height]: [usize; 2],
    process: impl Fn(T, RowBatch, &[f32]) + Sync + Send,
) {
    let row_len = width.saturating_mul(6);
    outputs.enumerate().for_each_init(
        || vec![0.0f32; row_len.saturating_mul(ROW_BATCH)],
        |scratch, (index, output)| {
            let batch = RowBatch::nth(index, height);
            let coordinates = &mut scratch[..batch.rows * row_len];
            match modifier {
                Some(modifier) => fill_source_coordinates(
                    modifier,
                    width as u32,
                    batch.y,
                    batch.rows,
                    coordinates,
                ),
                None => fill_identity_rows(coordinates, batch.y, batch.rows, width),
            }
            process(output, batch, coordinates);
        },
    );
}

/// Runs `process(output, batch, gains)` in parallel for every `ROW_BATCH`-row
/// batch of `raw`. `outputs` yields one item per batch, in row order. `gains`
/// holds Lensfun's row-major RGBA vignetting gains for the batch's pixels,
/// unity where its model does not apply.
fn for_each_vignette_batch<T: Send>(
    outputs: impl IndexedParallelIterator<Item = T>,
    raw: &LoadedRaw,
    modifier: &Modifier,
    stride_overflow_context: &'static str,
    process: impl Fn(T, RowBatch, &[AlignedRgba]) + Sync + Send,
) -> Result<()> {
    let width = raw.width as usize;
    let height = raw.height as usize;
    let row_stride = c_int::try_from(width.saturating_mul(std::mem::size_of::<AlignedRgba>()))
        .context(stride_overflow_context)?;
    outputs.enumerate().for_each_init(
        || vec![AlignedRgba([1.0; 4]); width.saturating_mul(ROW_BATCH)],
        |scratch, (index, output)| {
            let batch = RowBatch::nth(index, height);
            let gains = &mut scratch[..batch.rows * width];
            gains.fill(AlignedRgba([1.0; 4]));
            // SAFETY: `modifier` holds a live, initialized Lensfun modifier, which is safe to
            // apply from several threads (see `Modifier`). `gains` is `batch.rows` rows of
            // `width` contiguous RGBA f32 pixels and `row_stride` is exactly one such row in
            // bytes, so Lensfun stays inside this batch.
            let _applied = unsafe {
                lf_modifier_apply_color_modification(
                    modifier.0,
                    gains.as_mut_ptr().cast(),
                    0.0,
                    batch.y as f32,
                    raw.width as c_int,
                    batch.rows as c_int,
                    LF_CR_RGBA,
                    row_stride,
                )
            };
            process(output, batch, gains);
        },
    );
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
