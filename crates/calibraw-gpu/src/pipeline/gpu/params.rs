//! `GpuParams`: edit parameters packed into the uniform layouts the WGSL stages read.

use super::*;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(super) struct CameraUniforms {
    pub(super) black_point: f32,
    pub(super) temperature: f32,
    pub(super) highlight_clip: f32,
    pub(super) chroma_denoise: f32,
    pub(super) ca_red: f32,
    pub(super) ca_blue: f32,
    pub(super) highlight_reconstruction: f32,
    pub(super) tone_analysis_scale: f32,
    pub(super) tone_guide_radius: f32,
    pub(super) demosaic_mode: f32,
    pub(super) dual_threshold: f32,
    pub(super) frequency_chroma: f32,
    pub(super) tint: f32,
    pub(super) pre_demosaiced_raster: f32,
    pub(super) scene_view_transform_enabled: f32,
    pub(super) camera_linear_raster: f32,
    pub(super) highlight_options: [f32; 4],
    pub(super) noise_shot: [f32; 4],
    pub(super) noise_read: [f32; 4],
    pub(super) noise_options: [f32; 4],
    pub(super) wb: [f32; 4],
    pub(super) cam_to_srgb_0: [f32; 4],
    pub(super) cam_to_srgb_1: [f32; 4],
    pub(super) cam_to_srgb_2: [f32; 4],
    pub(super) black_levels: [f32; 4],
    pub(super) white_levels: [f32; 4],
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) tile_origin_x: i32,
    pub(super) tile_origin_y: i32,
    pub(super) full_width: u32,
    pub(super) full_height: u32,
    pub(super) abi_version: u32,
    pub(super) abi_size_bytes: u32,
    pub(super) tone_histogram_bounds: [u32; 4],
    pub(super) profile_hue_sat: [u32; 4],
    pub(super) profile_look: [u32; 4],
    pub(super) profile_tone: [u32; 4],
    pub(super) profile_flags: [u32; 4],
    pub(super) ai_denoise_enabled: u32,
    pub(super) user_exposure_bits: u32,
    pub(super) _pad_camera_0: u32,
    pub(super) _pad_camera_1: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(super) struct PackedPointColor {
    pub(super) sample_range: [f32; 4],
    pub(super) hue_range: [f32; 4],
    pub(super) saturation_range: [f32; 4],
    pub(super) luminance_range: [f32; 4],
    pub(super) shifts: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<CameraUniforms>() == CAMERA_UNIFORMS_SIZE_BYTES as usize);

