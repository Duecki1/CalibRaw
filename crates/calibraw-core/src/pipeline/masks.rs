use half::f16;
use rayon::prelude::*;
use std::collections::BTreeSet;
use std::f32::consts::TAU;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use super::{LensGeometryMap, SceneDepth};
use crate::color_math::{linear_srgb_to_oklab, srgb_decode_signed};

mod content_dependencies;
mod effects;
mod raster_cache;

pub use content_dependencies::ContentDependencies;

pub use effects::{
    effect_lanes, params as effect_params, BlurEffectSettings, ColorLane, EdgeGlowEffectSettings,
    EffectControl, EffectControls, EffectDetails, EffectInstances, EffectLanes, EffectOption,
    EffectPass, EffectReach, EffectSettings, EffectSettingsVisitor, FogEffectSettings,
    GlowEffectSettings, GrainEffectSettings, HalationEffectSettings, Lane, LaneName,
    LensBlurEffectSettings, LightRaySource, LightRaysEffectSettings, MaskEffect,
    MaskEffectCategory, MaskEffectSettings, MotionBlurEffectSettings, NeonEffectSettings,
    PackContext, PixelateEffectSettings, RadialBlurEffectSettings, RadialBlurMode,
    RainEffectSettings, ReachGroup, RelightEffectSettings, RelightLight, SceneNeed, SceneNeeds,
    SharpenEffectSettings, SmokeEffectSettings, TiltShiftEffectSettings, VignetteEffectSettings,
    WetSurfacesEffectSettings, MAX_EFFECT_INSTANCES, OPTION_COUNT,
};

mod box_filter;
mod brush;
mod edge_room;
mod editing;
mod layers;
mod morphology;
mod probability;
mod range_filters;
mod raster;
mod shapes;
pub use brush::*;
use probability::*;
use range_filters::*;
use raster::*;
pub use raster::{
    lightness_to_luminance, luminance_range_weight, luminance_to_lightness,
    DEFAULT_COLOR_TOLERANCE, LUMINANCE_FEATHER_MAX, LUMINANCE_FEATHER_WIDTH,
};
pub use shapes::*;

mod adjustments;
mod mask_geometry;
mod primitives;
pub use adjustments::*;
pub use mask_geometry::*;
pub use primitives::*;

pub const MAX_LOCAL_MASKS: usize = 32;
/// Places for effects on a mask or the Effects card. Each effect component
/// takes one per instance ([`EffectComponent::places`]), so the render slots
/// they fill stay within a fixed budget.
pub const MAX_EFFECT_COMPONENTS: usize = 12;
pub const MAX_MASK_COMPONENTS: usize = 64;
pub const MAX_PATH_POINTS: usize = 256;
/// People a people mask selects at most: as many as are found in a photo.
pub const MAX_MASK_PEOPLE: usize = 32;
pub const MASK_ATLAS_EDGE_DESKTOP: u32 = 2048;
pub const MASK_ATLAS_EDGE_ANDROID: u32 = 1024;
pub const MASK_ATLAS_EDGE_EXPORT_DESKTOP: u32 = 4096;
pub const MASK_ATLAS_EDGE_EXPORT_ANDROID: u32 = 2048;

pub const fn mask_atlas_edge() -> u32 {
    if cfg!(target_os = "android") {
        MASK_ATLAS_EDGE_ANDROID
    } else {
        MASK_ATLAS_EDGE_DESKTOP
    }
}

pub const fn export_mask_atlas_edge_limit() -> u32 {
    if cfg!(target_os = "android") {
        MASK_ATLAS_EDGE_EXPORT_ANDROID
    } else {
        MASK_ATLAS_EDGE_EXPORT_DESKTOP
    }
}

pub fn export_mask_atlas_edge(image_width: u32, image_height: u32) -> u32 {
    image_width
        .max(image_height)
        .min(export_mask_atlas_edge_limit())
        .max(mask_atlas_edge())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum MaskKind {
    #[default]
    Brush,
    Fullscreen,
    Radial,
    Linear,
    Path,
    Subject,
    Background,
    Sky,
    Object,
    LuminanceRange,
    ColorRange,
    DepthRange,
    Person,
}

impl MaskKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Brush => "Brush",
            Self::Fullscreen => "Fullscreen",
            Self::Radial => "Radial Gradient",
            Self::Linear => "Linear Gradient",
            Self::Path => "Freeform / Path",
            Self::Subject => "Select Subject",
            Self::Background => "Select Background",
            Self::Sky => "Select Sky",
            Self::Object => "Select Object",
            Self::LuminanceRange => "Luminance Range",
            Self::ColorRange => "Color Range",
            Self::DepthRange => "Depth",
            Self::Person => "Select People",
        }
    }

    /// Available, but its results are not yet reliable enough to present as finished.
    pub const fn is_experimental(self) -> bool {
        matches!(self, Self::DepthRange)
    }

    pub const fn is_available(self) -> bool {
        matches!(
            self,
            Self::Brush
                | Self::Fullscreen
                | Self::Radial
                | Self::Linear
                | Self::Path
                | Self::Subject
                | Self::Background
                | Self::Sky
                | Self::Object
                | Self::LuminanceRange
                | Self::ColorRange
                | Self::DepthRange
                | Self::Person
        )
    }
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MaskCommon {
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub invert: bool,
}

