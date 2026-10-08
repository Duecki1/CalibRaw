mod blur;
mod edge_glow;
mod fog;
mod glow;
mod grain;
mod halation;
mod lens_blur;
mod light_rays;
mod motion_blur;
mod neon;
pub mod params;
mod pixelate;
mod radial_blur;
mod relight;
mod smoke;
mod tilt_shift;
mod vignette;

#[cfg(test)]
mod tests;

pub use blur::BlurEffectSettings;
pub use edge_glow::EdgeGlowEffectSettings;
pub use fog::FogEffectSettings;
pub use glow::GlowEffectSettings;
pub use grain::GrainEffectSettings;
pub use halation::HalationEffectSettings;
pub use lens_blur::LensBlurEffectSettings;
pub use light_rays::LightRaysEffectSettings;
pub use motion_blur::MotionBlurEffectSettings;
pub use neon::NeonEffectSettings;
pub use pixelate::PixelateEffectSettings;
pub use radial_blur::{RadialBlurEffectSettings, RadialBlurMode};
pub use relight::RelightEffectSettings;
pub use smoke::SmokeEffectSettings;
pub use tilt_shift::TiltShiftEffectSettings;
pub use vignette::VignetteEffectSettings;

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum MaskEffect {
    #[default]
    Adjustment,
    Blur,
    LensBlur,
    MotionBlur,
    RadialBlur,
    TiltShift,
    Glow,
    LightRays,
    Neon,
    Relight,
    EdgeGlow,
    Pixelate,
    Fog,
    Smoke,
    Grain,
    Halation,
    Vignette,
}