pub(super) fn raster_uses_scene_view_transform(exposure: &ExposureParams) -> bool {
    const EPSILON: f32 = 1e-6;
    let default = SigmoidParams::default();
    exposure.contrast.abs() > EPSILON
        || (exposure.sigmoid.contrast - default.contrast).abs() > EPSILON
        || (exposure.sigmoid.skew - default.skew).abs() > EPSILON
        || (exposure.sigmoid.display_white_target - default.display_white_target).abs() > EPSILON
        || (exposure.sigmoid.display_black_target - default.display_black_target).abs() > EPSILON
        || (exposure.sigmoid.hue_preservation - default.hue_preservation).abs() > EPSILON
        || exposure.sigmoid.color_processing != default.color_processing
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(super) struct SceneToneUniforms {
    pub(super) exposure: f32,
    pub(super) saturation: f32,
    pub(super) vibrance: f32,
    // Depth availability must not share the packed mask crop bounds below.
    pub(super) scene_depth_present: u32,
    pub(super) basic_tone: [f32; 4],
    pub(super) sigmoid_curve: [f32; 4],
    pub(super) sigmoid_power: [f32; 4],
    /// Global point curves (master, red, green, blue), each packed by
    /// [`pack_local_point_curve`]: eight point pairs, then `[count, identity, 0, 0]`.
    pub(super) tone_curves: [[[f32; 4]; LOCAL_POINT_CURVE_BLOCKS]; 4],
    pub(super) hsl_hue_0: [f32; 4],
    pub(super) hsl_hue_1: [f32; 4],
    pub(super) hsl_saturation_0: [f32; 4],
    pub(super) hsl_saturation_1: [f32; 4],
    pub(super) hsl_luminance_0: [f32; 4],
    pub(super) hsl_luminance_1: [f32; 4],
    pub(super) mask_counts: [u32; 4],
    pub(super) grade_shadows: [f32; 4],
    pub(super) grade_midtones: [f32; 4],
    pub(super) grade_highlights: [f32; 4],
    pub(super) grade_global: [f32; 4],
    pub(super) grade_options: [f32; 4],
    pub(super) rec2020_to_xyz: [[f32; 4]; 3],
    pub(super) xyz_to_rec2020: [[f32; 4]; 3],
    pub(super) xyz_to_bradford: [[f32; 4]; 3],
    pub(super) bradford_to_xyz: [[f32; 4]; 3],
    pub(super) point_colors: [PackedPointColor; MAX_POINT_COLORS],
    pub(super) point_color_meta: [u32; 4],
}

const _: () =
    assert!(std::mem::size_of::<SceneToneUniforms>() == SCENE_TONE_UNIFORMS_SIZE_BYTES as usize);

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(super) struct EffectsUniforms {
    pub(super) presence: [f32; 4],
    pub(super) creative_effects: [f32; 4],
    // Halation amount, grain amount, any active halation, reserved.
    pub(super) film_effects: [f32; 4],
    pub(super) vignette: [f32; 4],
    pub(super) vignette_options: [f32; 4],
    pub(super) vignette_frame: [f32; 4],
    pub(super) vignette_transform: [f32; 4],
    pub(super) vignette_dark_half_fit: [f32; 4],
    pub(super) vignette_dark_full_fit: [f32; 4],
    pub(super) vignette_light_half_fit: [f32; 4],
    pub(super) vignette_light_full_fit: [f32; 4],
    pub(super) capture_scale_sigma: [f32; 4],
    pub(super) capture_thresholds: [f32; 4],
    pub(super) capture_mask_coherence: [f32; 4],
}

const _: () =
    assert!(std::mem::size_of::<EffectsUniforms>() == EFFECTS_UNIFORMS_SIZE_BYTES as usize);
const _: () = assert!(GPU_STAGE_UNIFORM_SIZE_BYTES == 2_256);

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(super) struct MaskData {
    pub(super) metadata: [u32; 4],
    pub(super) adjust_0: [f32; 4],
    pub(super) adjust_1: [f32; 4],
    pub(super) adjust_2: [f32; 4],
    // Local halation amount; remaining lanes reserved (grain is global only).
    pub(super) film_effects: [f32; 4],
    pub(super) curves: [[f32; 4]; LOCAL_POINT_CURVE_BLOCKS],
    pub(super) grade_shadows: [f32; 4],
    pub(super) grade_midtones: [f32; 4],
    pub(super) grade_highlights: [f32; 4],
    pub(super) grade_global: [f32; 4],
    pub(super) grade_options: [f32; 4],
    pub(super) curves_red: [[f32; 4]; LOCAL_POINT_CURVE_BLOCKS],
    pub(super) curves_green: [[f32; 4]; LOCAL_POINT_CURVE_BLOCKS],
    pub(super) curves_blue: [[f32; 4]; LOCAL_POINT_CURVE_BLOCKS],
    pub(super) hsl_hue_0: [f32; 4],
    pub(super) hsl_hue_1: [f32; 4],
    pub(super) hsl_saturation_0: [f32; 4],
    pub(super) hsl_saturation_1: [f32; 4],
    pub(super) hsl_luminance_0: [f32; 4],
    pub(super) hsl_luminance_1: [f32; 4],
    pub(super) point_colors: [PackedPointColor; MAX_POINT_COLORS],
    pub(super) point_color_meta: [u32; 4],
}

const _: () = assert!(std::mem::size_of::<MaskData>() == 1_488);

/// Inputs of the relight shadow map besides scene depth
/// (`GpuParams::relight_shadow_map_key`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct RelightShadowMapKey {
    full_size: [u32; 2],
    lights: [Option<[f32; 5]>; RELIGHT_SHADOW_MAP_CHANNELS],
}

