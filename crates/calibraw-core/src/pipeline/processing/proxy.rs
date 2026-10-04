//! Reduced and cropped proxies of a RAW for previews and regions.

use super::*;

#[derive(Clone, Copy, Debug)]
pub struct ProxySpec {
    pub max_edge: u32,
}

impl Default for ProxySpec {
    fn default() -> Self {
        Self {
            max_edge: if cfg!(target_os = "android") {
                1280
            } else {
                2048
            },
        }
    }
}

fn raster_with_metadata(
    raw: &LoadedRaw,
    width: u32,
    height: u32,
    rgb: Vec<f32>,
    preserve_lens_geometry: bool,
) -> LoadedRaw {
    let mut output = LoadedRaw::from_scene_linear_rec2020(width, height, rgb)
        .expect("validated raster dimensions must remain valid");
    output.camera_make = raw.camera_make.clone();
    output.camera_model = raw.camera_model.clone();
    output.lens_make = raw.lens_make.clone();
    output.lens_model = raw.lens_model.clone();
    output.focal_length = raw.focal_length;
    output.aperture = raw.aperture;
    output.focus_distance = raw.focus_distance;
    output.capture_metadata = raw.capture_metadata.clone();
    output.wb_coeffs = raw.wb_coeffs;
    output.cam_to_srgb = raw.cam_to_srgb;
    output.black_levels = raw.black_levels;
    output.white_levels = raw.white_levels;
    output.white_balance_model = raw.white_balance_model.clone();
    output.cfa_kind = raw.cfa_kind;
    output.noise_profile = raw.noise_profile;
    output.camera_profile = raw.camera_profile.clone();
    output.camera_profile_source = raw.camera_profile_source.clone();
    output.available_camera_profiles = raw.available_camera_profiles.clone();
    output.lens_geometry = preserve_lens_geometry
        .then(|| raw.lens_geometry.clone())
        .flatten();
    output.opposed_chroma_cache = std::sync::Arc::clone(&raw.opposed_chroma_cache);
    output
}

fn crop_raster(raw: &LoadedRaw, x: u32, y: u32, width: u32, height: u32) -> LoadedRaw {
    let x = x.min(raw.width.saturating_sub(1));
    let y = y.min(raw.height.saturating_sub(1));
    let width = width.max(1).min(raw.width - x);
    let height = height.max(1).min(raw.height - y);
    let source = raw
        .scene_linear_raster()
        .expect("raster source flag requires RGB pixels");
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    let source_width = raw.width as usize;
    for row in y..y + height {
        let start = (row as usize * source_width + x as usize) * 3;
        let end = start + width as usize * 3;
        rgb.extend_from_slice(&source[start..end]);
    }
    raster_with_metadata(
        raw,
        width,
        height,
        rgb,
        x == 0 && y == 0 && width == raw.width && height == raw.height,
    )
}

fn build_raster_region_proxy(
    raw: &LoadedRaw,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    spec: ProxySpec,
) -> LoadedRaw {
    let x = x.min(raw.width.saturating_sub(1));
    let y = y.min(raw.height.saturating_sub(1));
    let region_width = width.max(1).min(raw.width - x);
    let region_height = height.max(1).min(raw.height - y);
    let max_edge = spec.max_edge.max(1);
    let longest = region_width.max(region_height);
    if longest <= max_edge {
        return crop_raster(raw, x, y, region_width, region_height);
    }

    let scale = max_edge as f64 / longest as f64;
    let output_width = (region_width as f64 * scale).round().max(1.0) as u32;
    let output_height = (region_height as f64 * scale).round().max(1.0) as u32;
    let source = raw
        .scene_linear_raster()
        .expect("raster source flag requires RGB pixels");
    let source_width = raw.width as usize;
    let mut rgb = vec![0.0f32; output_width as usize * output_height as usize * 3];
    rgb.par_chunks_mut(output_width as usize * 3)
        .enumerate()
        .for_each(|(output_y, row)| {
            let (sy0, sy1) = proportional_partition(output_y as u32, region_height, output_height);
            for output_x in 0..output_width {
                let (sx0, sx1) = proportional_partition(output_x, region_width, output_width);
                let mut sum = [0.0f64; 3];
                let mut count = 0u32;
                for sy in sy0..sy1 {
                    for sx in sx0..sx1 {
                        let index = ((y + sy) as usize * source_width + (x + sx) as usize) * 3;
                        sum[0] += f64::from(source[index]);
                        sum[1] += f64::from(source[index + 1]);
                        sum[2] += f64::from(source[index + 2]);
                        count += 1;
                    }
                }
                let destination = output_x as usize * 3;
                let divisor = f64::from(count.max(1));
                row[destination] = (sum[0] / divisor) as f32;
                row[destination + 1] = (sum[1] / divisor) as f32;
                row[destination + 2] = (sum[2] / divisor) as f32;
            }
        });
    raster_with_metadata(
        raw,
        output_width,
        output_height,
        rgb,
        x == 0 && y == 0 && region_width == raw.width && region_height == raw.height,
    )
}

