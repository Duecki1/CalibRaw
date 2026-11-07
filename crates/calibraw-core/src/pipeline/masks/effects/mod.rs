//! The mask effects, registered once in [`mask_effects!`].
//!
//! Each effect is a row of that table and a settings type in its own module,
//! which implements [`EffectSettings`]: its starting point, when it is active,
//! its controls (sidecar validation and the generated card), its lanes and
//! how it packs them. Everything else follows from the row: the shader ID and
//! lane constants calibraw-gpu writes into the shaders, the creative pass's
//! dispatch, which passes and maps the GPU prepares, export-tile halos and
//! whether scene depth is generated.

mod blur;
mod edge_glow;
mod fog;
mod glow;
mod grain;
mod halation;
mod instances;
mod lanes;
mod lens_blur;
mod light_rays;
mod motion_blur;
mod neon;
pub mod params;
mod passes;
mod pixelate;
mod radial_blur;
mod rain;
mod relight;
mod settings;
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
pub use instances::{EffectInstances, MAX_EFFECT_INSTANCES};
pub use lanes::{ColorLane, EffectLanes, EffectOption, Lane, LaneName, PackContext, OPTION_COUNT};
pub use lens_blur::LensBlurEffectSettings;
pub use light_rays::{LightRaySource, LightRaysEffectSettings};
pub use motion_blur::MotionBlurEffectSettings;
pub use neon::NeonEffectSettings;
pub use passes::{EffectPass, EffectReach, ReachGroup, SceneNeed, SceneNeeds};
pub use pixelate::PixelateEffectSettings;
pub use radial_blur::{RadialBlurEffectSettings, RadialBlurMode};
pub use rain::RainEffectSettings;
pub use relight::{RelightEffectSettings, RelightLight};
pub use settings::{EffectControl, EffectControls, EffectDetails, EffectSettings};
pub use sharpen::SharpenEffectSettings;
pub use smoke::SmokeEffectSettings;
pub use tilt_shift::TiltShiftEffectSettings;
pub use vignette::VignetteEffectSettings;
pub use wet_surfaces::WetSurfacesEffectSettings;

use crate::pipeline::processing::{
    CAPTURE_SHARPEN_SUPPORT, EDGE_GLOW_SUPPORT, FOCUS_BLUR_SUPPORT, GLOW_SUPPORT,
    MASK_BLUR_SUPPORT, NEON_SUPPORT, PIXELATE_SUPPORT,
};

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// Something done with the settings of one effect, whichever it is
/// ([`MaskEffectSettings::visit_mut`]).
pub trait EffectSettingsVisitor {
    type Output;

    fn visit<S: EffectSettings>(self, effect: MaskEffect, settings: &mut S) -> Self::Output;
}