#[derive(Clone, Debug)]
pub struct GpuParams {
    pub(super) camera: CameraUniforms,
    pub(super) scene_tone: SceneToneUniforms,
    pub(super) effects: EffectsUniforms,
    mask_data: Box<[MaskData]>,
    pub(super) scene_depth: Option<MaskImage>,
}

impl GpuParams {
    pub(super) fn camera_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(&self.camera)
    }

    pub(super) fn scene_tone_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(&self.scene_tone)
    }

    pub(super) fn effects_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(&self.effects)
    }

    pub(super) fn mask_data_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(&self.mask_data[..])
    }
}

/// An enabled Glow slot that emits into the shared Glow diffusion. Mirrors
/// `mask_glow_self_illuminating` in `mask_effects/glow.wgsl`.
pub(super) fn is_self_illuminating_glow(mask: &MaskData) -> bool {
    mask.metadata[0] != 0
        && mask.metadata[1] != 0
        && mask.metadata[3] >> MASK_EFFECT_ID_SHIFT == MaskEffect::Glow.shader_id()
        && mask.adjust_0[3] > 0.5
}

pub(super) fn split_eight(values: [f32; 8]) -> ([f32; 4], [f32; 4]) {
    (
        [values[0], values[1], values[2], values[3]],
        [values[4], values[5], values[6], values[7]],
    )
}

pub(super) fn pack_point_color(point: PointColor) -> PackedPointColor {
    let point = point.sanitized();
    let range = |r: crate::pipeline::PointColorRange| [r.min, r.inner_min, r.inner_max, r.max];
    PackedPointColor {
        sample_range: [
            point.sample_hsl[0],
            point.sample_hsl[1],
            point.sample_hsl[2],
            point.range,
        ],
        hue_range: range(point.hue_range),
        saturation_range: range(point.saturation_range),
        luminance_range: range(point.luminance_range),
        shifts: [
            point.hue_shift / 200.0,
            point.saturation_shift / 100.0,
            point.luminance_shift / 100.0,
            0.0,
        ],
    }
}

const POINT_CURVE_PAIRS: usize = MAX_POINT_CURVE_POINTS / 2;
const LOCAL_POINT_CURVE_BLOCKS: usize = POINT_CURVE_PAIRS + 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PackedPointCurve {
    pub(super) pairs: [[f32; 4]; POINT_CURVE_PAIRS],
    pub(super) meta: [f32; 4],
}

pub(super) fn pack_point_curve(curve: &PointCurve) -> PackedPointCurve {
    let pairs = std::array::from_fn(|pair| {
        [
            curve.points[pair * 2][0],
            curve.points[pair * 2][1],
            curve.points[pair * 2 + 1][0],
            curve.points[pair * 2 + 1][1],
        ]
    });
    PackedPointCurve {
        pairs,
        meta: [
            curve.len.clamp(2, MAX_POINT_CURVE_POINTS as u32) as f32,
            if curve.is_identity() { 1.0 } else { 0.0 },
            0.0,
            0.0,
        ],
    }
}

pub(super) fn pack_local_point_curve(curve: &PointCurve) -> [[f32; 4]; LOCAL_POINT_CURVE_BLOCKS] {
    let curve = pack_point_curve(curve);
    let mut packed = [[0.0; 4]; LOCAL_POINT_CURVE_BLOCKS];
    packed[..POINT_CURVE_PAIRS].copy_from_slice(&curve.pairs);
    packed[POINT_CURVE_PAIRS] = curve.meta;
    packed
}

