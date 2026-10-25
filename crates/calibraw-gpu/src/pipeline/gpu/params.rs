//! `GpuParams`: edit parameters packed into the uniform layouts the WGSL stages read.

use super::*;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(super) struct CameraUniforms {
    pub(super) black_point: f32,
    pub(super) highlight_clip: f32,
    pub(super) chroma_denoise: f32,
    pub(super) highlight_reconstruction: f32,
    pub(super) tone_analysis_scale: f32,
    pub(super) tone_guide_radius: f32,
    pub(super) demosaic_mode: f32,
    pub(super) dual_threshold: f32,
    pub(super) frequency_chroma: f32,
    /// [`ViewTransformMode::shader_value`].
    pub(super) view_transform_mode: u32,
    /// Pads the scalars to the 16-byte alignment of `highlight_options`.
    pub(super) padding_0: u32,
    pub(super) padding_1: u32,
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
    /// Weight in `[0, 1]` of the AI-denoised plane (G) of a packed Bayer raw
    /// texture against the original mosaic (R). The pipeline uploads 0 when its
    /// raw texture has no AI plane.
    pub(super) ai_cfa_weight: f32,
    /// Shorter edge in pixels of the whole photo at its native resolution
    /// ([`LoadedRaw::source_extent`]); `full_width` and `full_height` give the
    /// processing resolution. `resolution_scale` in common.wgsl reads both.
    pub(super) source_short_edge: u32,
    /// Chromatic aberration amount in `[0, 1]`, then the gains that white-
    /// balance the scene's red, green and blue (1 unless the scene is a
    /// camera-linear raster's unbalanced camera RGB).
    pub(super) chromatic_aberration: [f32; 4],
    /// The photo's lateral chromatic aberration of red and of blue
    /// ([`crate::pipeline::LateralChromaticAberration::coefficients`]),
    /// scaled by the amount.
    pub(super) lateral_ca_red: [f32; 4],
    pub(super) lateral_ca_blue: [f32; 4],
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

/// How `apply_view_node` maps graded RGB to display-linear RGB
/// (`VIEW_TRANSFORM_*` in common.wgsl).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ViewTransformMode {
    /// A developed raster at the reference tone curve keeps its rendered values.
    Passthrough,
    /// Scene radiance (RAWs, camera-linear and scene-referred rasters) takes
    /// the sigmoid view transform. `scene_working_at` in scene_source.wgsl
    /// also compresses these sources into Rec.2020.
    SceneSigmoid,
    /// A developed raster whose tone curve differs from the reference is
    /// retoned relative to it: each value is mapped back through the reference
    /// curve and forward through the edited one. The mapping is the identity
    /// at the reference curve, so tone edits start continuously from the
    /// rendered photo instead of jumping to the scene transform.
    DisplayRetone,
}

impl ViewTransformMode {
    pub(super) fn select(
        raw: &LoadedRaw,
        exposure: &ExposureParams,
        camera_linear_raster: bool,
    ) -> Self {
        if camera_linear_raster || !raw.is_pre_demosaiced_raster() || raw.scene_referred_raster {
            Self::SceneSigmoid
        } else if same_tone_curve(
            tone_sigmoid_coefficients(exposure),
            reference_sigmoid_coefficients(),
        ) {
            Self::Passthrough
        } else {
            Self::DisplayRetone
        }
    }

    pub(super) const fn shader_value(self) -> u32 {
        match self {
            Self::Passthrough => 0,
            Self::SceneSigmoid => 1,
            Self::DisplayRetone => 2,
        }
    }
}

/// The sigmoid of the edit: its Sigmoid settings with Contrast mapped onto the
/// sigmoid contrast.
pub(super) fn tone_sigmoid_coefficients(exposure: &ExposureParams) -> SigmoidCoefficients {
    sigmoid_coefficients(SigmoidParams {
        contrast: sigmoid_contrast_from_percent(exposure.contrast),
        ..exposure.sigmoid
    })
}

/// The sigmoid a developed raster is taken to be rendered with: the curve of
/// default settings.
pub(super) fn reference_sigmoid_coefficients() -> SigmoidCoefficients {
    sigmoid_coefficients(SigmoidParams::default())
}

