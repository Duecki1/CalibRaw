//! Export tiling: halos required by each stage, tile plans and padded tile extraction.

use super::*;

pub(super) const DEMOSAIC_CHAIN_SUPPORT: u32 = 32;
const COLOR_DENOISE_SUPPORT_FAST: u32 = 2;
const COLOR_DENOISE_SUPPORT_BALANCED: u32 = 2 * (1 + 2 + 4 + 8);
const COLOR_DENOISE_SUPPORT_HIGH: u32 = COLOR_DENOISE_SUPPORT_BALANCED + 16 + 32;
/// Native pixels around a pixel that Chromatic Aberration's fringe window
/// reads (`CA_WINDOW_RADIUS` in chromatic_aberration.wgsl).
pub const CA_WINDOW_RADIUS: u32 = 7;
/// Pixels Chromatic Aberration reads past a tile core: its fringe window
/// around the farthest red or blue sample a lateral shift and its cubic
/// interpolation take. The window's Laplacian of green reaches less far.
pub const CHROMATIC_ABERRATION_SUPPORT: u32 =
    CA_WINDOW_RADIUS + crate::pipeline::MAX_LATERAL_SHIFT as u32 + 2;

/// Native processing pixels represented by one adaptive tone-guide cell.
///
/// Cropped and tiled processing must use this same global grid so the guide
/// does not move when a crop origin changes. Every platform uses the same
/// grid and radius, so an edit renders alike on all of them.
pub const TONE_GUIDE_CELL_SIZE: u32 = 4;

/// Radius in cells of the tone-guide blur at a resolution scale of 1: a
/// 1080-pixel short edge (`resolution_scale` in common.wgsl). The shader
/// scales it with the photo, by at most `MAX_RESOLUTION_SCALE`.
pub const TONE_GUIDE_RADIUS_CELLS: u32 = 5;
/// Largest scale `resolution_scale` (common.wgsl) applies to reference radii;
/// every caller there passes it as `maximum`.
pub const MAX_RESOLUTION_SCALE: u32 = 3;
pub(super) const TONE_GUIDE_SUPPORT: u32 =
    (TONE_GUIDE_RADIUS_CELLS * MAX_RESOLUTION_SCALE + 1) * TONE_GUIDE_CELL_SIZE;