pub(super) fn pack_color_grade_wheel(wheel: crate::pipeline::ColorGradeWheel) -> [f32; 4] {
    [
        color_grade_hue_turns(wheel.hue),
        (wheel.saturation / 100.0).clamp(0.0, 1.0),
        (wheel.luminance / 100.0).clamp(-1.0, 1.0),
        0.0,
    ]
}

fn color_grade_hue_turns(hue_degrees: f32) -> f32 {
    let hue = hue_degrees.rem_euclid(360.0) / 60.0;
    let sector = hue.floor() as u32;
    let fraction = hue - sector as f32;
    let value = 0.9;
    let (r, g, b) = match sector % 6 {
        0 => (value, value * fraction, 0.0),
        1 => (value * (1.0 - fraction), value, 0.0),
        2 => (0.0, value, value * fraction),
        3 => (0.0, value * (1.0 - fraction), value),
        4 => (value * fraction, 0.0, value),
        _ => (value, 0.0, value * (1.0 - fraction)),
    };
    let [_, a, b] = linear_srgb_to_oklab([r, g, b].map(srgb_decode));
    b.atan2(a).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU
}

pub(super) fn shader_highlight_method(
    cfa_kind: CfaKind,
    method: HighlightReconstructionMethod,
) -> f32 {
    match (cfa_kind, method) {
        (CfaKind::XTrans, HighlightReconstructionMethod::Lch) => {
            HighlightReconstructionMethod::InpaintOpposed.shader_value()
        }
        (_, method) => method.shader_value(),
    }
}

pub(super) fn pack_view_color_options(
    grading: crate::pipeline::ColorGrading,
    hue: f32,
) -> [f32; 4] {
    [
        (grading.blending / 100.0).clamp(0.0, 1.0),
        (grading.balance / 100.0).clamp(-1.0, 1.0),
        effect_params::adjustment::HUE.clamp(hue),
        0.0,
    ]
}

pub(super) fn canonicalize_green_noise(
    mut coefficients: [f32; 4],
    green2_present: bool,
) -> [f32; 4] {
    if green2_present {
        let green = 0.5 * (coefficients[1] + coefficients[3]);
        coefficients[1] = green;
        coefficients[3] = green;
    }
    coefficients
}

#[derive(Clone, Copy)]
pub(super) struct GpuTileInfo {
    pub(super) origin_x: i32,
    pub(super) origin_y: i32,
    pub(super) full_width: u32,
    pub(super) full_height: u32,
}

pub(super) struct GpuParamContext<'a> {
    pub(super) exposure: &'a ExposureParams,
    pub(super) masks: &'a MaskStack,
    pub(super) raw: &'a LoadedRaw,
    pub(super) tile: GpuTileInfo,
}

impl GpuParams {
    pub fn new(exposure: &ExposureParams, masks: &MaskStack, raw: &LoadedRaw) -> Self {
        Self::new_for_tile(exposure, masks, raw, 0, 0, raw.width, raw.height)
    }

    pub fn new_for_tile(
        exposure: &ExposureParams,
        masks: &MaskStack,
        raw: &LoadedRaw,
        tile_origin_x: i32,
        tile_origin_y: i32,
        full_width: u32,
        full_height: u32,
    ) -> Self {
        let context = GpuParamContext {
            exposure,
            masks,
            raw,
            tile: GpuTileInfo {
                origin_x: tile_origin_x,
                origin_y: tile_origin_y,
                full_width,
                full_height,
            },
        };
        let mask_data = pack_mask_params(masks);
        Self {
            camera: pack_camera_params(&context),
            scene_tone: pack_scene_tone_params(&context),
            effects: pack_effect_params(&context, &mask_data),
            mask_data,
            scene_depth: masks.scene_depth_image().cloned(),
        }
    }

