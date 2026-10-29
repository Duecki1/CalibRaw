//! Reduced and cropped proxies of a RAW for previews and regions.

use super::binning::{bin_mosaic, BinnedMosaic};
use super::*;
use crate::pipeline::{AiDenoisedImage, BinnedSensor};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

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
    output.source_extent = raw.source_extent;
    output.camera_make = raw.camera_make.clone();
    output.camera_model = raw.camera_model.clone();
    output.lens_make = raw.lens_make.clone();
    output.lens_model = raw.lens_model.clone();
    output.focal_length = raw.focal_length;
    output.aperture = raw.aperture;
    output.focus_distance = raw.focus_distance;
    output.capture_metadata = raw.capture_metadata.clone();
    output.scene_referred_raster = raw.scene_referred_raster;
    output.wb_coeffs = raw.wb_coeffs;
    output.black_levels = raw.black_levels;
    output.white_levels = raw.white_levels;
    output.cfa_kind = raw.cfa_kind;
    output.noise_profile = raw.noise_profile;
    output.profile_color = Arc::clone(&raw.profile_color);
    output.color_source = raw.color_source.clone();
    output.available_camera_profiles = raw.available_camera_profiles.clone();
    output.lens_geometry = preserve_lens_geometry
        .then(|| raw.lens_geometry.clone())
        .flatten();
    output.lateral_chromatic_aberration = raw.lateral_chromatic_aberration;
    output.opposed_chroma_cache = Arc::clone(&raw.opposed_chroma_cache);
    output
}

/// A binned view of `raw`'s capture, `width` by `height`, with `binned` as its
/// pixels and `ai_denoised` as its X-Trans AI-denoised scene: the mosaic's
/// metadata with uniform placeholder CFA and black-level maps.
fn binned_with_metadata(
    raw: &LoadedRaw,
    [width, height]: [u32; 2],
    binned: BinnedSensor,
    ai_denoised: Option<AiDenoisedImage>,
    preserve_lens_geometry: bool,
) -> LoadedRaw {
    LoadedRaw {
        binned_sensor: Some(binned),
        lens_geometry: preserve_lens_geometry
            .then(|| raw.lens_geometry.clone())
            .flatten(),
        ai_denoised: Arc::new(RwLock::new(ai_denoised)),
        ..raw.derive_with(
            width,
            height,
            Vec::new(),
            CompactPixelMap::repeating(width, height, 1, 1, vec![1]),
            CompactPixelMap::repeating(width, height, 1, 1, vec![0.0]),
        )
    }
}

/// `[x, y, width, height]` limited to `raw`: the origin inside the image and
/// a size of at least one pixel that ends at its edge.
fn clamped_region(raw: &LoadedRaw, x: u32, y: u32, width: u32, height: u32) -> [u32; 4] {
    let x = x.min(raw.width.saturating_sub(1));
    let y = y.min(raw.height.saturating_sub(1));
    [
        x,
        y,
        width.max(1).min(raw.width - x),
        height.max(1).min(raw.height - y),
    ]
}

fn is_whole_image(raw: &LoadedRaw, [x, y, width, height]: [u32; 4]) -> bool {
    x == 0 && y == 0 && width == raw.width && height == raw.height
}

/// The size a `region_width` by `region_height` region takes when its longer
/// edge is reduced to `max_edge`.
fn reduced_size(region_width: u32, region_height: u32, max_edge: u32) -> [u32; 2] {
    let scale = f64::from(max_edge) / f64::from(region_width.max(region_height));
    [
        (f64::from(region_width) * scale).round().max(1.0) as u32,
        (f64::from(region_height) * scale).round().max(1.0) as u32,
    ]
}

/// The pixels of `region` in interleaved RGB rows `source_width` wide.
fn crop_rgb(source: &[f32], source_width: u32, [x, y, width, height]: [u32; 4]) -> Vec<f32> {
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for row in y..y + height {
        let start = (row as usize * source_width as usize + x as usize) * 3;
        rgb.extend_from_slice(&source[start..start + width as usize * 3]);
    }
    rgb
}

