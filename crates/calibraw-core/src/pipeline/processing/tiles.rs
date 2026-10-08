//! Export tiling: halos required by each stage, tile plans and padded tile extraction.

use super::*;

pub(super) const DEMOSAIC_CHAIN_SUPPORT: u32 = 32;
const COLOR_DENOISE_SUPPORT_FAST: u32 = 2;
const COLOR_DENOISE_SUPPORT_BALANCED: u32 = 2 * (1 + 2 + 4 + 8);
const COLOR_DENOISE_SUPPORT_HIGH: u32 = COLOR_DENOISE_SUPPORT_BALANCED + 16 + 32;

/// Native processing pixels represented by one adaptive tone-guide cell.
///
/// Cropped and tiled processing must use this same global grid so the guide
/// does not move when a crop origin changes.
pub const TONE_GUIDE_CELL_SIZE: u32 = if cfg!(target_os = "android") { 8 } else { 4 };

const TONE_GUIDE_RADIUS_CELLS: u32 = if cfg!(target_os = "android") { 3 } else { 5 };
pub(super) const TONE_GUIDE_SUPPORT: u32 = (TONE_GUIDE_RADIUS_CELLS + 1) * TONE_GUIDE_CELL_SIZE;
pub(super) const LOCAL_EFFECTS_SUPPORT: u32 = 28;
pub(super) const NEON_SUPPORT: u32 = 48;
pub(super) const MASK_BLUR_SUPPORT: u32 = 72;
pub(super) const FOCUS_BLUR_SUPPORT: u32 = 144;
pub(super) const EDGE_GLOW_SUPPORT: u32 = 48;
pub(super) const PIXELATE_SUPPORT: u32 = 96;
// Depth-guided effects (Fog, Smoke, Relight) upsample scene depth with image
// colours up to 3.5 depth texels away (fog_depth_at in atmosphere.wgsl). The
// depth texture is 1024 texels across, so this covers images up to about
// 13,500 pixels wide.
pub(super) const SCENE_DEPTH_GUIDE_SUPPORT: u32 = 48;
// EXPORT_CUMULATIVE_SUPPORT reserves Pixelate's support for the whole
// post-blur creative pass.
const _: () = assert!(SCENE_DEPTH_GUIDE_SUPPORT <= PIXELATE_SUPPORT);
// Maximum footprint of Glow diffusion and independent Glow/Halation modules.
// These sample the same local_effects input, so their support is not cumulative.
pub(super) const GLOW_SUPPORT: u32 = 96;
pub(super) const COLOR_MIXER_SUPPORT: u32 = 4;
const EXPORT_CUMULATIVE_SUPPORT: u32 = HIGHLIGHT_RECONSTRUCTION_SUPPORT
    + DEMOSAIC_CHAIN_SUPPORT
    + COLOR_DENOISE_SUPPORT_HIGH
    + TONE_GUIDE_SUPPORT
    + LOCAL_EFFECTS_SUPPORT
    + NEON_SUPPORT
    + MASK_BLUR_SUPPORT
    + FOCUS_BLUR_SUPPORT
    + PIXELATE_SUPPORT
    + GLOW_SUPPORT
    + COLOR_MIXER_SUPPORT;

pub const EXPORT_TILE_HALO: u32 = EXPORT_CUMULATIVE_SUPPORT.div_ceil(8) * 8;

pub const MIN_EXPORT_TILE_HALO: u32 = (HIGHLIGHT_RECONSTRUCTION_SUPPORT
    + DEMOSAIC_CHAIN_SUPPORT
    + TONE_GUIDE_SUPPORT
    + COLOR_MIXER_SUPPORT)
    .div_ceil(8)
    * 8;