    pub fn with_vignette_geometry(mut self, geometry: GeometryTransform) -> Self {
        let geometry = geometry.sanitized();
        let crop = geometry.crop;
        let source_width = self.camera.full_width.max(1) as f32;
        let source_height = self.camera.full_height.max(1) as f32;
        let crop_width = ((crop[2] - crop[0]) * source_width).max(1e-6);
        let crop_height = ((crop[3] - crop[1]) * source_height).max(1e-6);
        let center_u = (crop[0] + crop[2]) * 0.5;
        let center_v = (crop[1] + crop[3]) * 0.5;

        let fx = if geometry.flip_horizontal { -1.0 } else { 1.0 };
        let fy = if geometry.flip_vertical { -1.0 } else { 1.0 };
        let shx = geometry.horizontal_transform.to_radians().tan();
        let shy = geometry.vertical_transform.to_radians().tan();
        let angle = geometry.rotation_degrees.to_radians();
        let cos = angle.cos();
        let sin = angle.sin();
        let affine = [
            cos * fx - sin * shy * fx,
            cos * shx * fy - sin * fy,
            sin * fx + cos * shy * fx,
            sin * shx * fy + cos * fy,
        ];
        let quarter = match geometry.quarter_turns % 4 {
            0 => [1.0, 0.0, 0.0, 1.0],
            1 => [0.0, -1.0, 1.0, 0.0],
            2 => [-1.0, 0.0, 0.0, -1.0],
            _ => [0.0, 1.0, -1.0, 0.0],
        };
        let forward = [
            quarter[0] * affine[0] + quarter[1] * affine[2],
            quarter[0] * affine[1] + quarter[1] * affine[3],
            quarter[2] * affine[0] + quarter[3] * affine[2],
            quarter[2] * affine[1] + quarter[3] * affine[3],
        ];
        let (output_width, output_height) = if geometry.quarter_turns.is_multiple_of(2) {
            (crop_width, crop_height)
        } else {
            (crop_height, crop_width)
        };

        self.effects.vignette_frame = [center_u, center_v, output_width, output_height];
        self.effects.vignette_transform = [
            forward[0] * source_width / output_width,
            forward[1] * source_height / output_width,
            forward[2] * source_width / output_height,
            forward[3] * source_height / output_height,
        ];
        self
    }