pub(super) fn fill_padded_raster_pixels(raw: &LoadedRaw, tile: ExportTile, rgb: &mut [f32]) {
    let source = raw
        .scene_linear_raster()
        .expect("raster source flag requires RGB pixels");
    let width = tile.padded_width as usize;
    let expected = width
        .saturating_mul(tile.padded_height as usize)
        .saturating_mul(3);
    debug_assert_eq!(rgb.len(), expected);
    let max_x = i64::from(raw.width.saturating_sub(1));
    let max_y = i64::from(raw.height.saturating_sub(1));
    let source_width = raw.width as usize;
    for local_y in 0..tile.padded_height {
        let source_y =
            (i64::from(tile.global_origin_y) + i64::from(local_y)).clamp(0, max_y) as usize;
        for local_x in 0..tile.padded_width {
            let source_x =
                (i64::from(tile.global_origin_x) + i64::from(local_x)).clamp(0, max_x) as usize;
            let source_index = (source_y * source_width + source_x) * 3;
            let destination = (local_y as usize * width + local_x as usize) * 3;
            rgb[destination..destination + 3]
                .copy_from_slice(&source[source_index..source_index + 3]);
        }
    }
}

pub(super) fn extract_padded_raster_tile(raw: &LoadedRaw, tile: ExportTile) -> LoadedRaw {
    let width = tile.padded_width as usize;
    let height = tile.padded_height as usize;
    let mut rgb = vec![0.0f32; width.saturating_mul(height).saturating_mul(3)];
    fill_padded_raster_pixels(raw, tile, &mut rgb);
    raster_with_metadata(raw, tile.padded_width, tile.padded_height, rgb, false)
}

pub fn crop_raw(raw: &LoadedRaw, x: u32, y: u32, width: u32, height: u32) -> LoadedRaw {
    if raw.is_pre_demosaiced_raster() {
        return crop_raster(raw, x, y, width, height);
    }
    let x = x.min(raw.width.saturating_sub(1));
    let y = y.min(raw.height.saturating_sub(1));
    let width = width.max(1).min(raw.width - x);
    let height = height.max(1).min(raw.height - y);
    let mut raw_pixels = Vec::with_capacity((width * height) as usize);
    for row in y..y + height {
        let start = (row * raw.width + x) as usize;
        let end = start + width as usize;
        raw_pixels.extend_from_slice(&raw.raw_pixels[start..end]);
    }
    let color_indices =
        raw.color_indices
            .subregion_clamped(i64::from(x), i64::from(y), width, height);
    let black_levels_per_pixel =
        raw.black_levels_per_pixel
            .subregion_clamped(i64::from(x), i64::from(y), width, height);

    LoadedRaw {
        width,
        height,
        camera_make: raw.camera_make.clone(),
        camera_model: raw.camera_model.clone(),
        lens_make: raw.lens_make.clone(),
        lens_model: raw.lens_model.clone(),
        focal_length: raw.focal_length,
        aperture: raw.aperture,
        focus_distance: raw.focus_distance,
        capture_metadata: raw.capture_metadata.clone(),
        cfa_kind: raw.cfa_kind,
        raw_pixels,
        scene_linear_raster: None,
        color_indices,
        wb_coeffs: raw.wb_coeffs,
        cam_to_srgb: raw.cam_to_srgb,
        black_levels: raw.black_levels,
        black_levels_per_pixel,
        white_levels: raw.white_levels,
        noise_profile: raw.noise_profile,
        camera_profile: raw.camera_profile.clone(),
        camera_profile_source: raw.camera_profile_source.clone(),
        available_camera_profiles: raw.available_camera_profiles.clone(),
        white_balance_model: raw.white_balance_model.clone(),
        lens_geometry: (x == 0 && y == 0 && width == raw.width && height == raw.height)
            .then(|| raw.lens_geometry.clone())
            .flatten(),
        ai_denoised: std::sync::Arc::new(std::sync::RwLock::new(crop_ai_denoised(
            raw, x, y, width, height,
        ))),
        opposed_chroma_cache: std::sync::Arc::clone(&raw.opposed_chroma_cache),
        opposed_chroma_source_identity: std::sync::Arc::clone(&raw.opposed_chroma_source_identity),
        opposed_chroma_reference_source: false,
    }
}