pub fn required_export_tile_halo(exposure: &ExposureParams, masks: &MaskStack) -> u32 {
    // Global components, mask components, and legacy mask effects all feed the
    // same spatial passes and must contribute identical sampling support.
    let effect_active = |effect| {
        let active = |component: &crate::pipeline::EffectComponent| {
            component.effect == effect && component.is_active()
        };
        masks.global_effects.iter().any(active)
            || masks.masks.iter().any(|mask| {
                mask.enabled
                    && mask.opacity > 0.0
                    && (mask.effect_components.iter().any(active)
                        || active(&crate::pipeline::EffectComponent {
                            effect: mask.effect,
                            enabled: true,
                            settings: mask.effect_settings,
                        }))
            })
    };
    let mut support = HIGHLIGHT_RECONSTRUCTION_SUPPORT
        + DEMOSAIC_CHAIN_SUPPORT
        + TONE_GUIDE_SUPPORT
        + COLOR_MIXER_SUPPORT;

    if exposure.chroma_denoise > 1e-6 {
        support += match exposure.denoise_quality {
            DenoiseQuality::Fast => COLOR_DENOISE_SUPPORT_FAST,
            DenoiseQuality::Balanced => COLOR_DENOISE_SUPPORT_BALANCED,
            DenoiseQuality::High => COLOR_DENOISE_SUPPORT_HIGH,
        };
    }

    let local_spatial_active = exposure.sharpen_amount.abs() > 1e-6
        || exposure.texture.abs() > 1e-6
        || exposure.clarity.abs() > 1e-6
        || exposure.dehaze.abs() > 1e-6
        || masks.masks.iter().any(|mask| {
            mask.enabled
                && mask.opacity > 0.0
                && mask.adjustments_enabled
                && mask.effect.uses_adjustments()
                && (mask.adjustments.texture.abs() > 1e-6
                    || mask.adjustments.clarity.abs() > 1e-6
                    || mask.adjustments.dehaze.abs() > 1e-6)
        });
    if local_spatial_active {
        support += LOCAL_EFFECTS_SUPPORT;
    }

    if effect_active(MaskEffect::Neon) {
        support += NEON_SUPPORT;
    }

    if effect_active(MaskEffect::Blur) {
        support += MASK_BLUR_SUPPORT;
    }

    let focus_blur_active = [
        MaskEffect::LensBlur,
        MaskEffect::MotionBlur,
        MaskEffect::RadialBlur,
        MaskEffect::TiltShift,
    ]
    .into_iter()
    .any(effect_active);
    if focus_blur_active {
        support += FOCUS_BLUR_SUPPORT;
    }

    let post_blur_creative_support = [
        (MaskEffect::EdgeGlow, EDGE_GLOW_SUPPORT),
        (MaskEffect::Pixelate, PIXELATE_SUPPORT),
        (MaskEffect::Fog, SCENE_DEPTH_GUIDE_SUPPORT),
        (MaskEffect::Smoke, SCENE_DEPTH_GUIDE_SUPPORT),
        (MaskEffect::Relight, SCENE_DEPTH_GUIDE_SUPPORT),
    ]
    .into_iter()
    .filter(|(effect, _)| effect_active(*effect))
    .map(|(_, support)| support)
    .max()
    .unwrap_or(0);
    support += post_blur_creative_support;

    // Self-illuminating Glow shares the Glow diffusion passes. Highlight Glow
    // and Halation sample the scene neighborhood with their own radius. All read
    // the same local_effects input with at most GLOW_SUPPORT pixels of support,
    // so reserve the maximum once, not the sum of their footprints or the number
    // of active components.
    if effect_active(MaskEffect::Glow) || effect_active(MaskEffect::Halation) {
        support += GLOW_SUPPORT;
    }

    support.div_ceil(8) * 8
}

#[derive(Clone, Copy, Debug)]
pub struct TileSpec {
    pub core_edge: u32,
    pub halo: u32,
}