/// Whether both coefficient sets describe the same tone curve. Hue
/// preservation and colour processing only change how a curve is applied.
/// Exact comparison is intended: default settings always derive bit-identical
/// coefficients, and any edit must leave passthrough.
fn same_tone_curve(a: SigmoidCoefficients, b: SigmoidCoefficients) -> bool {
    a.white_target == b.white_target
        && a.black_target == b.black_target
        && a.paper_exposure == b.paper_exposure
        && a.film_fog == b.film_fog
        && a.film_power == b.film_power
        && a.paper_power == b.paper_power
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
    /// The edit's sigmoid: white target, black target, log2 paper exposure, film fog.
    pub(super) sigmoid_curve: [f32; 4],
    /// Film power, paper power, hue preservation, colour processing.
    pub(super) sigmoid_power: [f32; 4],
    /// [`reference_sigmoid_coefficients`], packed like `sigmoid_curve` and
    /// `sigmoid_power`; [`ViewTransformMode::DisplayRetone`] inverts it.
    pub(super) reference_sigmoid_curve: [f32; 4],
    pub(super) reference_sigmoid_power: [f32; 4],
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
    /// The edit's LUT (`GpuParams::with_look_lut`): amount (0 without a LUT),
    /// lattice points per axis, 0, 0.
    pub(super) look_lut: [f32; 4],
    /// The LUT's domain minimum, then the reciprocal of its extent, per channel.
    pub(super) look_lut_domain_min: [f32; 4],
    pub(super) look_lut_domain_scale: [f32; 4],
}