/// Registers the effects. Each row names an effect's serialized variant and:
///
/// - `label`, and `description` for the tooltip of its card's title;
/// - `shader_id`, its ID in the shaders (`MASK_EFFECT_<VARIANT>_ID`);
/// - `category`, its place in the picker;
/// - `settings`, the field of [`MaskEffectSettings`] and its type;
/// - `passes`, where it runs ([`EffectPass`]); a `Creative` effect defines
///   `apply_<field>` in its shader, which the creative pass calls;
/// - `reach`, how far around a pixel it reads ([`EffectReach`]);
/// - optionally `scene`, what it reads of the scene bake ([`SceneNeed`]s).
///
/// Rows are in the order the settings serialize, which sidecars and presets
/// keep.
macro_rules! mask_effects {
    ($(
        $variant:ident {
            label: $label:literal,
            description: $description:literal,
            shader_id: $id:literal,
            category: $category:ident,
            settings: $field:ident: $settings:ty,
            passes: [$($pass:ident),+],
            reach: $reach:ident $(($pixels:expr))?
            $(, scene: [$($need:ident),* $(,)?])? $(,)?
        }
    )+) => {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
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

            /// What the effect does, for the tooltip of its card's title.
            pub const fn description(self) -> Option<&'static str> {
                match self {
                    Self::Adjustment => None,
                    $(Self::$variant => Some($description),)+
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

            /// The name the shaders know the effect by: its settings field,
            /// as in `apply_<name>` and, in upper case,
            /// `MASK_EFFECT_<NAME>_ID` and `<NAME>_<LANE>_LANE`.
            pub const fn shader_name(self) -> &'static str {
                match self {
                    Self::Adjustment => "adjustment",
                    $(Self::$variant => stringify!($field),)+
                }
            }

            /// The effect with [`Self::shader_id`] `id`.
            pub const fn from_shader_id(id: u32) -> Option<Self> {
                match id {
                    0 => Some(Self::Adjustment),
                    $($id => Some(Self::$variant),)+
                    _ => None,
                }
            }

            /// The passes the effect runs in; none for an adjustment.
            pub const fn passes(self) -> &'static [EffectPass] {
                match self {
                    Self::Adjustment => &[],
                    $(Self::$variant => &[$(EffectPass::$pass),+],)+
                }
            }

            pub fn runs_in(self, pass: EffectPass) -> bool {
                self.passes().contains(&pass)
            }

            pub const fn reach(self) -> EffectReach {
                match self {
                    Self::Adjustment => EffectReach::Pixel,
                    $(Self::$variant => EffectReach::$reach $(($pixels))?,)+
                }
            }

            pub const fn scene_needs(self) -> SceneNeeds {
                match self {
                    Self::Adjustment => SceneNeeds::NONE,
                    $(Self::$variant => SceneNeeds::NONE $($(.with(SceneNeed::$need))*)?,)+
                }
            }

            /// The lanes the effect's settings pack; none for an adjustment.
            pub const fn lanes(self) -> &'static [LaneName] {
                match self {
                    Self::Adjustment => &[],
                    $(Self::$variant => <$settings as EffectSettings>::LANES,)+
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
                    $(MaskEffect::$variant => settings.$field = <$settings as EffectSettings>::initial(),)+
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

            /// Whether `effect`, when active, asks for scene depth.
            pub fn wants_scene_depth(&self, effect: MaskEffect) -> bool {
                match effect {
                    MaskEffect::Adjustment => false,
                    $(MaskEffect::$variant => self.$field.wants_scene_depth(),)+
                }
            }

            /// How many slots `effect` fills: one per instance
            /// ([`EffectSettings::instance_count`]); none for an adjustment.
            pub fn instance_count(&self, effect: MaskEffect) -> usize {
                match effect {
                    MaskEffect::Adjustment => 0,
                    $(MaskEffect::$variant => self.$field.instance_count(),)+
                }
            }

            /// What `effect`'s shader reads in the slot of instance
            /// `context.instance`; `None` for an adjustment.
            pub fn pack(&self, effect: MaskEffect, context: &PackContext) -> Option<EffectLanes> {
                let mut slot = EffectLanes::default();
                match effect {
                    MaskEffect::Adjustment => return None,
                    $(MaskEffect::$variant => self.$field.pack(&mut slot, context),)+
                }
                Some(slot)
            }

            /// Runs `visitor` on the settings of `effect`; `None` for an
            /// adjustment.
            pub fn visit_mut<V: EffectSettingsVisitor>(
                &mut self,
                effect: MaskEffect,
                visitor: V,
            ) -> Option<V::Output> {
                match effect {
                    MaskEffect::Adjustment => None,
                    $(MaskEffect::$variant => Some(visitor.visit(effect, &mut self.$field)),)+
                }
            }

            /// Every effect's controls, in serialized order.
            pub fn all_controls(&mut self) -> impl Iterator<Item = (MaskEffect, EffectControls<'_>)> {
                [$((MaskEffect::$variant, self.$field.controls()),)+].into_iter()
            }
        }

        /// Each effect's lanes, by its settings field.
        pub mod effect_lanes {
            pub use super::lanes::options;
            $(pub use super::$field::lanes as $field;)+
        }
    };
}