// Capture sharpening (detail_capture.wgsl): a 5×5 blur at a step of at most 3,
// in the scene tone pass before the scene effects.
pub(super) const CAPTURE_SHARPEN_SUPPORT: u32 = 6;
/// Pixels past a tile core that the haze grid's box means read in the export
/// tone prepass (`HAZE_BOX_MAX` - 1 in haze_analysis.wgsl).
pub const HAZE_BOX_SUPPORT: u32 = 1;
/// Pixels around each pixel that Dehaze blurs into its guide scene
/// (`HAZE_GUIDE_MAX_RADIUS` in haze.wgsl); it also reads the full-image haze
/// map.
pub const HAZE_GUIDE_SUPPORT: u32 = 2;
// The prepass reads the box beside the tone guide over the same scene, and
// Dehaze reads its guide scene from that scene in the stage that applies the
// tone guide, so the guide's halo covers both.
const _: () = assert!(HAZE_BOX_SUPPORT <= TONE_GUIDE_SUPPORT);
const _: () = assert!(HAZE_GUIDE_SUPPORT <= TONE_GUIDE_SUPPORT);
/// Footprint of the Texture and Clarity cascade (detail_scale_space.wgsl):
/// 2 taps each side at steps 1, 2, 4, …, 2^(levels - 1), so 2 × (2^levels - 1).
/// Texture alone builds 5 levels, Clarity all 7; `layout_contract_tests`
/// derives both from the shader constants.
pub const DETAIL_TEXTURE_SUPPORT: u32 = 62;
pub const DETAIL_CLARITY_SUPPORT: u32 = 254;
// Texture and Clarity run in one pass over the same input, so their
// footprints overlap rather than add up; sharpening's adds to them.
const _: () = assert!(DETAIL_TEXTURE_SUPPORT <= DETAIL_CLARITY_SUPPORT);
pub(super) const LOCAL_EFFECTS_SUPPORT: u32 = CAPTURE_SHARPEN_SUPPORT + DETAIL_CLARITY_SUPPORT;
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
    + CHROMATIC_ABERRATION_SUPPORT
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

    if exposure.chromatic_aberration > 0.0 {
        support += CHROMATIC_ABERRATION_SUPPORT;
    }

    if exposure.chroma_denoise > 1e-6 {
        support += match exposure.denoise_quality {
            DenoiseQuality::Fast => COLOR_DENOISE_SUPPORT_FAST,
            DenoiseQuality::Balanced => COLOR_DENOISE_SUPPORT_BALANCED,
            DenoiseQuality::High => COLOR_DENOISE_SUPPORT_HIGH,
        };
    }

    // Sharpen components run in the capture-sharpening pass of the Detail card.
    if exposure.sharpen_amount.abs() > 1e-6 || effect_active(MaskEffect::Sharpen) {
        support += CAPTURE_SHARPEN_SUPPORT;
    }
    let local_adjustment_active = |value: fn(&crate::pipeline::LocalAdjustments) -> f32| {
        masks.masks.iter().any(|mask| {
            mask.enabled
                && mask.opacity > 0.0
                && mask.adjustments_enabled
                && mask.effect.uses_adjustments()
                && value(&mask.adjustments).abs() > 1e-6
        })
    };
    let scene_effects_support = [
        (
            exposure.texture.abs() > 1e-6 || local_adjustment_active(|a| a.texture),
            DETAIL_TEXTURE_SUPPORT,
        ),
        (
            exposure.clarity.abs() > 1e-6 || local_adjustment_active(|a| a.clarity),
            DETAIL_CLARITY_SUPPORT,
        ),
    ]
    .into_iter()
    .filter(|(active, _)| *active)
    .map(|(_, support)| support)
    .max()
    .unwrap_or(0);
    support += scene_effects_support;

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
    if let Some(binned) = &raw.binned_sensor {
        return extract_padded_binned_tile(raw, binned, tile);
    }
    if raw.is_pre_demosaiced_raster() {
        return extract_padded_raster_tile(raw, tile);
    }
    let mut tile_raw = raw.derive_with(
        tile.padded_width,
        tile.padded_height,
        Vec::new(),
        raw.color_indices.subregion_reflected(
            i64::from(tile.global_origin_x),
            i64::from(tile.global_origin_y),
            tile.padded_width,
            tile.padded_height,
        ),
        raw.black_levels_per_pixel.subregion_reflected(
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
    if let Some(binned) = &raw.binned_sensor {
        // Binned views are previews, so their tiles are built afresh rather
        // than in `tile_raw`'s buffers.
        *tile_raw = extract_padded_binned_tile(raw, binned, tile);
        return;
    }
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
        let source = raw
            .scene_linear_raster()
            .expect("raster source flag requires RGB pixels");
        fill_padded_rgb(source, [raw.width, raw.height], tile, rgb);
        tile_raw.raw_pixels = Default::default();
        tile_raw.lens_geometry = None;
        tile_raw.clear_ai_denoised_image();
        return;
    }
    tile_raw.width = tile.padded_width;
    tile_raw.height = tile.padded_height;
    tile_raw.color_indices = raw.color_indices.subregion_reflected(
        i64::from(tile.global_origin_x),
        i64::from(tile.global_origin_y),
        tile.padded_width,
        tile.padded_height,
    );
    tile_raw.black_levels_per_pixel = raw.black_levels_per_pixel.subregion_reflected(
        i64::from(tile.global_origin_x),
        i64::from(tile.global_origin_y),
        tile.padded_width,
        tile.padded_height,
    );
    fill_padded_tile(raw, tile, tile_raw);
}

/// Fills the tile's samples. The halo past the photo mirrors it
/// ([`reflect_index`]), because repeating an edge sample would leave a halo
/// of only that edge's CFA colours: demosaiced, it has a false colour, which
/// the demosaic reads back into the edge pixels and colour denoise spreads
/// into the photo.
fn fill_padded_tile(raw: &LoadedRaw, tile: ExportTile, tile_raw: &mut LoadedRaw) {
    if let Ok(mut cached) = tile_raw.ai_denoised.write() {
        *cached = padded_tile_ai_denoised(raw, tile);
    }
    let width = tile.padded_width as usize;
    let height = tile.padded_height as usize;
    // The tile's own samples, reused from the previous tile.
    let samples = tile_raw.raw_pixels.make_mut();
    samples.resize(width.saturating_mul(height), 0);

    let source_width = raw.width as i64;

    for local_y in 0..tile.padded_height {
        let global_y = reflect_index(
            i64::from(tile.global_origin_y) + i64::from(local_y),
            raw.height as usize,
        );
        let destination_start = local_y as usize * width;
        let destination = &mut samples[destination_start..destination_start + width];
        let origin_x = i64::from(tile.global_origin_x);
        let end_x = origin_x + i64::from(tile.padded_width);
        let source_row = global_y * raw.width as usize;

        if origin_x >= 0 && end_x <= source_width {
            let source_start = source_row + origin_x as usize;
            destination.copy_from_slice(&raw.raw_pixels[source_start..source_start + width]);
            continue;
        }

        let left = (-origin_x).clamp(0, i64::from(tile.padded_width)) as usize;
        let right = (end_x - source_width).clamp(0, i64::from(tile.padded_width)) as usize;
        let middle_len = width.saturating_sub(left + right);
        if middle_len > 0 {
            let source_start = source_row + origin_x.max(0) as usize;
            destination[left..left + middle_len]
                .copy_from_slice(&raw.raw_pixels[source_start..source_start + middle_len]);
        }
        let halo = (0..left).chain(left + middle_len..width);
        for local_x in halo {
            let global_x = reflect_index(origin_x + local_x as i64, raw.width as usize);
            destination[local_x] = raw.raw_pixels[source_row + global_x];
        }
    }
}