const _: () =
    assert!(std::mem::size_of::<SceneToneUniforms>() == SCENE_TONE_UNIFORMS_SIZE_BYTES as usize);

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(super) struct EffectsUniforms {
    // Texture, clarity, dehaze, reserved.
    pub(super) presence: [f32; 4],
    // Sharpening amount, radius, detail, masking.
    pub(super) sharpen: [f32; 4],
    // Radius of the shared Glow diffusion, reserved, reserved, reserved.
    pub(super) glow_diffusion: [f32; 4],
    // The frame and calibration of Vignette effects.
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
const _: () = assert!(GPU_STAGE_UNIFORM_SIZE_BYTES == 2_336);

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(super) struct MaskData {
    pub(super) metadata: [u32; 4],
    pub(super) adjust_0: [f32; 4],
    pub(super) adjust_1: [f32; 4],
    pub(super) adjust_2: [f32; 4],
    // Per-slot switches of effect slots (`effect_lanes::options`).
    pub(super) effect_options: [f32; 4],
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

impl MaskData {
    /// The slot's [`MaskEffect::shader_id`] (`Common::mask_effect_id`).
    pub(super) fn effect_id(&self) -> u32 {
        self.metadata[3] >> MASK_EFFECT_ID_SHIFT
    }

    /// Whether the slot is enabled and changes the image: an adjustment that
    /// is not neutral, or an active effect (`Common::mask_slot_active`).
    pub(super) fn is_active(&self) -> bool {
        self.metadata[0] != 0 && self.metadata[1] != 0
    }

    /// Whether the slot is active and applies `effect`
    /// (`Common::mask_slot_applies`).
    pub(super) fn applies(&self, effect: MaskEffect) -> bool {
        self.is_active() && self.effect_id() == effect.shader_id()
    }
}

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
    /// The LUT `scene_tone.look_lut` describes, uploaded with the parameters.
    look_lut: Option<Arc<CubeLut>>,
    /// The camera profile `camera` describes, whose tables the pipeline
    /// uploads with the parameters.
    pub(super) profile_color: Arc<ProfileColor>,
}

impl GpuParams {
    /// The camera uniforms for a pipeline whose raw texture does or does not
    /// carry an AI-denoised plane. Without one the AI weight is forced to 0, so
    /// the shader never blends with the zero G channel of an `R16Uint` texture.
    pub(super) fn camera_for_raw_texture(&self, has_ai_cfa: bool) -> CameraUniforms {
        let mut camera = self.camera;
        if !has_ai_cfa {
            camera.ai_cfa_weight = 0.0;
        }
        camera
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

    /// The mask slots the shaders read: the first `mask_counts[0]`.
    fn mask_slots(&self) -> &[MaskData] {
        let count = (self.scene_tone.mask_counts[0] as usize).min(MAX_RENDER_MASK_SLOTS);
        &self.mask_data[..count]
    }
}

/// An enabled Glow slot that emits into the shared Glow diffusion. Mirrors
/// `mask_glow_self_illuminating` in `mask_effects/glow.wgsl`.
pub(super) fn is_self_illuminating_glow(mask: &MaskData) -> bool {
    mask.applies(MaskEffect::Glow) && mask.adjust_0[3] > 0.5
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
    /// The camera profile colour rendered: `raw`'s own, or another profile
    /// of the same photo.
    pub(super) color: &'a ProfileColor,
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
        Self::new_for_tile_with_profile_color(
            exposure,
            masks,
            raw,
            &raw.profile_color,
            [tile_origin_x, tile_origin_y],
            [full_width, full_height],
        )
    }

    /// [`Self::new`] rendering `profile_color` instead of `raw`'s own: the
    /// colour another camera profile gives the same photo
    /// (`LoadedRaw::profile_color_for_selection`), to preview it.
    pub fn new_with_profile_color(
        exposure: &ExposureParams,
        masks: &MaskStack,
        raw: &LoadedRaw,
        profile_color: &Arc<ProfileColor>,
    ) -> Self {
        Self::new_for_tile_with_profile_color(
            exposure,
            masks,
            raw,
            profile_color,
            [0, 0],
            [raw.width, raw.height],
        )
    }

    /// [`Self::new_for_tile`] rendering `profile_color` instead of `raw`'s
    /// own ([`Self::new_with_profile_color`]).
    pub fn new_for_tile_with_profile_color(
        exposure: &ExposureParams,
        masks: &MaskStack,
        raw: &LoadedRaw,
        profile_color: &Arc<ProfileColor>,
        [tile_origin_x, tile_origin_y]: [i32; 2],
        [full_width, full_height]: [u32; 2],
    ) -> Self {
        let context = GpuParamContext {
            exposure,
            masks,
            raw,
            color: profile_color,
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
            look_lut: None,
            profile_color: Arc::clone(profile_color),
        }
    }

    /// Applies `look`'s LUT at its amount after tone mapping (look_lut.wgsl).
    /// `None`, or an amount of 0, leaves the image as without a LUT.
    pub fn with_look_lut(mut self, look: Option<&ResolvedLook>) -> Self {
        let look = look.filter(|look| look.amount > 0.0);
        self.look_lut = look.map(|look| Arc::clone(&look.lut));
        let (uniform, domain_min, domain_scale) = match look {
            Some(look) => {
                let lut = &look.lut;
                let [min_r, min_g, min_b] = lut.domain_min();
                let [max_r, max_g, max_b] = lut.domain_max();
                (
                    [look.amount.min(1.0), lut.size() as f32, 0.0, 0.0],
                    [min_r, min_g, min_b, 0.0],
                    [
                        1.0 / (max_r - min_r),
                        1.0 / (max_g - min_g),
                        1.0 / (max_b - min_b),
                        0.0,
                    ],
                )
            }
            None => ([0.0; 4], [0.0; 4], [0.0; 4]),
        };
        self.scene_tone.look_lut = uniform;
        self.scene_tone.look_lut_domain_min = domain_min;
        self.scene_tone.look_lut_domain_scale = domain_scale;
        self
    }

    /// The LUT these parameters apply, if any.
    pub fn look_lut(&self) -> Option<&CubeLut> {
        self.look_lut.as_deref()
    }

    /// The scene and tone uniforms, with the LUT switched off when the
    /// pipeline could not upload it.
    pub(super) fn scene_tone_for_upload(&self, look_lut_uploaded: bool) -> SceneToneUniforms {
        let mut scene_tone = self.scene_tone;
        if !look_lut_uploaded {
            scene_tone.look_lut = [0.0; 4];
        }
        scene_tone
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

    /// Chromatic aberration runs between the demosaic finish and colour
    /// denoise whenever its amount is above zero, even for a photo without
    /// lateral aberration: its fringe removal still applies.
    pub(super) fn needs_chromatic_aberration_passes(&self) -> bool {
        self.camera.chromatic_aberration[0] > 0.0
    }

    /// Dual demosaicing with a zero threshold keeps the reference demosaic
    /// (`dual_blend` in dual_demosaic.wgsl), as darktable's does.
    pub(super) fn needs_dual_demosaic_passes(&self) -> bool {
        self.camera.demosaic_mode >= 1.5 && self.camera.dual_threshold > 0.0
    }

    /// How many Texture and Clarity cascade levels the scene effects pass
    /// reads: every level for Clarity, fewer for Texture alone, none without
    /// either. Covers every call of `apply_texture_and_clarity_values`
    /// (creative_effects.wgsl): the global values and adjustment masks.
    pub(super) fn detail_pyramid_levels(&self) -> Option<usize> {
        let [global_texture, global_clarity, ..] = self.effects.presence;
        let local_values = self
            .mask_slots()
            .iter()
            .filter(|mask| mask.applies(MaskEffect::Adjustment))
            .map(|mask| (mask.adjust_2[1], mask.adjust_2[2]));
        let (mut texture, mut clarity) = (global_texture.abs() > 1e-6, global_clarity.abs() > 1e-6);
        for (local_texture, local_clarity) in local_values {
            texture |= local_texture.abs() > 1e-6;
            clarity |= local_clarity.abs() > 1e-6;
        }
        if clarity {
            Some(DETAIL_PYRAMID_LEVELS)
        } else if texture {
            Some(DETAIL_PYRAMID_TEXTURE_LEVELS)
        } else {
            None
        }
    }

    /// Whether anything after the scene tone pass runs. Dehaze is applied
    /// in the scene preparation and needs none of these passes.
    pub(super) fn needs_intermediate_adjustment_passes(&self) -> bool {
        let [global_texture, global_clarity, ..] = self.effects.presence;
        let global_effects = self.scene_tone.saturation.abs() > 1e-6
            || self.scene_tone.vibrance.abs() > 1e-6
            || global_texture.abs() > 1e-6
            || global_clarity.abs() > 1e-6;
        let local_effects = self.mask_slots().iter().any(|local| {
            if !local.is_active() {
                return false;
            }
            let effect_id = local.effect_id();
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
            let curves = local.metadata[2] != 0;
            let [saturation, texture, clarity, _dehaze] = local.adjust_2;
            let presence_or_saturation = [saturation, texture, clarity]
                .iter()
                .any(|value| value.abs() > 1e-6);
            tone || white_balance || curves || presence_or_saturation
        });
        global_effects || local_effects
    }

    pub(super) fn needs_glow_passes(&self) -> bool {
        self.mask_slots().iter().any(is_self_illuminating_glow)
    }

    /// Whether an active Pixelate effect reads the block cache. Mirrors
    /// `pixelate_is_active` in `pixelate.wgsl`; block sizes too small to be
    /// cached still run the pass, which then writes nothing.
    pub(super) fn needs_pixelate_block_pass(&self) -> bool {
        self.mask_slots().iter().any(|mask| {
            let [amount, block_size, ..] = mask.adjust_0;
            mask.applies(MaskEffect::Pixelate)
                && (amount / 100.0).clamp(0.0, 1.0) > 1e-6
                && block_size > 1.0
        })
    }

    /// Whether an effect samples the image-light map (Image lights on Fog or
    /// Smoke), so the export tone prepass must build it.
    pub(super) fn needs_image_lights(&self) -> bool {
        self.mask_slots().iter().any(medium_uses_image_lights)
    }

    /// Whether global or mask Dehaze applies (`dehaze_amount_at` in
    /// scene_adjustments.wgsl), so the haze map must be built.
    pub(super) fn needs_haze(&self) -> bool {
        self.effects.presence[2].abs() > 1e-6
            || self
                .mask_slots()
                .iter()
                .any(|mask| mask.applies(MaskEffect::Adjustment) && mask.adjust_2[3].abs() > 1e-7)
    }

    /// Whether an effect reads `map`, which the tone stage, or the export tone
    /// prepass, must then build.
    pub(super) fn needs_full_image_map(&self, map: FullImageMap) -> bool {
        match map {
            FullImageMap::ImageLights => self.needs_image_lights(),
            FullImageMap::Haze => self.needs_haze(),
        }
    }

    /// Whether an effect upsamples scene depth with the image as its guide:
    /// active Fog, Smoke or Relight in a visible slot while scene depth is
    /// present. The creative pass then reads the guide map, so it is built
    /// right before it.
    pub(super) fn needs_scene_depth_guide(&self) -> bool {
        if self.scene_tone.scene_depth_present == 0 {
            return false;
        }
        self.mask_slots().iter().any(|mask| {
            [MaskEffect::Fog, MaskEffect::Smoke, MaskEffect::Relight]
                .into_iter()
                .any(|effect| mask.applies(effect))
        })
    }

    /// Whether an active Relight effect in a visible slot reads the
    /// relighting surface derived from scene depth. Mirrors the slot checks
    /// of `scene_light_at` in scene_lights.wgsl.
    pub(super) fn needs_relight_surface(&self) -> bool {
        self.mask_slots()
            .iter()
            .any(|mask| mask.applies(MaskEffect::Relight))
    }

    /// What the relight shadow map depends on besides scene depth: the frame
    /// and, per channel, the light's position, depth, size and relief. `None`
    /// when no light uses the map. Amount, reach, colour, ambient and shadow
    /// strength are applied per pixel and leave the map valid.
    pub(super) fn relight_shadow_map_key(&self) -> Option<RelightShadowMapKey> {
        if self.scene_tone.scene_depth_present == 0 {
            return None;
        }
        let mut lights = [None; RELIGHT_SHADOW_MAP_CHANNELS];
        for mask in self.mask_slots() {
            if let Some(channel) = relight_shadow_channel(mask) {
                use super::effect_lanes::relight;
                lights[channel] = Some(
                    [
                        relight::SOURCE_X,
                        relight::SOURCE_Y,
                        relight::DEPTH,
                        relight::SIZE,
                        relight::RELIEF,
                    ]
                    .map(|lane| mask.lane(lane)),
                );
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
        self.mask_slots().iter().any(|mask| {
            [
                MaskEffect::Blur,
                MaskEffect::LensBlur,
                MaskEffect::MotionBlur,
                MaskEffect::RadialBlur,
                MaskEffect::TiltShift,
            ]
            .into_iter()
            .any(|effect| mask.applies(effect))
        })
    }

    pub(super) fn needs_progressive_blur_passes(&self) -> bool {
        self.mask_slots()
            .iter()
            .any(|mask| mask.applies(MaskEffect::Blur))
    }
}
