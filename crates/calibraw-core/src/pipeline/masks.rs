use half::f16;
use rayon::prelude::*;
use std::f32::consts::TAU;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use super::LensGeometryMap;
use crate::color_math::{linear_srgb_to_oklab, srgb_decode_signed};

mod content_dependencies;
mod effects;
mod raster_cache;

pub use content_dependencies::ContentDependencies;

pub use effects::{
    params as effect_params, BlurEffectSettings, EdgeGlowEffectSettings, FogEffectSettings,
    GlowEffectSettings, GrainEffectSettings, HalationEffectSettings, LensBlurEffectSettings,
    LightRaysEffectSettings, MaskEffect, MaskEffectCategory, MaskEffectSettings,
    MotionBlurEffectSettings, NeonEffectSettings, PixelateEffectSettings, RadialBlurEffectSettings,
    RadialBlurMode, SmokeEffectSettings, TiltShiftEffectSettings, VignetteEffectSettings,
};

mod brush;
mod editing;
mod layers;
mod probability;
mod raster;
mod shapes;
pub use brush::*;
use probability::*;
pub use raster::luminance_range_weight;
use raster::*;
pub use shapes::*;

mod adjustments;
mod mask_geometry;
mod primitives;
pub use adjustments::*;
pub use mask_geometry::*;
pub use primitives::*;

pub const MAX_LOCAL_MASKS: usize = 32;
pub const MAX_EFFECT_COMPONENTS: usize = 12;
pub const MAX_MASK_COMPONENTS: usize = 64;
pub const MAX_PATH_POINTS: usize = 256;
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
            Self::DepthRange => "Depth Range",
        }
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
    pub geometry: MaskGeometry,
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
            geometry: MaskGeometry::for_kind(kind),
        }
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

    pub fn migrate_legacy_effect(&mut self) {
        if self.effect != MaskEffect::Adjustment {
            if self.effect_components.len() >= MAX_EFFECT_COMPONENTS {
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
            || (self.effect != MaskEffect::Adjustment
                && EffectComponent {
                    effect: self.effect,
                    enabled: true,
                    settings: self.effect_settings,
                }
                .is_active())
    }

    pub fn has_light_rays_effect(&self) -> bool {
        self.effect == MaskEffect::LightRays
            || self
                .effect_components
                .iter()
                .any(|component| component.enabled && component.effect == MaskEffect::LightRays)
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
            settings: MaskEffectSettings::default(),
        }
    }

    pub fn is_active(&self) -> bool {
        if !self.enabled {
            return false;
        }
        match self.effect {
            MaskEffect::Adjustment => false,
            MaskEffect::Blur => self.settings.blur.is_active(),
            MaskEffect::LensBlur => self.settings.lens_blur.is_active(),
            MaskEffect::MotionBlur => self.settings.motion_blur.is_active(),
            MaskEffect::RadialBlur => self.settings.radial_blur.is_active(),
            MaskEffect::TiltShift => self.settings.tilt_shift.is_active(),
            MaskEffect::Glow => self.settings.glow.is_active(),
            MaskEffect::LightRays => self.settings.light_rays.is_active(),
            MaskEffect::Neon => self.settings.neon.is_active(),
            MaskEffect::EdgeGlow => self.settings.edge_glow.is_active(),
            MaskEffect::Pixelate => self.settings.pixelate.is_active(),
            MaskEffect::Fog => self.settings.fog.is_active(),
            MaskEffect::Smoke => self.settings.smoke.is_active(),
            MaskEffect::Grain => self.settings.grain.is_active(),
            MaskEffect::Halation => self.settings.halation.is_active(),
            MaskEffect::Vignette => self.settings.vignette.is_active(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MaskStack {
    pub masks: Vec<LocalMask>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub global_effects: Vec<EffectComponent>,
    /// Scene depth in full-image coordinates, including when this stack is cropped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene_depth: Option<MaskImage>,
    pub selected_mask: Option<usize>,
    pub selected_component: Option<usize>,
    #[serde(skip, default)]
    pub subject_refinement: SubjectRefinement,
}

impl MaskStack {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Returns full-image depth, falling back to the first cached depth-range image.
    /// Disabled masks/components still provide useful scene data. Consumers sampling
    /// full-image UVs must use width, height and pixels directly: a cropped depth-range
    /// image shares its original pixels but has a region-specific sampling rectangle.
    pub fn scene_depth_image(&self) -> Option<&MaskImage> {
        self.scene_depth.as_ref().or_else(|| {
            self.masks
                .iter()
                .flat_map(|mask| &mask.components)
                .find_map(|component| match &component.geometry {
                    MaskGeometry::DepthRange { depth, .. } => depth.as_ref(),
                    _ => None,
                })
        })
    }

    pub fn has_depth_fog_effect(&self) -> bool {
        self.has_fog_effect_matching(|settings| settings.depth_enabled)
    }

    fn has_fog_effect_matching(&self, matches: impl Fn(&FogEffectSettings) -> bool) -> bool {
        let active_fog = |component: &EffectComponent| {
            component.effect == MaskEffect::Fog
                && component.is_active()
                && matches(&component.settings.fog)
        };
        self.global_effects.iter().any(active_fog)
            || self.masks.iter().any(|mask| {
                mask.enabled
                    && mask.opacity > 0.0
                    && (mask.effect_components.iter().any(active_fog)
                        || active_fog(&EffectComponent {
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

#[cfg(test)]
mod tests;

#[cfg(test)]
mod zoom_tests;

#[cfg(test)]
mod lens_tests;
