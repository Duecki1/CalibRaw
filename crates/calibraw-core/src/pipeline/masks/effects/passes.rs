//! Where in the processing chain an effect runs, how far around a pixel it
//! reads and which scene inputs it needs.

/// A stage of the processing chain that applies effects, in processing order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectPass {
    /// Capture sharpening, before the scene effects (detail_capture.wgsl).
    CaptureSharpening,
    /// The scene effects pass, with Texture and Clarity
    /// (`apply_scene_effects_node` in creative_effects.wgsl).
    SceneEffects,
    /// The blur passes (blur.wgsl and its family).
    Blur,
    /// Its own pass before the creative pass (`apply_wet_surfaces`).
    BeforeCreative,
    /// The creative pass, slot by slot in card order: the shader calls the
    /// effect's `apply_<effect>(pos, rgb, params)` (effect_api.wgsl).
    Creative,
    /// The creative pass, after every slot, from passes of the effect's own
    /// (Glow's diffusion, Light Rays' masks).
    CreativeLight,
    /// Its own pass after the creative pass (`apply_weather_air`).
    AfterCreative,
    /// After the display transform (film_finish.wgsl).
    DisplayFinish,
}

impl EffectPass {
    /// Whether the pass runs among the scene-referred passes after the scene
    /// tone pass, which then must run (`needs_intermediate_adjustment_passes`).
    pub const fn is_intermediate(self) -> bool {
        !matches!(self, Self::CaptureSharpening | Self::DisplayFinish)
    }
}

/// How far around a pixel an effect reads, which export tiles must cover
/// with their halo (`required_export_tile_halo`).
///
/// Effects of one group read the same input, so the group reserves its
/// widest reach once; the groups chain, so their reaches add up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectReach {
    /// Only the pixel itself and full-image maps.
    Pixel,
    CaptureSharpening(u32),
    SceneEffects(u32),
    MaskBlur(u32),
    FocusBlur(u32),
    /// The creative pass's per-slot effects.
    Creative(u32),
    /// Glow's diffusion and the reads of highlight Glow and Halation, which
    /// all sample the creative pass's input.
    Glow(u32),
}

/// A group of [`EffectReach`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReachGroup {
    CaptureSharpening,
    SceneEffects,
    MaskBlur,
    FocusBlur,
    Creative,
    Glow,
}

impl ReachGroup {
    pub const ALL: [Self; 6] = [
        Self::CaptureSharpening,
        Self::SceneEffects,
        Self::MaskBlur,
        Self::FocusBlur,
        Self::Creative,
        Self::Glow,
    ];
}

impl EffectReach {
    /// Its group and pixels, or `None` for [`Self::Pixel`].
    pub const fn group(self) -> Option<(ReachGroup, u32)> {
        match self {
            Self::Pixel => None,
            Self::CaptureSharpening(pixels) => Some((ReachGroup::CaptureSharpening, pixels)),
            Self::SceneEffects(pixels) => Some((ReachGroup::SceneEffects, pixels)),
            Self::MaskBlur(pixels) => Some((ReachGroup::MaskBlur, pixels)),
            Self::FocusBlur(pixels) => Some((ReachGroup::FocusBlur, pixels)),
            Self::Creative(pixels) => Some((ReachGroup::Creative, pixels)),
            Self::Glow(pixels) => Some((ReachGroup::Glow, pixels)),
        }
    }
}

/// What an effect reads of the scene bake (calibraw-gpu `scene`): the scene's
/// geometry with two surfaces per pixel, its working image and its lights.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneNeeds {
    /// Depth, coverage, and each surface's normal and colour; read only while
    /// scene depth is present.
    pub geometry: bool,
    /// The light bake's shadow cubes and each surface's visibility of the
    /// lights that cast shadows (calibraw-gpu `scene::lights`); with the
    /// geometry.
    pub light_bake: bool,
    /// The shadowed share of each light along every view ray (the air
    /// light); with the light bake.
    pub air_light: bool,
    /// The working image with its mip chain, to read the scene's colour
    /// with a footprint (`scene_image`). Also without a depth result.
    pub image: bool,
}

/// One part of [`SceneNeeds`], as the effect registry names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneNeed {
    Geometry,
    LightBake,
    AirLight,
    Image,
}

impl SceneNeeds {
    pub const NONE: Self = Self {
        geometry: false,
        light_bake: false,
        air_light: false,
        image: false,
    };

    /// These needs and `need`, with what it is built from.
    pub const fn with(self, need: SceneNeed) -> Self {
        match need {
            SceneNeed::Geometry => Self {
                geometry: true,
                ..self
            },
            SceneNeed::LightBake => Self {
                geometry: true,
                light_bake: true,
                ..self
            },
            SceneNeed::AirLight => Self {
                geometry: true,
                light_bake: true,
                air_light: true,
                ..self
            },
            SceneNeed::Image => Self {
                image: true,
                ..self
            },
        }
    }
}