mask_effects! {
    Blur {
        label: "Blur",
        description: "Soften distracting detail. The mask controls where the blurred image is blended in.",
        shader_id: 4,
        category: BlurAndFocus,
        settings: blur: BlurEffectSettings,
        passes: [Blur],
        reach: MaskBlur(MASK_BLUR_SUPPORT),
    }
    LensBlur {
        label: "Lens Blur",
        description: "Defocus backgrounds with aperture-shaped bokeh. The mask controls the blend; use a soft edge for a gradual transition.",
        shader_id: 7,
        category: BlurAndFocus,
        settings: lens_blur: LensBlurEffectSettings,
        passes: [Blur],
        reach: FocusBlur(FOCUS_BLUR_SUPPORT),
    }
    MotionBlur {
        label: "Motion Blur",
        description: "Add directional movement. Distance sets the trail length; the mask controls where the blur appears.",
        shader_id: 8,
        category: BlurAndFocus,
        settings: motion_blur: MotionBlurEffectSettings,
        passes: [Blur],
        reach: FocusBlur(FOCUS_BLUR_SUPPORT),
    }
    RadialBlur {
        label: "Radial Blur",
        description: "Zoom toward a center or spin around it. Position is relative to the full image; the mask controls the blend.",
        shader_id: 9,
        category: BlurAndFocus,
        settings: radial_blur: RadialBlurEffectSettings,
        passes: [Blur],
        reach: FocusBlur(FOCUS_BLUR_SUPPORT),
    }
    TiltShift {
        label: "Tilt-Shift",
        description: "Keep a band in focus and soften its surroundings. Position is relative to the full image. Use a Fullscreen mask for an uninterrupted focus transition.",
        shader_id: 10,
        category: BlurAndFocus,
        settings: tilt_shift: TiltShiftEffectSettings,
        passes: [Blur],
        reach: FocusBlur(FOCUS_BLUR_SUPPORT),
    }
    Sharpen {
        label: "Sharpen",
        description: "Sharpen fine detail inside the mask with its own radius, detail and masking. It adds to the Detail card's sharpening.",
        shader_id: 17,
        category: BlurAndFocus,
        settings: sharpen: SharpenEffectSettings,
        passes: [CaptureSharpening],
        reach: CaptureSharpening(CAPTURE_SHARPEN_SUPPORT),
    }
    EdgeGlow {
        label: "Edge Glow",
        description: "Light up image contours with a soft halo. Both the outlines and their glow stay inside the mask.",
        shader_id: 5,
        category: Stylize,
        settings: edge_glow: EdgeGlowEffectSettings,
        passes: [Creative],
        reach: Creative(EDGE_GLOW_SUPPORT),
    }
    Glow {
        label: "Glow",
        description: "Spread colored light from bright pixels. The mask selects the highlights that emit light; their glow can extend beyond the selection. A self-illuminating Glow also lights the scene around it; Scene light sets how much.",
        shader_id: 2,
        category: GlowAndLight,
        settings: glow: GlowEffectSettings,
        passes: [CreativeLight],
        reach: Glow(GLOW_SUPPORT),
    }
    LightRays {
        label: "Light Rays",
        description: "Place the source relative to the full image, then paint its openings. The light shafts shine through the mask and show only inside it.",
        shader_id: 3,
        category: GlowAndLight,
        settings: light_rays: LightRaysEffectSettings,
        passes: [CreativeLight],
        reach: Pixel,
    }
    Relight {
        label: "Relight",
        description: "Add a light that shapes the scene in depth. Position it like Light Rays; scene depth is generated automatically. Turn on Shadows to let nearer objects block the light. The sky and anything the depth map places at its far limit stay unlit. The mask controls where the light falls.",
        shader_id: 16,
        category: GlowAndLight,
        settings: relight: RelightEffectSettings,
        passes: [Creative],
        reach: Pixel,
        scene: [LightBake],
    }
    Neon {
        label: "Neon",
        description: "Trace image edges with colored light inside the mask. Original image keeps the photo visible behind the lines; the glow also stays inside the mask. Scene light lets the lines light their surroundings.",
        shader_id: 1,
        category: GlowAndLight,
        settings: neon: NeonEffectSettings,
        passes: [SceneEffects],
        reach: SceneEffects(NEON_SUPPORT),
    }
    Pixelate {
        label: "Pixelate",
        description: "Create a graphic mosaic. Block size sets the tile scale; the mask controls where the result appears.",
        shader_id: 6,
        category: Stylize,
        settings: pixelate: PixelateEffectSettings,
        passes: [Creative],
        reach: Creative(PIXELATE_SUPPORT),
    }
    Fog {
        label: "Fog",
        description: "Add atmospheric haze through the mask. Scene depth builds fog with distance; turn it off for an even-distance veil.",
        shader_id: 11,
        category: Texture,
        settings: fog: FogEffectSettings,
        passes: [Creative],
        reach: Pixel,
        scene: [AirLight],
    }
    Smoke {
        label: "Smoke",
        description: "Layer drifting plumes through the mask. The pattern stays anchored to the full image as you edit the selection.",
        shader_id: 12,
        category: Texture,
        settings: smoke: SmokeEffectSettings,
        passes: [Creative],
        reach: Pixel,
        scene: [LightBake],
    }
    Grain {
        label: "Grain",
        description: "Add photographic texture. Grain stays anchored to the image; the mask controls its local strength.",
        shader_id: 13,
        category: FilmAndFinish,
        settings: grain: GrainEffectSettings,
        passes: [DisplayFinish],
        reach: Pixel,
    }
    Halation {
        label: "Halation",
        description: "Give bright highlights a warm film halo inside the mask. Nearby highlights outside the selection can cast a halo into it. Use Fullscreen for a whole-image finish.",
        shader_id: 14,
        category: FilmAndFinish,
        settings: halation: HalationEffectSettings,
        passes: [Creative],
        reach: Glow(GLOW_SUPPORT),
    }
    Vignette {
        label: "Vignette",
        description: "Darken edges with negative amounts or brighten them with positive amounts. The center follows the cropped, rotated frame; the mask controls the blend.",
        shader_id: 15,
        category: FilmAndFinish,
        settings: vignette: VignetteEffectSettings,
        passes: [DisplayFinish],
        reach: Pixel,
    }
    Rain {
        label: "Rain",
        description: "Add falling rain and its mist. Drops pass behind nearer objects, catch Relight, Light Rays, Glow, Neon and the photo's own lights, and show best against dark areas. The mask controls where it falls.",
        shader_id: 18,
        category: Texture,
        settings: rain: RainEffectSettings,
        passes: [AfterCreative],
        reach: Pixel,
        scene: [AirLight, Image],
    }
    WetSurfaces {
        label: "Wet Surfaces",
        description: "Darken surfaces as water does and let the ground reflect the photo, with standing puddles on smooth ground. The ground is found in scene depth, which is generated automatically; a Rain effect ripples the puddles. The mask controls where it applies.",
        shader_id: 19,
        category: Texture,
        settings: wet_surfaces: WetSurfacesEffectSettings,
        passes: [BeforeCreative, AfterCreative],
        reach: Pixel,
        scene: [Geometry, Image],
    }
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

    /// Switched off: never offered and never rendered (2026-11-07; Rain and Wet
    /// Surfaces shipped in no release and nobody uses them). Components that hold
    /// one keep their settings and render as if disabled. Their GPU passes are
    /// never compiled either: Rain's crashes the Adreno 830 shader compiler.
    pub const fn is_disabled(self) -> bool {
        matches!(self, Self::Rain | Self::WetSurfaces)
    }

    /// Available, but its results are not yet reliable enough to present as finished.
    pub const fn is_experimental(self) -> bool {
        matches!(self, Self::Relight)
    }

    pub const fn uses_adjustments(self) -> bool {
        matches!(self, Self::Adjustment)
    }

    /// Whether the effect reads its mask beyond an export tile or zoomed
    /// view, from the full-image mask atlas: Light Rays shapes rays from
    /// openings anywhere in the frame, and Glow and Neon light the scene from
    /// wherever they emit (scene_emission.wgsl). Their masks
    /// are kept there whatever their settings, so changing one needs no new
    /// mask upload.
    pub const fn reads_full_image_mask(self) -> bool {
        matches!(self, Self::LightRays | Self::Glow | Self::Neon)
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