impl Default for TileSpec {
    fn default() -> Self {
        Self {
            core_edge: if cfg!(target_os = "android") {
                768
            } else {
                1024
            },
            halo: EXPORT_TILE_HALO,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportTile {
    pub core_x: u32,
    pub core_y: u32,
    pub core_width: u32,
    pub core_height: u32,
    pub local_core_x: u32,
    pub local_core_y: u32,
    pub padded_width: u32,
    pub padded_height: u32,
    pub global_origin_x: i32,
    pub global_origin_y: i32,
}

#[derive(Clone, Debug)]
pub struct TilePlan {
    pub full_width: u32,
    pub full_height: u32,
    pub spec: TileSpec,
    pub tiles: Vec<ExportTile>,
}

impl TilePlan {
    pub fn new(full_width: u32, full_height: u32, spec: TileSpec) -> Self {
        let core_edge = spec.core_edge.max(64);
        let halo = spec.halo;
        let padded_width = core_edge.saturating_add(halo.saturating_mul(2));
        let padded_height = padded_width;
        let mut tiles = Vec::new();

        let mut y = 0;
        while y < full_height {
            let core_height = core_edge.min(full_height - y);
            let mut x = 0;
            while x < full_width {
                let core_width = core_edge.min(full_width - x);
                tiles.push(ExportTile {
                    core_x: x,
                    core_y: y,
                    core_width,
                    core_height,
                    local_core_x: halo,
                    local_core_y: halo,
                    padded_width,
                    padded_height,
                    global_origin_x: x as i32 - halo as i32,
                    global_origin_y: y as i32 - halo as i32,
                });
                x = x.saturating_add(core_edge);
            }
            y = y.saturating_add(core_edge);
        }

        Self {
            full_width,
            full_height,
            spec: TileSpec { core_edge, halo },
            tiles,
        }
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }
}

pub fn extract_padded_tile(raw: &LoadedRaw, tile: ExportTile) -> LoadedRaw {
    if raw.is_pre_demosaiced_raster() {
        return extract_padded_raster_tile(raw, tile);
    }
    let mut tile_raw = raw.derive_with(
        tile.padded_width,
        tile.padded_height,
        Vec::new(),
        raw.color_indices.subregion_clamped(
            i64::from(tile.global_origin_x),
            i64::from(tile.global_origin_y),
            tile.padded_width,
            tile.padded_height,
        ),
        raw.black_levels_per_pixel.subregion_clamped(
            i64::from(tile.global_origin_x),
            i64::from(tile.global_origin_y),
            tile.padded_width,
            tile.padded_height,
        ),
    );
    fill_padded_tile(raw, tile, &mut tile_raw);
    tile_raw
}

pub fn extract_padded_tile_into(raw: &LoadedRaw, tile: ExportTile, tile_raw: &mut LoadedRaw) {
    tile_raw.opposed_chroma_cache = std::sync::Arc::clone(&raw.opposed_chroma_cache);
    tile_raw.opposed_chroma_source_identity =
        std::sync::Arc::clone(&raw.opposed_chroma_source_identity);
    tile_raw.opposed_chroma_reference_source = false;
    if raw.is_pre_demosaiced_raster() {
        let dimensions_changed =
            tile_raw.width != tile.padded_width || tile_raw.height != tile.padded_height;
        tile_raw.width = tile.padded_width;
        tile_raw.height = tile.padded_height;
        if dimensions_changed {
            tile_raw.color_indices =
                CompactPixelMap::repeating(tile.padded_width, tile.padded_height, 1, 1, vec![1]);
            tile_raw.black_levels_per_pixel =
                CompactPixelMap::repeating(tile.padded_width, tile.padded_height, 1, 1, vec![0.0]);
        }
        let expected = (tile.padded_width as usize)
            .saturating_mul(tile.padded_height as usize)
            .saturating_mul(3);
        if tile_raw
            .scene_linear_raster
            .as_ref()
            .is_none_or(|rgb| rgb.len() != expected)
        {
            tile_raw.scene_linear_raster = Some(vec![0.0f32; expected].into());
        }
        let rgb = std::sync::Arc::make_mut(
            tile_raw
                .scene_linear_raster
                .as_mut()
                .expect("raster tile allocation must exist"),
        );
        fill_padded_raster_pixels(raw, tile, rgb);
        tile_raw.raw_pixels.clear();
        tile_raw.lens_geometry = None;
        tile_raw.clear_ai_denoised_image();
        return;
    }
    tile_raw.width = tile.padded_width;
    tile_raw.height = tile.padded_height;
    tile_raw.color_indices = raw.color_indices.subregion_clamped(
        i64::from(tile.global_origin_x),
        i64::from(tile.global_origin_y),
        tile.padded_width,
        tile.padded_height,
    );
    tile_raw.black_levels_per_pixel = raw.black_levels_per_pixel.subregion_clamped(
        i64::from(tile.global_origin_x),
        i64::from(tile.global_origin_y),
        tile.padded_width,
        tile.padded_height,
    );
    fill_padded_tile(raw, tile, tile_raw);
}

fn fill_padded_tile(raw: &LoadedRaw, tile: ExportTile, tile_raw: &mut LoadedRaw) {
    if let Ok(mut cached) = tile_raw.ai_denoised.write() {
        *cached = padded_tile_ai_denoised(raw, tile);
    }
    let width = tile.padded_width as usize;
    let height = tile.padded_height as usize;
    tile_raw.raw_pixels.resize(width.saturating_mul(height), 0);

    let source_width = raw.width as i64;
    let max_x = source_width.saturating_sub(1);
    let max_y = i64::from(raw.height.saturating_sub(1));

    for local_y in 0..tile.padded_height {
        let global_y = (i64::from(tile.global_origin_y) + i64::from(local_y)).clamp(0, max_y);
        let destination_start = local_y as usize * width;
        let destination = &mut tile_raw.raw_pixels[destination_start..destination_start + width];
        let origin_x = i64::from(tile.global_origin_x);
        let end_x = origin_x + i64::from(tile.padded_width);
        let source_row = global_y as usize * raw.width as usize;

        if origin_x >= 0 && end_x <= source_width {
            let source_start = source_row + origin_x as usize;
            destination.copy_from_slice(&raw.raw_pixels[source_start..source_start + width]);
            continue;
        }

        let left = (-origin_x).clamp(0, i64::from(tile.padded_width)) as usize;
        let right = (end_x - source_width).clamp(0, i64::from(tile.padded_width)) as usize;
        if left > 0 {
            destination[..left].fill(raw.raw_pixels[source_row]);
        }
        let middle_start_global = origin_x.max(0);
        let middle_len = width.saturating_sub(left + right);
        if middle_len > 0 {
            let source_start = source_row + middle_start_global as usize;
            destination[left..left + middle_len]
                .copy_from_slice(&raw.raw_pixels[source_start..source_start + middle_len]);
        }
        if right > 0 {
            let edge = raw.raw_pixels[source_row + max_x as usize];
            destination[width - right..].fill(edge);
        }
    }
}