pub fn build_proxy(raw: &LoadedRaw, spec: ProxySpec) -> LoadedRaw {
    build_region_proxy(raw, 0, 0, raw.width, raw.height, spec)
}

/// Builds a bounded, interactive approximation of a source region.
///
/// For sensor RAW inputs this averages samples of the same CFA colour before
/// highlight reconstruction, demosaic, and denoise.  It is consequently not a
/// fidelity reference: averaging can hide clipped samples, erase sub-footprint
/// colour structure, and reduce the noise seen by RAW-domain processing.  Use a
/// native crop (or native tiles) and resize the developed linear result for a
/// settled/final comparison.
pub fn build_region_proxy(
    raw: &LoadedRaw,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    spec: ProxySpec,
) -> LoadedRaw {
    if raw.is_pre_demosaiced_raster() {
        return build_raster_region_proxy(raw, x, y, width, height, spec);
    }
    let x = x.min(raw.width.saturating_sub(1));
    let y = y.min(raw.height.saturating_sub(1));
    let region_width = width.max(1).min(raw.width - x);
    let region_height = height.max(1).min(raw.height - y);
    let max_edge = spec.max_edge.max(1);
    let longest = region_width.max(region_height);
    if longest <= max_edge {
        if x == 0 && y == 0 && region_width == raw.width && region_height == raw.height {
            return raw.clone();
        }
        return crop_raw(raw, x, y, region_width, region_height);
    }

    let cfa_period = match raw.cfa_kind {
        crate::pipeline::CfaKind::Bayer => 2,
        crate::pipeline::CfaKind::XTrans => 6,
    };
    let target_scale = max_edge as f64 / longest as f64;
    let target_width = (region_width as f64 * target_scale).floor().max(1.0) as u32;
    let target_height = (region_height as f64 * target_scale).floor().max(1.0) as u32;
    let phase_aligned_dimension = |source: u32, target: u32| {
        if target >= source {
            source
        } else {
            target
                .div_euclid(cfa_period)
                .max(1)
                .saturating_mul(cfa_period)
                .min(source)
        }
    };
    let width = phase_aligned_dimension(region_width, target_width);
    let height = phase_aligned_dimension(region_height, target_height);
    let len = (width * height) as usize;
    let row_stride = width as usize;
    let mut raw_pixels = vec![0u16; len];
    let mut color_indices = vec![0u8; len];
    let mut black_levels_per_pixel = vec![0.0f32; len];
    raw_pixels
        .par_chunks_mut(row_stride)
        .zip(color_indices.par_chunks_mut(row_stride))
        .zip(black_levels_per_pixel.par_chunks_mut(row_stride))
        .enumerate()
        .for_each(|(py, ((raw_row, cfa_row), black_row))| {
            let py = py as u32;
            let output_phase_y = py % cfa_period;
            let (source_y0, source_y1) = proportional_partition(py, region_height, height);
            let footprint_y0 = y + source_y0;
            let footprint_y1 = (y + source_y1).min(y + region_height);

            for px in 0..width {
                let output_phase_x = px % cfa_period;
                let (source_x0, source_x1) = proportional_partition(px, region_width, width);
                let footprint_x0 = x + source_x0;
                let footprint_x1 = (x + source_x1).min(x + region_width);
                let center_x = (footprint_x0 + (footprint_x1.saturating_sub(footprint_x0)) / 2)
                    .min(raw.width - 1);
                let center_y = (footprint_y0 + (footprint_y1.saturating_sub(footprint_y0)) / 2)
                    .min(raw.height - 1);

                let phase_x = (x + output_phase_x).min(raw.width - 1);
                let phase_y = (y + output_phase_y).min(raw.height - 1);
                let phase_index = (phase_y * raw.width + phase_x) as usize;
                let cfa = raw.color_indices[phase_index];

                let mut pixel_sum = 0u64;
                let mut black_sum = 0.0f64;
                let mut count = 0u32;

                for sy in footprint_y0..footprint_y1 {
                    let row = sy * raw.width;
                    for sx in footprint_x0..footprint_x1 {
                        let index = (row + sx) as usize;
                        if raw.color_indices[index] == cfa {
                            pixel_sum += u64::from(raw.raw_pixels[index]);
                            black_sum += f64::from(raw.black_levels_per_pixel[index]);
                            count += 1;
                        }
                    }
                }

                let px = px as usize;
                if count == 0 {
                    let fallback = nearest_cfa_sample(raw, center_x, center_y, cfa, cfa_period);
                    raw_row[px] = raw.raw_pixels[fallback];
                    black_row[px] = raw.black_levels_per_pixel[fallback];
                } else {
                    raw_row[px] = (pixel_sum / u64::from(count)) as u16;
                    black_row[px] = (black_sum / f64::from(count)) as f32;
                }
                cfa_row[px] = cfa;
            }
        });

    LoadedRaw {
        width,
        height,
        camera_make: raw.camera_make.clone(),
        camera_model: raw.camera_model.clone(),
        lens_make: raw.lens_make.clone(),
        lens_model: raw.lens_model.clone(),
        focal_length: raw.focal_length,
        aperture: raw.aperture,
        focus_distance: raw.focus_distance,
        capture_metadata: raw.capture_metadata.clone(),
        cfa_kind: raw.cfa_kind,
        raw_pixels,
        scene_linear_raster: None,
        color_indices: CompactPixelMap::compact_from_dense(width, height, color_indices, 64),
        wb_coeffs: raw.wb_coeffs,
        cam_to_srgb: raw.cam_to_srgb,
        black_levels: raw.black_levels,
        black_levels_per_pixel: CompactPixelMap::compact_from_dense(
            width,
            height,
            black_levels_per_pixel,
            64,
        ),
        white_levels: raw.white_levels,
        noise_profile: raw.noise_profile.scaled_variance(
            ((u64::from(width) * u64::from(height)) as f64
                / (u64::from(region_width) * u64::from(region_height)) as f64)
                .clamp(0.0, 1.0) as f32,
        ),
        camera_profile: raw.camera_profile.clone(),
        camera_profile_source: raw.camera_profile_source.clone(),
        available_camera_profiles: raw.available_camera_profiles.clone(),
        white_balance_model: raw.white_balance_model.clone(),
        lens_geometry: (x == 0
            && y == 0
            && region_width == raw.width
            && region_height == raw.height)
            .then(|| raw.lens_geometry.clone())
            .flatten(),
        ai_denoised: std::sync::Arc::new(std::sync::RwLock::new(proxy_ai_denoised(
            raw,
            x,
            y,
            region_width,
            region_height,
            width,
            height,
        ))),
        opposed_chroma_cache: std::sync::Arc::clone(&raw.opposed_chroma_cache),
        opposed_chroma_source_identity: std::sync::Arc::clone(&raw.opposed_chroma_source_identity),
        opposed_chroma_reference_source: false,
    }
}

pub(super) fn nearest_cfa_sample(
    raw: &LoadedRaw,
    center_x: u32,
    center_y: u32,
    cfa: u8,
    search_radius: u32,
) -> usize {
    let max_x = raw.width.saturating_sub(1) as i64;
    let max_y = raw.height.saturating_sub(1) as i64;
    for radius in 0..=search_radius.max(1) {
        let r = i64::from(radius);
        for dy in -r..=r {
            for dx in -r..=r {
                if radius > 0 && dx.abs() != r && dy.abs() != r {
                    continue;
                }
                let x = (i64::from(center_x) + dx).clamp(0, max_x) as u32;
                let y = (i64::from(center_y) + dy).clamp(0, max_y) as u32;
                let index = (y * raw.width + x) as usize;
                if raw.color_indices[index] == cfa {
                    return index;
                }
            }
        }
    }

    (center_y * raw.width + center_x) as usize
}
