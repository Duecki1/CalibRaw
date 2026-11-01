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
mod rain;
mod relight;
mod sharpen;
mod smoke;
mod tilt_shift;
mod vignette;
mod wet_surfaces;

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
pub use rain::RainEffectSettings;
pub use relight::RelightEffectSettings;
pub use sharpen::SharpenEffectSettings;
pub use smoke::SmokeEffectSettings;
pub use tilt_shift::TiltShiftEffectSettings;
pub use vignette::VignetteEffectSettings;
pub use wet_surfaces::WetSurfacesEffectSettings;

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// Registers the effects: each row names an effect's serialized variant, label,
/// id in the shaders (`MASK_EFFECT_*_ID` in mask_effects/shared.wgsl), place in the
/// picker and its settings' field. Rows are in the order the settings serialize,
/// which sidecars and presets keep.
macro_rules! mask_effects {
    ($(
        $variant:ident {
            label: $label:literal,
            shader_id: $id:literal,
            category: $category:ident,
            settings: $field:ident: $settings:ty $(,)?
        }
    )+) => {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
        pub enum MaskEffect {
            #[default]
            Adjustment,
            $($variant,)+
        }

        impl MaskEffect {
            const COUNT: usize = 1 + [$(Self::$variant),+].len();

            pub const fn label(self) -> &'static str {
                match self {
                    Self::Adjustment => "Adjustment",
                    $(Self::$variant => $label,)+
                }
            }

            pub const fn category(self) -> Option<MaskEffectCategory> {
                match self {
                    Self::Adjustment => None,
                    $(Self::$variant => Some(MaskEffectCategory::$category),)+
                }
            }

            pub const fn shader_id(self) -> u32 {
                match self {
                    Self::Adjustment => 0,
                    $(Self::$variant => $id,)+
                }
            }
        }

        #[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
        pub struct MaskEffectSettings {
            $(
                #[serde(default, skip_serializing_if = "is_default")]
                pub $field: $settings,
            )+
        }

        impl MaskEffectSettings {
            /// The settings a newly added `effect` starts with.
            pub fn initial(effect: MaskEffect) -> Self {
                let mut settings = Self::default();
                match effect {
                    MaskEffect::Adjustment => {}
                    $(MaskEffect::$variant => settings.$field = InitialEffectSettings::initial(),)+
                }
                settings
            }

            /// Whether these settings make `effect` change the image.
            pub fn effect_is_active(&self, effect: MaskEffect) -> bool {
                match effect {
                    MaskEffect::Adjustment => false,
                    $(MaskEffect::$variant => self.$field.is_active(),)+
                }
            }
        }
    };
}

mask_effects! {
    Blur { label: "Blur", shader_id: 4, category: BlurAndFocus, settings: blur: BlurEffectSettings }
    LensBlur { label: "Lens Blur", shader_id: 7, category: BlurAndFocus, settings: lens_blur: LensBlurEffectSettings }
    MotionBlur { label: "Motion Blur", shader_id: 8, category: BlurAndFocus, settings: motion_blur: MotionBlurEffectSettings }
    RadialBlur { label: "Radial Blur", shader_id: 9, category: BlurAndFocus, settings: radial_blur: RadialBlurEffectSettings }
    TiltShift { label: "Tilt-Shift", shader_id: 10, category: BlurAndFocus, settings: tilt_shift: TiltShiftEffectSettings }
    Sharpen { label: "Sharpen", shader_id: 17, category: BlurAndFocus, settings: sharpen: SharpenEffectSettings }
    EdgeGlow { label: "Edge Glow", shader_id: 5, category: Stylize, settings: edge_glow: EdgeGlowEffectSettings }
    Glow { label: "Glow", shader_id: 2, category: GlowAndLight, settings: glow: GlowEffectSettings }
    LightRays { label: "Light Rays", shader_id: 3, category: GlowAndLight, settings: light_rays: LightRaysEffectSettings }
    Relight { label: "Relight", shader_id: 16, category: GlowAndLight, settings: relight: RelightEffectSettings }
    Neon { label: "Neon", shader_id: 1, category: GlowAndLight, settings: neon: NeonEffectSettings }
    Pixelate { label: "Pixelate", shader_id: 6, category: Stylize, settings: pixelate: PixelateEffectSettings }
    Fog { label: "Fog", shader_id: 11, category: Texture, settings: fog: FogEffectSettings }
    Smoke { label: "Smoke", shader_id: 12, category: Texture, settings: smoke: SmokeEffectSettings }
    Grain { label: "Grain", shader_id: 13, category: FilmAndFinish, settings: grain: GrainEffectSettings }
    Halation { label: "Halation", shader_id: 14, category: FilmAndFinish, settings: halation: HalationEffectSettings }
    Vignette { label: "Vignette", shader_id: 15, category: FilmAndFinish, settings: vignette: VignetteEffectSettings }
    Rain { label: "Rain", shader_id: 18, category: Texture, settings: rain: RainEffectSettings }
    WetSurfaces { label: "Wet Surfaces", shader_id: 19, category: Texture, settings: wet_surfaces: WetSurfacesEffectSettings }
}

impl MaskEffect {
    /// Every effect, in the order the picker lists them.
    pub const ALL: [Self; Self::COUNT] = [
        Self::Adjustment,
        Self::Blur,
        Self::LensBlur,
        Self::MotionBlur,
        Self::RadialBlur,
        Self::Sharpen,
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
        Self::Rain,
        Self::WetSurfaces,
    ];

    /// Available, but its results are not yet reliable enough to present as finished.
    pub const fn is_experimental(self) -> bool {
        matches!(self, Self::Relight)
    }

    pub const fn uses_adjustments(self) -> bool {
        matches!(self, Self::Adjustment)
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

impl MaskEffectSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
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
impl InitialEffectSettings for SharpenEffectSettings {}
impl InitialEffectSettings for LightRaysEffectSettings {}
impl InitialEffectSettings for NeonEffectSettings {}
impl InitialEffectSettings for RelightEffectSettings {}
impl InitialEffectSettings for EdgeGlowEffectSettings {}
impl InitialEffectSettings for PixelateEffectSettings {}
impl InitialEffectSettings for GrainEffectSettings {}
impl InitialEffectSettings for HalationEffectSettings {}
impl InitialEffectSettings for VignetteEffectSettings {}
impl InitialEffectSettings for RainEffectSettings {}
impl InitialEffectSettings for WetSurfacesEffectSettings {}