impl MaskCommon {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            enabled: true,
            invert: false,
        }
    }

    pub fn rename(&mut self, name: impl Into<String>) -> bool {
        set_if_changed(&mut self.name, name.into())
    }

    pub fn set_enabled(&mut self, enabled: bool) -> bool {
        set_if_changed(&mut self.enabled, enabled)
    }

    pub fn toggle_invert(&mut self) {
        self.invert = !self.invert;
    }
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MaskComponent {
    #[serde(flatten)]
    pub common: MaskCommon,
    pub kind: MaskKind,
    pub combine: MaskCombineMode,
    /// How strongly the component's combine step applies, 0 to 1: the mask
    /// moves this far from its state before the component toward the
    /// combined result. Full opacity is left out of saved files, so those
    /// written before components had opacity read and write unchanged.
    #[serde(
        default = "full_component_opacity",
        skip_serializing_if = "is_full_component_opacity"
    )]
    pub opacity: f32,
    pub geometry: MaskGeometry,
}

const fn full_component_opacity() -> f32 {
    1.0
}

fn is_full_component_opacity(opacity: &f32) -> bool {
    *opacity == 1.0
}

impl Deref for MaskComponent {
    type Target = MaskCommon;

    fn deref(&self) -> &Self::Target {
        &self.common
    }
}

impl DerefMut for MaskComponent {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.common
    }
}

impl MaskComponent {
    pub fn new(kind: MaskKind, combine: MaskCombineMode) -> Self {
        Self {
            common: MaskCommon::new(kind.label()),
            kind,
            combine,
            opacity: full_component_opacity(),
            geometry: MaskGeometry::for_kind(kind),
        }
    }

    pub fn set_opacity(&mut self, opacity: f32) -> bool {
        set_if_changed(&mut self.opacity, opacity.clamp(0.0, 1.0))
    }

    pub fn set_combine(&mut self, combine: MaskCombineMode) -> bool {
        set_if_changed(&mut self.combine, combine)
    }

    pub fn set_feather(&mut self, feather: f32) -> bool {
        self.geometry.set_feather(feather)
    }
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct LocalMask {
    #[serde(flatten)]
    pub common: MaskCommon,
    // migration: remove in v2.0.0. Masks saved before effect components, up to
    // v1.0, carry one effect here instead of adjustments; `migrate_legacy_effect`
    // turns it into a component when the mask is opened. Listed in
    // `crate::migrations`.
    #[serde(default)]
    pub effect: MaskEffect,
    #[serde(default, skip_serializing_if = "MaskEffectSettings::is_default")]
    pub effect_settings: MaskEffectSettings,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effect_components: Vec<EffectComponent>,
    #[serde(default = "default_enabled")]
    pub adjustments_enabled: bool,
    pub opacity: f32,
    pub components: Vec<MaskComponent>,
    pub adjustments: LocalAdjustments,
}

impl Deref for LocalMask {
    type Target = MaskCommon;

    fn deref(&self) -> &Self::Target {
        &self.common
    }
}

impl DerefMut for LocalMask {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.common
    }
}

impl LocalMask {
    pub fn new(kind: MaskKind, number: usize) -> Self {
        Self {
            common: MaskCommon::new(format!("Mask {number}")),
            effect: MaskEffect::default(),
            effect_settings: MaskEffectSettings::default(),
            effect_components: Vec::new(),
            adjustments_enabled: true,
            opacity: 1.0,
            components: vec![MaskComponent::new(kind, MaskCombineMode::Add)],
            adjustments: LocalAdjustments::default(),
        }
    }

    pub fn set_opacity(&mut self, opacity: f32) -> bool {
        set_if_changed(&mut self.opacity, opacity)
    }

    /// The first submask is the base the others combine into, so it always
    /// adds. Call after any edit that can change which submask comes first.
    pub(super) fn restore_base_combine(&mut self) {
        if let Some(base) = self.components.first_mut() {
            base.combine = MaskCombineMode::Add;
        }
    }