impl MaskEffect {
    pub const ALL: [Self; 17] = [
        Self::Adjustment,
        Self::Blur,
        Self::LensBlur,
        Self::MotionBlur,
        Self::RadialBlur,
        Self::TiltShift,
        Self::Glow,
        Self::LightRays,
        Self::Neon,
        Self::Relight,
        Self::EdgeGlow,
        Self::Pixelate,
        Self::Fog,
        Self::Smoke,
        Self::Grain,
        Self::Halation,
        Self::Vignette,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Adjustment => "Adjustment",
            Self::Blur => "Blur",
            Self::LensBlur => "Lens Blur",
            Self::MotionBlur => "Motion Blur",
            Self::RadialBlur => "Radial Blur",
            Self::TiltShift => "Tilt-Shift",
            Self::Glow => "Glow",
            Self::LightRays => "Light Rays",
            Self::Relight => "Relight",
            Self::Neon => "Neon",
            Self::EdgeGlow => "Edge Glow",
            Self::Pixelate => "Pixelate",
            Self::Fog => "Fog",
            Self::Smoke => "Smoke",
            Self::Grain => "Grain",
            Self::Halation => "Halation",
            Self::Vignette => "Vignette",
        }
    }

    pub const fn category(self) -> Option<MaskEffectCategory> {
        match self {
            Self::Adjustment => None,
            Self::Blur | Self::LensBlur | Self::MotionBlur | Self::RadialBlur | Self::TiltShift => {
                Some(MaskEffectCategory::BlurAndFocus)
            }
            Self::Glow | Self::LightRays | Self::Neon | Self::Relight => {
                Some(MaskEffectCategory::GlowAndLight)
            }
            Self::EdgeGlow | Self::Pixelate => Some(MaskEffectCategory::Stylize),
            Self::Fog | Self::Smoke => Some(MaskEffectCategory::Texture),
            Self::Grain | Self::Halation | Self::Vignette => {
                Some(MaskEffectCategory::FilmAndFinish)
            }
        }
    }

    pub const fn uses_adjustments(self) -> bool {
        matches!(self, Self::Adjustment)
    }

    pub const fn shader_id(self) -> u32 {
        match self {
            Self::Adjustment => 0,
            Self::Neon => 1,
            Self::Glow => 2,
            Self::LightRays => 3,
            Self::Blur => 4,
            Self::EdgeGlow => 5,
            Self::Pixelate => 6,
            Self::LensBlur => 7,
            Self::MotionBlur => 8,
            Self::RadialBlur => 9,
            Self::TiltShift => 10,
            Self::Fog => 11,
            Self::Smoke => 12,
            Self::Grain => 13,
            Self::Halation => 14,
            Self::Vignette => 15,
            Self::Relight => 16,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaskEffectCategory {
    BlurAndFocus,
    GlowAndLight,
    Stylize,
    Texture,
    FilmAndFinish,
}

impl MaskEffectCategory {
    pub const ALL: [Self; 5] = [
        Self::BlurAndFocus,
        Self::GlowAndLight,
        Self::Stylize,
        Self::Texture,
        Self::FilmAndFinish,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::BlurAndFocus => "Blur & Focus",
            Self::GlowAndLight => "Glow & Light",
            Self::Stylize => "Stylize",
            Self::Texture => "Atmosphere",
            Self::FilmAndFinish => "Film & Finish",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MaskEffectSettings {
    #[serde(default, skip_serializing_if = "is_default")]
    pub blur: BlurEffectSettings,
    #[serde(default, skip_serializing_if = "is_default")]
    pub lens_blur: LensBlurEffectSettings,
    #[serde(default, skip_serializing_if = "is_default")]
    pub motion_blur: MotionBlurEffectSettings,
    #[serde(default, skip_serializing_if = "is_default")]
    pub radial_blur: RadialBlurEffectSettings,
    #[serde(default, skip_serializing_if = "is_default")]
    pub tilt_shift: TiltShiftEffectSettings,
    #[serde(default, skip_serializing_if = "is_default")]
    pub edge_glow: EdgeGlowEffectSettings,
    #[serde(default, skip_serializing_if = "is_default")]
    pub glow: GlowEffectSettings,
    #[serde(default, skip_serializing_if = "is_default")]
    pub light_rays: LightRaysEffectSettings,
    #[serde(default, skip_serializing_if = "is_default")]
    pub relight: RelightEffectSettings,
    #[serde(default, skip_serializing_if = "is_default")]
    pub neon: NeonEffectSettings,
    #[serde(default, skip_serializing_if = "is_default")]
    pub pixelate: PixelateEffectSettings,
    #[serde(default, skip_serializing_if = "is_default")]
    pub fog: FogEffectSettings,
    #[serde(default, skip_serializing_if = "is_default")]
    pub smoke: SmokeEffectSettings,
    #[serde(default, skip_serializing_if = "is_default")]
    pub grain: GrainEffectSettings,
    #[serde(default, skip_serializing_if = "is_default")]
    pub halation: HalationEffectSettings,
    #[serde(default, skip_serializing_if = "is_default")]
    pub vignette: VignetteEffectSettings,
}

impl MaskEffectSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The settings a newly added `effect` starts with.
    pub fn initial(effect: MaskEffect) -> Self {
        let mut settings = Self::default();
        match effect {
            MaskEffect::Adjustment => {}
            MaskEffect::Blur => settings.blur = InitialEffectSettings::initial(),
            MaskEffect::LensBlur => settings.lens_blur = InitialEffectSettings::initial(),
            MaskEffect::MotionBlur => settings.motion_blur = InitialEffectSettings::initial(),
            MaskEffect::RadialBlur => settings.radial_blur = InitialEffectSettings::initial(),
            MaskEffect::TiltShift => settings.tilt_shift = InitialEffectSettings::initial(),
            MaskEffect::Glow => settings.glow = InitialEffectSettings::initial(),
            MaskEffect::LightRays => settings.light_rays = InitialEffectSettings::initial(),
            MaskEffect::Neon => settings.neon = InitialEffectSettings::initial(),
            MaskEffect::Relight => settings.relight = InitialEffectSettings::initial(),
            MaskEffect::EdgeGlow => settings.edge_glow = InitialEffectSettings::initial(),
            MaskEffect::Pixelate => settings.pixelate = InitialEffectSettings::initial(),
            MaskEffect::Fog => settings.fog = InitialEffectSettings::initial(),
            MaskEffect::Smoke => settings.smoke = InitialEffectSettings::initial(),
            MaskEffect::Grain => settings.grain = InitialEffectSettings::initial(),
            MaskEffect::Halation => settings.halation = InitialEffectSettings::initial(),
            MaskEffect::Vignette => settings.vignette = InitialEffectSettings::initial(),
        }
        settings
    }
}

/// The settings an effect starts with when it is added or its card is reset.
///
/// `Default` is the serialized baseline: omitted fields and omitted settings
/// decode to it, so changing it would change saved edits. An effect whose
/// starting point differs from that baseline overrides [`Self::initial`].
pub trait InitialEffectSettings: Default {
    fn initial() -> Self {
        Self::default()
    }
}

impl InitialEffectSettings for BlurEffectSettings {}
impl InitialEffectSettings for LensBlurEffectSettings {}
impl InitialEffectSettings for MotionBlurEffectSettings {}
impl InitialEffectSettings for RadialBlurEffectSettings {}
impl InitialEffectSettings for TiltShiftEffectSettings {}
impl InitialEffectSettings for LightRaysEffectSettings {}
impl InitialEffectSettings for NeonEffectSettings {}
impl InitialEffectSettings for RelightEffectSettings {}
impl InitialEffectSettings for EdgeGlowEffectSettings {}
impl InitialEffectSettings for PixelateEffectSettings {}
impl InitialEffectSettings for FogEffectSettings {}
impl InitialEffectSettings for SmokeEffectSettings {}
impl InitialEffectSettings for GrainEffectSettings {}
impl InitialEffectSettings for HalationEffectSettings {}
impl InitialEffectSettings for VignetteEffectSettings {}