/// The box-filtered mean of `region` of interleaved RGB rows `source_width`
/// wide, `output` pixels in size: every source pixel belongs to exactly one
/// output pixel. `None` once `cancel` is set, which is checked every row.
fn average_rgb_region(
    source: &[f32],
    source_width: u32,
    [x, y, region_width, region_height]: [u32; 4],
    [output_width, output_height]: [u32; 2],
    cancel: &AtomicBool,
) -> Option<Vec<f32>> {
    let source_width = source_width as usize;
    let mut rgb = vec![0.0f32; output_width as usize * output_height as usize * 3];
    rgb.par_chunks_mut(output_width as usize * 3)
        .enumerate()
        .for_each(|(output_y, row)| {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
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
    (!cancel.load(Ordering::Relaxed)).then_some(rgb)
}

fn crop_raster(raw: &LoadedRaw, region: [u32; 4]) -> LoadedRaw {
    let source = raw
        .scene_linear_raster()
        .expect("raster source flag requires RGB pixels");
    let [_, _, width, height] = region;
    raster_with_metadata(
        raw,
        width,
        height,
        crop_rgb(source, raw.width, region),
        is_whole_image(raw, region),
    )
}

fn build_raster_region_proxy(
    raw: &LoadedRaw,
    region: [u32; 4],
    spec: ProxySpec,
    cancel: &AtomicBool,
) -> Option<LoadedRaw> {
    let [_, _, region_width, region_height] = region;
    let max_edge = spec.max_edge.max(1);
    if region_width.max(region_height) <= max_edge {
        return Some(crop_raster(raw, region));
    }
    let output = reduced_size(region_width, region_height, max_edge);
    let source = raw
        .scene_linear_raster()
        .expect("raster source flag requires RGB pixels");
    let rgb = average_rgb_region(source, raw.width, region, output, cancel)?;
    let mut proxy =
        raster_with_metadata(raw, output[0], output[1], rgb, is_whole_image(raw, region));
    // Box-filtered pixels average their footprint, so noise variance scales
    // with the area ratio, as for sensor proxies.
    proxy.noise_profile = raw
        .noise_profile
        .scaled_variance([area_ratio(output, [region_width, region_height]); 4]);
    Some(proxy)
}

fn crop_binned(raw: &LoadedRaw, binned: &BinnedSensor, region: [u32; 4]) -> LoadedRaw {
    let [x, y, width, height] = region;
    let crop = |rgb: &[f32]| crop_rgb(rgb, raw.width, region);
    binned_with_metadata(
        raw,
        [width, height],
        BinnedSensor::new(crop(binned.rgb()), binned.ai_rgb().map(crop)),
        crop_ai_denoised(raw, x, y, width, height),
        is_whole_image(raw, region),
    )
}

/// A binned view reduced further: its pixels already hold every colour, so
/// they are averaged like a raster's.
fn build_binned_region_proxy(
    raw: &LoadedRaw,
    binned: &BinnedSensor,
    region: [u32; 4],
    spec: ProxySpec,
    cancel: &AtomicBool,
) -> Option<LoadedRaw> {
    let [x, y, region_width, region_height] = region;
    let max_edge = spec.max_edge.max(1);
    if region_width.max(region_height) <= max_edge {
        return Some(crop_binned(raw, binned, region));
    }
    let output = reduced_size(region_width, region_height, max_edge);
    let [width, height] = output;
    let average = |rgb: &[f32]| average_rgb_region(rgb, raw.width, region, output, cancel);
    let rgb = average(binned.rgb())?;
    let ai_rgb = match binned.ai_rgb() {
        Some(ai) => Some(average(ai)?),
        None => None,
    };
    Some(LoadedRaw {
        noise_profile: raw
            .noise_profile
            .scaled_variance([area_ratio(output, [region_width, region_height]); 4]),
        ..binned_with_metadata(
            raw,
            output,
            BinnedSensor::new(rgb, ai_rgb),
            proxy_ai_denoised(raw, x, y, region_width, region_height, width, height),
            is_whole_image(raw, region),
        )
    })
}

/// The fraction of `region`'s area that `output` covers, at most 1.
fn area_ratio(output: [u32; 2], region: [u32; 2]) -> f32 {
    ((u64::from(output[0]) * u64::from(output[1])) as f64
        / (u64::from(region[0]) * u64::from(region[1])) as f64)
        .clamp(0.0, 1.0) as f32
}

/// Copies the padded `tile` of interleaved RGB rows `source_size[0]` wide
/// into `rgb`; positions outside the image take its nearest edge pixel.
pub(super) fn fill_padded_rgb(
    source: &[f32],
    source_size: [u32; 2],
    tile: ExportTile,
    rgb: &mut [f32],
) {
    let width = tile.padded_width as usize;
    let expected = width
        .saturating_mul(tile.padded_height as usize)
        .saturating_mul(3);
    debug_assert_eq!(rgb.len(), expected);
    let max_x = i64::from(source_size[0].saturating_sub(1));
    let max_y = i64::from(source_size[1].saturating_sub(1));
    let source_width = source_size[0] as usize;
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

/// The padded `tile` of interleaved RGB rows `source_size[0]` wide.
fn padded_rgb(source: &[f32], source_size: [u32; 2], tile: ExportTile) -> Vec<f32> {
    let mut rgb = vec![
        0.0f32;
        (tile.padded_width as usize)
            .saturating_mul(tile.padded_height as usize)
            .saturating_mul(3)
    ];
    fill_padded_rgb(source, source_size, tile, &mut rgb);
    rgb
}

pub(super) fn extract_padded_raster_tile(raw: &LoadedRaw, tile: ExportTile) -> LoadedRaw {
    let source = raw
        .scene_linear_raster()
        .expect("raster source flag requires RGB pixels");
    let rgb = padded_rgb(source, [raw.width, raw.height], tile);
    raster_with_metadata(raw, tile.padded_width, tile.padded_height, rgb, false)
}

pub(super) fn extract_padded_binned_tile(
    raw: &LoadedRaw,
    binned: &BinnedSensor,
    tile: ExportTile,
) -> LoadedRaw {
    let pad = |rgb: &[f32]| padded_rgb(rgb, [raw.width, raw.height], tile);
    binned_with_metadata(
        raw,
        [tile.padded_width, tile.padded_height],
        BinnedSensor::new(pad(binned.rgb()), binned.ai_rgb().map(pad)),
        padded_tile_ai_denoised(raw, tile),
        false,
    )
}

pub fn crop_raw(raw: &LoadedRaw, x: u32, y: u32, width: u32, height: u32) -> LoadedRaw {
    let region = clamped_region(raw, x, y, width, height);
    if let Some(binned) = &raw.binned_sensor {
        return crop_binned(raw, binned, region);
    }
    if raw.is_pre_demosaiced_raster() {
        return crop_raster(raw, region);
    }
    crop_mosaic(raw, region)
}

fn crop_mosaic(raw: &LoadedRaw, region: [u32; 4]) -> LoadedRaw {
    let [x, y, width, height] = region;
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
        lens_geometry: is_whole_image(raw, region)
            .then(|| raw.lens_geometry.clone())
            .flatten(),
        ai_denoised: Arc::new(RwLock::new(crop_ai_denoised(raw, x, y, width, height))),
        ..raw.derive_with(
            width,
            height,
            raw_pixels,
            color_indices,
            black_levels_per_pixel,
        )
    }
}

pub fn build_proxy(raw: &LoadedRaw, spec: ProxySpec) -> LoadedRaw {
    build_region_proxy(raw, 0, 0, raw.width, raw.height, spec)
}

/// A proxy of `region` of `raw`'s mosaic binned to `output` pixels
/// ([`bin_mosaic`]).
fn build_binned_mosaic_proxy(
    raw: &LoadedRaw,
    region: [u32; 4],
    output: [u32; 2],
    cancel: &AtomicBool,
) -> Option<LoadedRaw> {
    let [x, y, region_width, region_height] = region;
    let [width, height] = output;
    let BinnedMosaic {
        sensor,
        variance_scale,
    } = bin_mosaic(raw, region, output, cancel)?;
    Some(LoadedRaw {
        noise_profile: raw.noise_profile.scaled_variance(variance_scale),
        ..binned_with_metadata(
            raw,
            output,
            sensor,
            // A Bayer result is binned with the mosaic; an X-Trans one is a
            // demosaiced scene, which is averaged as it is.
            proxy_ai_denoised(raw, x, y, region_width, region_height, width, height),
            is_whole_image(raw, region),
        )
    })
}

/// Builds a bounded, interactive approximation of a source region.
///
/// A sensor mosaic is binned to co-sited RGB ([`bin_mosaic`]) before
/// highlight reconstruction and denoise. It is consequently not a fidelity
/// reference: averaging can hide clipped samples, and it reduces the noise
/// seen by RAW-domain processing. Use a native crop (or native tiles) and
/// resize the developed linear result for a settled/final comparison.
pub fn build_region_proxy(
    raw: &LoadedRaw,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    spec: ProxySpec,
) -> LoadedRaw {
    build_region_proxy_until(raw, x, y, width, height, spec, &AtomicBool::new(false))
        .expect("a flag nobody else holds is never set, so the build is not cancelled")
}

/// [`build_region_proxy`] that gives up, returning `None`, soon after `cancel`
/// is set: it looks at the flag every few rows it resamples. For a build whose
/// result is no longer wanted, such as a zoomed crop of a view that moved on.
pub fn build_region_proxy_until(
    raw: &LoadedRaw,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    spec: ProxySpec,
    cancel: &AtomicBool,
) -> Option<LoadedRaw> {
    let region = clamped_region(raw, x, y, width, height);
    if let Some(binned) = &raw.binned_sensor {
        return build_binned_region_proxy(raw, binned, region, spec, cancel);
    }
    if raw.is_pre_demosaiced_raster() {
        return build_raster_region_proxy(raw, region, spec, cancel);
    }
    let [_, _, region_width, region_height] = region;
    let max_edge = spec.max_edge.max(1);
    if region_width.max(region_height) > max_edge {
        let output = reduced_size(region_width, region_height, max_edge);
        return build_binned_mosaic_proxy(raw, region, output, cancel);
    }
    if is_whole_image(raw, region) {
        return Some(raw.clone());
    }
    Some(crop_mosaic(raw, region))
}
