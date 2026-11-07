//! `GpuParams`: edit parameters packed into the uniform layouts the WGSL stages read.

use super::*;
use crate::pipeline::effect_lanes::{glow, light_rays, neon, pixelate, relight};
use crate::pipeline::{EffectOption, EffectPass, Lane};

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
    /// The film negative `film_positive` in scene_source.wgsl inverts: its
    /// base in the units of the scene texture, then the mode (`FILM_MODE_*`)
    /// as a float.
    pub(super) film_base: [f32; 4],
    /// The exponent of each channel's density, then the print exposure in EV.
    pub(super) film_exponents: [f32; 4],
    /// Gains of the positive's camera RGB, then 0. They apply the white
    /// balance relative to the as-shot one, less any the camera transform
    /// applies.
    pub(super) film_gains: [f32; 4],
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

/// How `film_positive` in scene_source.wgsl reads the scene: as it is, or as
/// a colour or black-and-white negative (`FILM_MODE_*` there, where colour is
/// any value but the other two).
pub(super) const FILM_MODE_POSITIVE: u32 = 0;
pub(super) const FILM_MODE_COLOR: u32 = 1;
pub(super) const FILM_MODE_MONOCHROME: u32 = 2;

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
        // The positive of a negative scan is scene radiance, whatever the
        // scan's own encoding.
        if camera_linear_raster
            || !raw.is_pre_demosaiced_raster()
            || raw.scene_referred_raster
            || !exposure.film.is_positive()
        {
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
    /// Keeps the next vector aligned (the shader's `_padding`).
    pub(super) _padding: u32,
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
    /// [`ExposureParams::raster_temperature`] on a rendered photo, 0 on any
    /// other, then 0, 0, 0.
    pub(super) raster_temperature: [f32; 4],
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
    // The frame and calibration of Vignette effects: the output width and height, two reserved,
    // then the rows of the homography from normalized source to output position (0–1 across
    // the output), each with one reserved.
    pub(super) vignette_frame: [f32; 4],
    pub(super) vignette_projection: [[f32; 4]; 3],
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
const _: () = assert!(GPU_STAGE_UNIFORM_SIZE_BYTES == 2_432);

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(super) struct MaskData {
    pub(super) metadata: [u32; 4],
    pub(super) adjust_0: [f32; 4],
    pub(super) adjust_1: [f32; 4],
    pub(super) adjust_2: [f32; 4],
    // Per-slot switches of effect slots (`effect_lanes::options` in calibraw-core).
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

    /// The effect of an active slot.
    pub(super) fn active_effect(&self) -> Option<MaskEffect> {
        self.is_active()
            .then(|| MaskEffect::from_shader_id(self.effect_id()))
            .flatten()
    }

    /// Whether the slot is active and its effect runs in `pass`.
    pub(super) fn runs_in(&self, pass: EffectPass) -> bool {
        self.active_effect()
            .is_some_and(|effect| effect.runs_in(pass))
    }

    /// An effect slot's lane: component `n % 4` of `adjust_{n / 4}`
    /// ([`EffectLanes::rows`]).
    pub(super) fn lane(&self, lane: Lane) -> f32 {
        let row = match lane.index() / 4 {
            0 => &self.adjust_0,
            1 => &self.adjust_1,
            _ => &self.adjust_2,
        };
        row[lane.index() % 4]
    }

    pub(super) fn set_lane(&mut self, lane: Lane, value: f32) {
        // `Lane::new` keeps every lane below 12.
        let row = match lane.index() / 4 {
            0 => &mut self.adjust_0,
            1 => &mut self.adjust_1,
            _ => &mut self.adjust_2,
        };
        row[lane.index() % 4] = value;
    }

    /// A percentage lane as a fraction in [0, 1] (`mask_effect_percent`).
    pub(super) fn percent(&self, lane: Lane) -> f32 {
        (self.lane(lane) / 100.0).clamp(0.0, 1.0)
    }

    pub(super) fn option(&self, option: EffectOption) -> f32 {
        self.effect_options[option.index()]
    }
}

#[derive(Clone, Debug)]
pub struct GpuParams {
    pub(super) camera: CameraUniforms,
    pub(super) scene_tone: SceneToneUniforms,
    pub(super) effects: EffectsUniforms,
    mask_data: Box<[MaskData]>,
    /// The depth result the scene bake is made from (`scene`).
    pub(super) depth_result: Option<SceneDepth>,
    /// How carefully the light bake is built for this render.
    scene_light_quality: SceneLightQuality,
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
    mask.applies(MaskEffect::Glow) && mask.lane(glow::SELF_ILLUMINATING) > 0.5
}

/// A Glow slot that lights the scene. Mirrors `mask_glow_lights_scene` in
/// `mask_effects/glow.wgsl`.
fn glow_lights_scene(mask: &MaskData) -> bool {
    is_self_illuminating_glow(mask)
        && mask.percent(glow::AMOUNT) > 1e-6
        && mask.percent(glow::SCENE_LIGHT) > 1e-6
}

/// A Neon slot that lights the scene. Mirrors `neon_lights_scene` in
/// `mask_effects/neon.wgsl`.
fn neon_lights_scene(mask: &MaskData) -> bool {
    mask.applies(MaskEffect::Neon)
        && mask.percent(neon::AMOUNT) > 1e-6
        && mask.percent(neon::SCENE_LIGHT) > 1e-6
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

/// The view node's options (`grade_options`), packed alike for the edit and
/// each mask: colour grading's blending and balance as fractions, the hue
/// rotation in degrees and Brightness, -100 to 100.
pub(super) fn pack_view_options(
    grading: crate::pipeline::ColorGrading,
    hue: f32,
    brightness: f32,
) -> [f32; 4] {
    [
        (grading.blending / 100.0).clamp(0.0, 1.0),
        (grading.balance / 100.0).clamp(-1.0, 1.0),
        effect_params::adjustment::HUE.clamp(hue),
        effect_params::adjustment::BRIGHTNESS.clamp(brightness),
    ]
}

/// Per-channel `coefficients` (R, G1, B, G2) with both greens at their mean
/// when the sensor has a second green, for stages that treat the greens as
/// one colour.
pub(super) fn canonicalize_greens(mut coefficients: [f32; 4], green2_present: bool) -> [f32; 4] {
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
        let mask_data = pack_mask_params(masks, raw.capture_metadata.shutter_seconds);
        Self {
            camera: pack_camera_params(&context),
            scene_tone: pack_scene_tone_params(&context),
            effects: pack_effect_params(&context, &mask_data),
            mask_data,
            depth_result: masks.scene_depth.clone(),
            scene_light_quality: SceneLightQuality::default(),
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

    /// Renders the light bake at `quality`: Draft while the user drags a
    /// control, a light or the view, Final otherwise (the default, and what
    /// exports use).
    pub fn with_scene_light_quality(mut self, quality: SceneLightQuality) -> Self {
        self.scene_light_quality = quality;
        self
    }

    /// Whether the render builds a light bake, whose tier
    /// `with_scene_light_quality` sets: a Draft render of these parameters
    /// needs a Final one once the drag ends.
    pub fn uses_scene_light_quality(&self) -> bool {
        self.needs_light_bake()
    }

    /// Places Vignette effects in the output frame of `geometry`: the shaders map each source
    /// position to its output position.
    pub fn with_vignette_geometry(mut self, geometry: GeometryTransform) -> Self {
        let mapping =
            GeometryMapping::new(geometry, self.camera.full_width, self.camera.full_height);
        let [output_width, output_height] = mapping.output_size();
        self.effects.vignette_frame = [output_width, output_height, 0.0, 0.0];
        let projection = mapping.output_uv_from_source_uv().rows();
        self.effects.vignette_projection = projection.map(|[x, y, w]| [x, y, w, 0.0]);
        let full = [
            self.camera.full_width.max(1) as f32,
            self.camera.full_height.max(1) as f32,
        ];
        let short = full[0].min(full[1]);
        let direction_of = |direction| {
            source_direction(
                projection,
                [output_width, output_height],
                [full[0] / short, full[1] / short],
                direction,
            )
        };
        let count = (self.scene_tone.mask_counts[0] as usize).min(MAX_RENDER_MASK_SLOTS);
        orient_rain_slots(&mut self.mask_data[..count], direction_of);
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
            let Some(effect) = local.active_effect() else {
                return false;
            };
            // Effects after the display transform (Grain, Vignette) or in the
            // scene tone pass (Sharpen) need none of these passes. Their
            // packed controls are not adjustments.
            if effect != MaskEffect::Adjustment {
                return effect.passes().iter().any(|pass| pass.is_intermediate());
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

    /// Whether the creative pass (`apply_creative_effects`) changes the image:
    /// a slot effect that runs in it, Glow or Light Rays, or light that Glow and
    /// Neon cast on the scene. Without any it copies its input unchanged, and the
    /// output stage skips it: it is the largest program of the chain, and on
    /// some mobile GPUs (Adreno) even that copy costs as much as all other
    /// passes together.
    pub(super) fn needs_creative_pass(&self) -> bool {
        self.needs_scene_emission()
            || self.mask_slots().iter().any(|mask| {
                mask.runs_in(EffectPass::Creative) || mask.runs_in(EffectPass::CreativeLight)
            })
    }

    pub(super) fn needs_glow_passes(&self) -> bool {
        self.mask_slots().iter().any(is_self_illuminating_glow)
    }

    /// Whether an active Pixelate effect reads the block cache. Mirrors
    /// `pixelate_is_active` in `pixelate.wgsl`; block sizes too small to be
    /// cached still run the pass, which then writes nothing.
    pub(super) fn needs_pixelate_block_pass(&self) -> bool {
        self.mask_slots().iter().any(|mask| {
            mask.applies(MaskEffect::Pixelate)
                && mask.percent(pixelate::AMOUNT) > 1e-6
                && mask.lane(pixelate::BLOCK_SIZE) > 1.0
        })
    }

    /// Whether an effect samples the image-light map (Image lights on Fog,
    /// Smoke or Rain), so the export tone prepass must build it.
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

    /// Whether an effect runs a pass before the creative effects (Wet
    /// Surfaces).
    pub(super) fn needs_wet_surfaces_pass(&self) -> bool {
        self.mask_slots()
            .iter()
            .any(|mask| mask.runs_in(EffectPass::BeforeCreative))
    }

    /// Whether an effect runs a pass after the creative effects (Rain, Wet
    /// Surfaces).
    pub(super) fn needs_weather_air_pass(&self) -> bool {
        self.mask_slots()
            .iter()
            .any(|mask| mask.runs_in(EffectPass::AfterCreative))
    }

    /// Whether an active effect reads the scene bake's geometry
    /// ([`MaskEffect::scene_needs`]), which needs a depth result.
    pub(super) fn needs_scene_geometry(&self) -> bool {
        self.depth_result.is_some()
            && (self.needs_scene_emission()
                || self
                    .mask_slots()
                    .iter()
                    .filter_map(MaskData::active_effect)
                    .any(|effect| effect.scene_needs().geometry))
    }

    /// Whether an active effect reads the scene bake's working image
    /// ([`MaskEffect::scene_needs`]), which exists without a depth result.
    pub(super) fn needs_scene_image(&self) -> bool {
        self.mask_slots()
            .iter()
            .filter_map(MaskData::active_effect)
            .any(|effect| effect.scene_needs().image)
    }

    /// Whether an active effect reads the scene bake: its working image, and
    /// with a depth result its geometry.
    pub(super) fn needs_scene_bake(&self) -> bool {
        self.needs_scene_geometry() || self.needs_scene_image()
    }

    /// Whether an active effect reads the light bake and a light casts
    /// shadows into it.
    pub(super) fn needs_light_bake(&self) -> bool {
        self.needs_scene_geometry()
            && self
                .mask_slots()
                .iter()
                .filter_map(MaskData::active_effect)
                .any(|effect| effect.scene_needs().light_bake)
            && self
                .mask_slots()
                .iter()
                .any(|mask| scene_bake_light_channel(mask).is_some())
    }

    /// Whether an active effect reads the air light (L3), which needs the
    /// light bake.
    pub(super) fn needs_air_light(&self) -> bool {
        self.needs_light_bake()
            && self
                .mask_slots()
                .iter()
                .filter_map(MaskData::active_effect)
                .any(|effect| effect.scene_needs().air_light)
    }

    /// The light bake's tier.
    pub(super) fn scene_light_quality(&self) -> SceneLightQuality {
        self.scene_light_quality
    }

    /// The lights of the light bake in their channels
    /// (`assign_scene_bake_lights`), and the tier to build them in.
    pub(super) fn scene_light_set(&self) -> SceneLightSet {
        let mut lights = [None; SCENE_SHADOW_LIGHTS];
        for mask in self.mask_slots() {
            let Some(channel) = scene_bake_light_channel(mask) else {
                continue;
            };
            lights[channel] = Some(if mask.applies(MaskEffect::LightRays) {
                // A point light at the farthest surface (control depth 100).
                SceneShadowLight {
                    kind: SceneLightKind::LightRays,
                    source: [
                        mask.lane(light_rays::SOURCE_X),
                        mask.lane(light_rays::SOURCE_Y),
                    ],
                    light_depth: 100.0,
                    relief_scale: 1.0,
                    size: 0.0,
                    reach: mask.lane(light_rays::LENGTH),
                }
            } else {
                SceneShadowLight {
                    kind: SceneLightKind::Relight,
                    source: [mask.lane(relight::SOURCE_X), mask.lane(relight::SOURCE_Y)],
                    light_depth: mask.lane(relight::LIGHT_DEPTH),
                    relief_scale: mask.lane(relight::RELIEF_SCALE) / 100.0,
                    size: mask.percent(relight::SIZE),
                    reach: mask.lane(relight::REACH),
                }
            });
        }
        SceneLightSet {
            lights,
            quality: self.scene_light_quality,
        }
    }

    /// Whether an effect lights the scene by emission (scene_emission.wgsl),
    /// so the output stage builds its light. Mirrors `scene_emission_present`.
    pub(super) fn needs_scene_emission(&self) -> bool {
        self.mask_slots()
            .iter()
            .any(|mask| glow_lights_scene(mask) || neon_lights_scene(mask))
    }

    /// Whether a Neon slot lights the scene, which reads the contour grid.
    fn needs_contours(&self) -> bool {
        self.mask_slots().iter().any(neon_lights_scene)
    }

    /// Whether an effect reads `map`, which the tone stage, or the export tone
    /// prepass, must then build.
    pub(super) fn needs_full_image_map(&self, map: FullImageMap) -> bool {
        match map {
            FullImageMap::ImageLights => self.needs_image_lights(),
            FullImageMap::Haze => self.needs_haze(),
            FullImageMap::Contours => self.needs_contours(),
        }
    }

    pub(super) fn needs_blur_passes(&self) -> bool {
        self.mask_slots()
            .iter()
            .any(|mask| mask.runs_in(EffectPass::Blur))
    }

    pub(super) fn needs_progressive_blur_passes(&self) -> bool {
        self.mask_slots()
            .iter()
            .any(|mask| mask.applies(MaskEffect::Blur))
    }
}
