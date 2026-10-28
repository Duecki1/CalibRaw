use super::film_negative::FilmSource;
use super::noise::DenoiseQuality;
use super::sigmoid::SigmoidParams;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum DemosaicMode {
    #[default]
    Reference,
    FrequencyDomainChroma,
    Dual,
}

impl DemosaicMode {
    pub const fn shader_value(self) -> f32 {
        match self {
            Self::Reference => 0.0,
            Self::FrequencyDomainChroma => 1.0,
            Self::Dual => 2.0,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Reference => "Reference (RCD / Markesteijn 3-pass)",
            Self::FrequencyDomainChroma => "Frequency-domain chroma",
            Self::Dual => "Dual demosaic (robust)",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum HighlightReconstructionMethod {
    Off,
    Lch,
    #[default]
    InpaintOpposed,
}

impl HighlightReconstructionMethod {
    pub const fn shader_value(self) -> f32 {
        match self {
            Self::Off => 0.0,
            Self::Lch => 1.0,
            Self::InpaintOpposed => 2.0,
        }
    }
}

pub const MAX_POINT_CURVE_POINTS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PointCurve {
    #[serde(deserialize_with = "crate::migrations::point_curve_slots::deserialize")]
    pub points: [[f32; 2]; MAX_POINT_CURVE_POINTS],
    pub len: u32,
}

impl PointCurve {
    pub const fn linear() -> Self {
        let mut points = [[1.0, 1.0]; MAX_POINT_CURVE_POINTS];
        points[0] = [0.0, 0.0];
        Self { points, len: 2 }
    }

    pub fn reset(&mut self) {
        *self = Self::linear();
    }

    pub fn is_identity(&self) -> bool {
        let len = self.len.clamp(2, MAX_POINT_CURVE_POINTS as u32) as usize;
        if self.points[0][0].abs() > 1e-6 || (self.points[len - 1][0] - 1.0).abs() > 1e-6 {
            return false;
        }
        self.points[..len]
            .iter()
            .all(|point| (point[1] - point[0]).abs() <= 1e-6)
    }

    pub fn sanitize(&mut self) {
        const MIN_X_GAP: f32 = 0.005;

        self.len = self.len.clamp(2, MAX_POINT_CURVE_POINTS as u32);
        let len = self.len as usize;
        for index in 0..len {
            let lower = if index == 0 {
                0.0
            } else {
                self.points[index - 1][0] + MIN_X_GAP
            };
            let remaining = (len - 1 - index) as f32;
            let upper = 1.0 - MIN_X_GAP * remaining;
            self.points[index][0] = self.points[index][0].clamp(lower, upper.max(lower));
            self.points[index][1] = self.points[index][1].clamp(0.0, 1.0);
        }
        for point in &mut self.points[len..] {
            *point = [1.0, 1.0];
        }
    }
}

impl Default for PointCurve {
    fn default() -> Self {
        Self::linear()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ColorGradeWheel {
    pub hue: f32,
    pub saturation: f32,
    pub luminance: f32,
}

impl Default for ColorGradeWheel {
    fn default() -> Self {
        Self {
            hue: 0.0,
            saturation: 0.0,
            luminance: 0.0,
        }
    }
}

impl ColorGradeWheel {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn is_neutral(self) -> bool {
        self.saturation.abs() <= 1e-6 && self.luminance.abs() <= 1e-6
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ColorGrading {
    pub shadows: ColorGradeWheel,
    pub midtones: ColorGradeWheel,
    pub highlights: ColorGradeWheel,
    pub global: ColorGradeWheel,
    pub blending: f32,
    pub balance: f32,
}

impl Default for ColorGrading {
    fn default() -> Self {
        Self {
            shadows: ColorGradeWheel::default(),
            midtones: ColorGradeWheel::default(),
            highlights: ColorGradeWheel::default(),
            global: ColorGradeWheel::default(),
            blending: 50.0,
            balance: 0.0,
        }
    }
}

impl ColorGrading {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn is_neutral(self) -> bool {
        self.shadows.is_neutral()
            && self.midtones.is_neutral()
            && self.highlights.is_neutral()
            && self.global.is_neutral()
    }
}

const DARKTABLE_SIGMOID_CONTRAST_SOFT_MIN: f32 = 0.7;
const DARKTABLE_SIGMOID_CONTRAST_SOFT_MAX: f32 = 3.0;
pub const MIN_TEMPERATURE_KELVIN: f32 = 1_901.0;
pub const MAX_TEMPERATURE_KELVIN: f32 = 25_000.0;
/// Limits of the white-balance tint the sliders show, in Lightroom units:
/// 3000 × the CIE 1960 uv distance from the Planckian locus along the
/// isotherm, positive for whites greener than the locus, which balancing
/// turns magenta.
pub const MIN_WHITE_BALANCE_TINT: f32 = -150.0;
pub const MAX_WHITE_BALANCE_TINT: f32 = 150.0;
/// Change of the shown tint per unit of the stored tint offset.
///
/// Stored offsets keep the scale and sign they had when the tint was
/// darktable's Y multiplier (100 per unit of multiplier, positive greener):
/// near neutral daylight one such unit moved the white about 5 tints towards
/// green. Edits saved by older versions, and older versions reading newer
/// edits, therefore keep about the same tint.
pub const WHITE_BALANCE_TINT_PER_OFFSET: f32 = -5.0;
/// Limit of stored tint offsets: the full range of the darktable multiplier
/// (0.135 to 2.326) they were defined on.
pub const GLOBAL_TINT_OFFSET_LIMIT: f32 = 219.1;
pub const GLOBAL_TEMPERATURE_LIMIT: f32 = 500.0;

pub fn temperature_kelvin_from_offset(base_kelvin: f32, offset_mired: f32) -> f32 {
    let base_mired =
        1_000_000.0 / base_kelvin.clamp(MIN_TEMPERATURE_KELVIN, MAX_TEMPERATURE_KELVIN);
    (1_000_000.0 / (base_mired - offset_mired).max(1.0))
        .clamp(MIN_TEMPERATURE_KELVIN, MAX_TEMPERATURE_KELVIN)
}

pub fn temperature_offset_from_kelvin(base_kelvin: f32, target_kelvin: f32) -> f32 {
    let base = base_kelvin.clamp(MIN_TEMPERATURE_KELVIN, MAX_TEMPERATURE_KELVIN);
    let target = target_kelvin.clamp(MIN_TEMPERATURE_KELVIN, MAX_TEMPERATURE_KELVIN);
    (1_000_000.0 / base - 1_000_000.0 / target)
        .clamp(-GLOBAL_TEMPERATURE_LIMIT, GLOBAL_TEMPERATURE_LIMIT)
}

/// Shown tint for a stored offset from the as-shot tint `base_tint`.
pub fn white_balance_tint_from_offset(base_tint: f32, offset: f32) -> f32 {
    (base_tint + offset * WHITE_BALANCE_TINT_PER_OFFSET)
        .clamp(MIN_WHITE_BALANCE_TINT, MAX_WHITE_BALANCE_TINT)
}

/// Stored offset from the as-shot tint `base_tint` for a shown tint.
pub fn white_balance_tint_offset(base_tint: f32, target_tint: f32) -> f32 {
    ((target_tint.clamp(MIN_WHITE_BALANCE_TINT, MAX_WHITE_BALANCE_TINT) - base_tint)
        / WHITE_BALANCE_TINT_PER_OFFSET)
        .clamp(-GLOBAL_TINT_OFFSET_LIMIT, GLOBAL_TINT_OFFSET_LIMIT)
}

/// The global white balance of an edit.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum WhiteBalance {
    /// Each photo's own as-shot white balance.
    #[default]
    AsShot,
    /// An absolute white balance. Copies and presets give every photo this
    /// white balance, limited to the slider ranges of its camera.
    Custom {
        /// Correlated colour temperature in kelvin.
        kelvin: f32,
        /// Tint in the units the slider shows, positive magenta (see
        /// [`MIN_WHITE_BALANCE_TINT`]).
        tint: f32,
        /// Temperature and tint offsets of this white balance from the
        /// as-shot white balance of the photo it was last set or opened on,
        /// written for readers up to v1.3. On a photo it was pasted into they
        /// lag behind until that photo is opened.
        ///
        /// migration: remove in v2.0.0 (`migrations::white_balance_offsets`).
        older_reader_offsets: [f32; 2],
    },
    /// Offsets from the as-shot white balance, the only form v1.3 and older
    /// saved: `temperature` in mired, `tint` in stored tint units (see
    /// [`WHITE_BALANCE_TINT_PER_OFFSET`]). Turning them into [`Self::Custom`]
    /// needs the photo, so they are converted when it is opened
    /// (`LoadedRaw::resolve_white_balance`); until then copies and presets
    /// move the shift as before.
    ///
    /// migration: remove in v2.0.0 (`migrations::white_balance_offsets`).
    LegacyOffset { temperature: f32, tint: f32 },
}

pub const HSL_HUE_LIMIT: f32 = 200.0;
pub const HUE_ROTATION_LIMIT_DEGREES: f32 = 180.0;

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ExposureParams {
    pub black_point: f32,
    pub exposure: f32,
    #[serde(default)]
    pub contrast: f32,
    pub sigmoid: SigmoidParams,
    #[serde(flatten, with = "crate::migrations::white_balance_offsets")]
    pub white_balance: WhiteBalance,
    #[serde(default)]
    pub hue: f32,
    pub saturation: f32,
    pub vibrance: f32,
    pub tone_curve: PointCurve,
    pub tone_curve_red: PointCurve,
    pub tone_curve_green: PointCurve,
    pub tone_curve_blue: PointCurve,
    pub chroma_denoise: f32,
    #[serde(default)]
    pub luminance_denoise: f32,
    #[serde(default = "default_denoise_detail")]
    pub denoise_detail: f32,
    #[serde(default)]
    pub denoise_quality: DenoiseQuality,
    #[serde(default)]
    pub ai_denoise_enabled: bool,
    /// Share of the AI-denoised Bayer mosaic mixed with the original one, in
    /// `[0, 1]`. Ignored unless `ai_denoise_enabled`; X-Trans always uses 1.
    #[serde(default = "default_ai_denoise_amount")]
    pub ai_denoise_amount: f32,
    pub demosaic_mode: DemosaicMode,
    pub dual_threshold: f32,
    pub frequency_chroma: f32,
    pub highlight_method: HighlightReconstructionMethod,
    pub highlight_clip: f32,
    pub highlight_reconstruction: f32,
    /// Chromatic aberration removal in `[0, 100]`, owned by the Lens
    /// Corrections card: moves red and blue back onto green by the photo's
    /// measured lateral aberration ([`LoadedRaw::lateral_chromatic_aberration`])
    /// and removes the axial colour fringes that remain.
    ///
    /// [`LoadedRaw::lateral_chromatic_aberration`]: super::LoadedRaw::lateral_chromatic_aberration
    #[serde(default)]
    pub chromatic_aberration: f32,
    /// How the source is read: a scanned film negative is developed as its
    /// positive. Left out of saved positives, whose files stay as they were.
    #[serde(default, skip_serializing_if = "FilmSource::is_positive")]
    pub film: FilmSource,

    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub texture: f32,
    pub clarity: f32,
    pub dehaze: f32,

    #[serde(default = "default_sharpen_amount")]
    pub sharpen_amount: f32,
    #[serde(default = "default_sharpen_radius")]
    pub sharpen_radius: f32,
    #[serde(default = "default_sharpen_detail")]
    pub sharpen_detail: f32,
    #[serde(default)]
    pub sharpen_masking: f32,

    pub hsl_hue: [f32; 8],
    pub hsl_saturation: [f32; 8],
    pub hsl_luminance: [f32; 8],

    #[serde(default)]
    pub point_colors: super::PointColors,
    #[serde(skip)]
    pub point_color_visualize: Option<usize>,

    pub color_grading: ColorGrading,
}

pub fn sigmoid_contrast_from_percent(contrast: f32) -> f32 {
    let amount = if contrast.is_finite() {
        (contrast / 100.0).clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let default = SigmoidParams::default().contrast;
    if amount >= 0.0 {
        default * (DARKTABLE_SIGMOID_CONTRAST_SOFT_MAX / default).powf(amount)
    } else {
        default * (DARKTABLE_SIGMOID_CONTRAST_SOFT_MIN / default).powf(-amount)
    }
}

impl ExposureParams {
    pub fn sanitize_tone_curves(&mut self) {
        self.tone_curve.sanitize();
        self.tone_curve_red.sanitize();
        self.tone_curve_green.sanitize();
        self.tone_curve_blue.sanitize();
    }

    pub fn scene_referred_default() -> Self {
        Self::default()
    }

    /// The default edit of this edit's source, which canonical renderings
    /// such as mask sources develop. A film negative stays one: it is how the
    /// source is read, not an edit of it.
    pub fn source_reference(&self) -> Self {
        Self {
            film: self.film,
            ..Self::scene_referred_default()
        }
    }
}

impl Default for ExposureParams {
    fn default() -> Self {
        Self {
            black_point: 0.0,
            exposure: 0.0,
            contrast: 0.0,
            sigmoid: SigmoidParams::default(),
            white_balance: WhiteBalance::AsShot,
            hue: 0.0,
            saturation: 0.0,
            vibrance: 0.0,
            tone_curve: PointCurve::linear(),
            tone_curve_red: PointCurve::linear(),
            tone_curve_green: PointCurve::linear(),
            tone_curve_blue: PointCurve::linear(),
            chroma_denoise: 0.0,
            luminance_denoise: 0.0,
            denoise_detail: default_denoise_detail(),
            denoise_quality: DenoiseQuality::default(),
            ai_denoise_enabled: false,
            ai_denoise_amount: default_ai_denoise_amount(),
            demosaic_mode: DemosaicMode::Reference,
            dual_threshold: 20.0,
            frequency_chroma: 1.0,
            highlight_method: HighlightReconstructionMethod::InpaintOpposed,
            highlight_clip: 1.0,
            highlight_reconstruction: 1.0,
            chromatic_aberration: 0.0,
            film: FilmSource::Positive,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
            texture: 0.0,
            clarity: 0.0,
            dehaze: 0.0,
            sharpen_amount: default_sharpen_amount(),
            sharpen_radius: default_sharpen_radius(),
            sharpen_detail: default_sharpen_detail(),
            sharpen_masking: 0.0,
            hsl_hue: [0.0; 8],
            hsl_saturation: [0.0; 8],
            hsl_luminance: [0.0; 8],
            point_colors: super::PointColors::default(),
            point_color_visualize: None,
            color_grading: ColorGrading::default(),
        }
    }
}

pub(super) const fn default_denoise_detail() -> f32 {
    50.0
}

const fn default_ai_denoise_amount() -> f32 {
    1.0
}

const fn default_sharpen_amount() -> f32 {
    40.0
}

const fn default_sharpen_radius() -> f32 {
    1.0
}

const fn default_sharpen_detail() -> f32 {
    25.0
}