    pub fn with_global_tone_histogram_bounds(
        mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Self {
        let x0 = x.min(self.camera.full_width);
        let y0 = y.min(self.camera.full_height);
        self.camera.tone_histogram_bounds = [
            x0,
            y0,
            x.saturating_add(width).min(self.camera.full_width),
            y.saturating_add(height).min(self.camera.full_height),
        ];
        self
    }

    pub fn with_mask_uv_rect(mut self, rect: [f32; 4]) -> Self {
        self.set_mask_uv_rect(rect);
        self.scene_tone.mask_counts[3] = u32::MAX;
        self
    }

    pub fn with_mask_uv_rect_and_extent(
        mut self,
        rect: [f32; 4],
        texture_extent: [u32; 2],
    ) -> Self {
        self.set_mask_uv_rect(rect);
        let width = texture_extent[0].clamp(1, u16::MAX as u32);
        let height = texture_extent[1].clamp(1, u16::MAX as u32);
        self.scene_tone.mask_counts[3] = width | (height << 16);
        self
    }

    fn set_mask_uv_rect(&mut self, rect: [f32; 4]) {
        let pack = |u: f32, v: f32| {
            let u = (u.clamp(0.0, 1.0) * 65_535.0).round() as u32;
            let v = (v.clamp(0.0, 1.0) * 65_535.0).round() as u32;
            u | (v << 16)
        };
        let min_u = rect[0].min(rect[2]);
        let min_v = rect[1].min(rect[3]);
        let max_u = rect[0].max(rect[2]);
        let max_v = rect[1].max(rect[3]);
        self.scene_tone.mask_counts[1] = pack(min_u, min_v);
        self.scene_tone.mask_counts[2] = pack(max_u, max_v);
    }

    pub(super) fn uses_ai_denoise(&self) -> bool {
        self.camera.ai_denoise_enabled != 0
    }

    pub(super) fn needs_dual_demosaic_passes(&self) -> bool {
        self.camera.demosaic_mode >= 1.5
    }

    pub(super) fn needs_intermediate_adjustment_passes(&self) -> bool {
        let global_effects = self.scene_tone.saturation.abs() > 1e-6
            || self.scene_tone.vibrance.abs() > 1e-6
            || self.effects.presence[..3]
                .iter()
                .any(|value| value.abs() > 1e-6);
        let creative =
            self.effects.creative_effects[0].abs() > 1e-6 || self.effects.film_effects[0] > 1e-6;
        let local_count = (self.scene_tone.mask_counts[0] as usize).min(MAX_RENDER_MASK_SLOTS);
        let local_effects = (0..local_count).any(|index| {
            let local = self.mask_data[index];
            let state = local.metadata;
            if state[0] == 0 || state[1] == 0 {
                return false;
            }
            let effect_id = state[3] >> MASK_EFFECT_ID_SHIFT;
            if effect_id == MaskEffect::Neon.shader_id()
                || effect_id == MaskEffect::Glow.shader_id()
                || effect_id == MaskEffect::LightRays.shader_id()
                || effect_id == MaskEffect::Relight.shader_id()
                || effect_id == MaskEffect::Blur.shader_id()
                || effect_id == MaskEffect::LensBlur.shader_id()
                || effect_id == MaskEffect::MotionBlur.shader_id()
                || effect_id == MaskEffect::RadialBlur.shader_id()
                || effect_id == MaskEffect::TiltShift.shader_id()
                || effect_id == MaskEffect::EdgeGlow.shader_id()
                || effect_id == MaskEffect::Pixelate.shader_id()
                || effect_id == MaskEffect::Fog.shader_id()
                || effect_id == MaskEffect::Smoke.shader_id()
                || effect_id == MaskEffect::Halation.shader_id()
            {
                return true;
            }
            // Grain and vignette operate after the display transform and need no
            // scene neighborhood passes. Their packed controls are not adjustments.
            if effect_id != MaskEffect::Adjustment.shader_id() {
                return false;
            }

            let tone = local.adjust_0[1..].iter().any(|value| value.abs() > 1e-6);
            let white_balance = local.adjust_1[0].abs() > 1e-6
                || local.adjust_1[2].abs() > 1e-6
                || local.adjust_1[3].abs() > 1e-6;
            let curves = state[2] != 0;
            let presence_or_saturation = local.adjust_2.iter().any(|value| value.abs() > 1e-6);
            tone || white_balance
                || curves
                || presence_or_saturation
                || local.film_effects[0] > 1e-6
        });
        global_effects || creative || local_effects
    }

    pub(super) fn needs_glow_passes(&self) -> bool {
        if self.effects.creative_effects[0].abs() > 1e-6 || self.effects.film_effects[0] > 1e-6 {
            return true;
        }
        let local_count = (self.scene_tone.mask_counts[0] as usize).min(MAX_RENDER_MASK_SLOTS);
        self.mask_data[..local_count].iter().any(|mask| {
            (mask.metadata[0] != 0 && mask.film_effects[0] > 1e-6)
                || is_self_illuminating_glow(mask)
        })
    }

    /// Whether an active Pixelate effect reads the block cache. Mirrors
    /// `pixelate_is_active` in `pixelate.wgsl`; block sizes too small to be
    /// cached still run the pass, which then writes nothing.
    pub(super) fn needs_pixelate_block_pass(&self) -> bool {
        let local_count = (self.scene_tone.mask_counts[0] as usize).min(MAX_RENDER_MASK_SLOTS);
        self.mask_data[..local_count].iter().any(|mask| {
            let [amount, block_size, ..] = mask.adjust_0;
            mask.metadata[0] != 0
                && mask.metadata[1] != 0
                && mask.metadata[3] >> MASK_EFFECT_ID_SHIFT == MaskEffect::Pixelate.shader_id()
                && (amount / 100.0).clamp(0.0, 1.0) > 1e-6
                && block_size > 1.0
        })
    }

    /// Whether an effect samples the image-light map (Image lights on Fog or
    /// Smoke), so the export tone prepass must build it.
    pub(super) fn needs_image_lights(&self) -> bool {
        let local_count = (self.scene_tone.mask_counts[0] as usize).min(MAX_RENDER_MASK_SLOTS);
        self.mask_data[..local_count]
            .iter()
            .any(medium_uses_image_lights)
    }

    /// Whether an active Relight effect reads the relighting surface derived
    /// from scene depth.
    pub(super) fn needs_relight_surface(&self) -> bool {
        let local_count = (self.scene_tone.mask_counts[0] as usize).min(MAX_RENDER_MASK_SLOTS);
        self.mask_data[..local_count].iter().any(|mask| {
            mask.metadata[0] != 0
                && mask.metadata[3] >> MASK_EFFECT_ID_SHIFT == MaskEffect::Relight.shader_id()
        })
    }

    /// What the relight shadow map depends on besides scene depth: the frame
    /// and, per channel, the light's position, depth, size and relief. `None`
    /// when no light uses the map. Amount, reach, colour, ambient and shadow
    /// strength are applied per pixel and leave the map valid.
    pub(super) fn relight_shadow_map_key(&self) -> Option<RelightShadowMapKey> {
        if self.scene_tone.scene_depth_present == 0 {
            return None;
        }
        let local_count = (self.scene_tone.mask_counts[0] as usize).min(MAX_RENDER_MASK_SLOTS);
        let mut lights = [None; RELIGHT_SHADOW_MAP_CHANNELS];
        for mask in &self.mask_data[..local_count] {
            if let Some(channel) = relight_shadow_channel(mask) {
                let [_, _, source_x, source_y] = mask.adjust_0;
                let [size, _, relief, _] = mask.adjust_2;
                lights[channel] = Some([source_x, source_y, mask.adjust_1[3], size, relief]);
            }
        }
        lights
            .iter()
            .any(Option::is_some)
            .then_some(RelightShadowMapKey {
                full_size: [self.camera.full_width, self.camera.full_height],
                lights,
            })
    }

    /// Traces every Relight shadow per pixel instead of reading the shadow map.
    #[cfg(test)]
    pub(super) fn with_relight_shadows_traced_per_pixel(mut self) -> Self {
        clear_relight_shadow_channels(&mut self.mask_data);
        self
    }

    pub(super) fn needs_blur_passes(&self) -> bool {
        let local_count = (self.scene_tone.mask_counts[0] as usize).min(MAX_RENDER_MASK_SLOTS);
        self.mask_data[..local_count].iter().any(|mask| {
            if mask.metadata[0] == 0 {
                return false;
            }
            matches!(
                mask.metadata[3] >> MASK_EFFECT_ID_SHIFT,
                id if id == MaskEffect::Blur.shader_id()
                    || id == MaskEffect::LensBlur.shader_id()
                    || id == MaskEffect::MotionBlur.shader_id()
                    || id == MaskEffect::RadialBlur.shader_id()
                    || id == MaskEffect::TiltShift.shader_id()
            )
        })
    }

    pub(super) fn needs_progressive_blur_passes(&self) -> bool {
        let local_count = (self.scene_tone.mask_counts[0] as usize).min(MAX_RENDER_MASK_SLOTS);
        self.mask_data[..local_count].iter().any(|mask| {
            mask.metadata[0] != 0
                && mask.metadata[3] >> MASK_EFFECT_ID_SHIFT == MaskEffect::Blur.shader_id()
        })
    }
}