    pub fn migrate_legacy_effect(&mut self) {
        if self.effect != MaskEffect::Adjustment {
            if effect_places(&self.effect_components) >= MAX_EFFECT_COMPONENTS {
                return;
            }
            self.effect_components.push(EffectComponent {
                effect: self.effect,
                enabled: true,
                settings: std::mem::take(&mut self.effect_settings),
            });
            if !self.adjustments.is_neutral() {
                self.adjustments_enabled = false;
            }
            self.effect = MaskEffect::Adjustment;
        }
    }

    pub fn has_active_edit(&self) -> bool {
        (!self.adjustments.is_neutral()
            && self.adjustments_enabled
            && self.effect == MaskEffect::Adjustment)
            || self
                .effect_components
                .iter()
                .any(EffectComponent::is_active)
            || self.effect_settings.effect_is_active(self.effect)
    }

    /// Whether an effect of this mask reads it beyond a tile or zoomed view
    /// ([`MaskEffect::reads_full_image_mask`]), so the full-image mask atlas
    /// holds it.
    pub fn reads_full_image_mask(&self) -> bool {
        self.effect.reads_full_image_mask()
            || self
                .effect_components
                .iter()
                .any(|component| component.enabled && component.effect.reads_full_image_mask())
    }
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct EffectComponent {
    pub effect: MaskEffect,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "MaskEffectSettings::is_default")]
    pub settings: MaskEffectSettings,
}

const fn default_enabled() -> bool {
    true
}

impl EffectComponent {
    pub fn new(effect: MaskEffect) -> Self {
        Self {
            effect,
            enabled: true,
            settings: MaskEffectSettings::initial(effect),
        }
    }

    pub fn is_active(&self) -> bool {
        self.enabled && self.settings.effect_is_active(self.effect)
    }

    /// The places of [`MAX_EFFECT_COMPONENTS`] the component takes: one per
    /// instance, such as each of Relight's lights.
    pub fn places(&self) -> usize {
        self.settings.instance_count(self.effect).max(1)
    }
}

/// The places of [`MAX_EFFECT_COMPONENTS`] that `components` take together.
pub fn effect_places(components: &[EffectComponent]) -> usize {
    components.iter().map(EffectComponent::places).sum()
}

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MaskStack {
    pub masks: Vec<LocalMask>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub global_effects: Vec<EffectComponent>,
    /// The photo's depth result in full-image coordinates, including when
    /// this stack is cropped. Depth Range components select on its
    /// [`SceneDepth::selection_map`], which they hold while it is set. Stored
    /// only as sidecar assets.
    #[serde(skip)]
    pub scene_depth: Option<SceneDepth>,
    pub selected_mask: Option<usize>,
    pub selected_component: Option<usize>,
    #[serde(skip, default)]
    pub subject_refinement: SubjectRefinement,
}

impl MaskStack {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Sets the photo's depth result, or clears it, and gives every Depth
    /// Range component its selection map, or none.
    pub fn set_scene_depth(&mut self, depth: Option<SceneDepth>) {
        let selection = depth.as_ref().map(|depth| depth.selection_map().clone());
        for component in self.masks.iter_mut().flat_map(|mask| &mut mask.components) {
            if let MaskGeometry::DepthRange { depth, .. } = &mut component.geometry {
                depth.clone_from(&selection);
            }
        }
        self.scene_depth = depth;
    }

    /// Whether any component is a Select People one, enabled or not.
    pub fn has_people_masks(&self) -> bool {
        self.masks
            .iter()
            .flat_map(|mask| &mask.components)
            .any(|component| component.kind == MaskKind::Person)
    }

    /// An active effect asks for scene depth
    /// ([`EffectSettings::wants_scene_depth`]).
    pub fn has_scene_depth_effect(&self) -> bool {
        self.has_active_effect(|component| component.settings.wants_scene_depth(component.effect))
    }

    /// An active global effect, or an active effect of an enabled, visible mask,
    /// including a mask's legacy single effect, satisfies `matches`.
    fn has_active_effect(&self, matches: impl Fn(&EffectComponent) -> bool) -> bool {
        let active = |component: &EffectComponent| component.is_active() && matches(component);
        self.global_effects.iter().any(active)
            || self.masks.iter().any(|mask| {
                mask.enabled
                    && mask.opacity > 0.0
                    && (mask.effect_components.iter().any(active)
                        || active(&EffectComponent {
                            effect: mask.effect,
                            enabled: true,
                            settings: mask.effect_settings,
                        }))
            })
    }
}

fn set_if_changed<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        false
    } else {
        *slot = value;
        true
    }
}

fn copied_name(base: &str, exists: impl Fn(&str) -> bool) -> String {
    for number in 1..=10_000usize {
        let candidate = if number == 1 {
            format!("{base} Copy")
        } else {
            format!("{base} Copy {number}")
        };
        if !exists(&candidate) {
            return candidate;
        }
    }
    format!("{base} Copy")
}
