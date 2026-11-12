//! Light Rays: shafts of light from one or more sources, in one of two
//! styles (mask_effects/light_shafts.wgsl). It replaces the earlier Light
//! Rays (legacy/light_rays.rs). Either style shows only where the mask
//! covers.
//!
//! Scene: light from sources behind the scene scattered toward the camera by
//! the air and shadowed by the scene, so trees and window frames cut the
//! rays. Each source is a point light of the light bake at the farthest
//! surface (calibraw-gpu `scene::lights`): its shadow cube says which air it
//! reaches, and the air light the share of every view ray it lights. The
//! shafts are the light the air scatters along each view ray up to the
//! visible surface, times that share. Scene depth is generated for it;
//! without depth nothing is drawn, as rays without the scene to cut them
//! would be a plain glow.
//!
//! 2D (`Flat`): the earlier Light Rays' shafts, without depth. Procedural
//! rays fan out from each source in the image and pass only through the
//! openings the mask paints along their way (light_shafts_flat.wgsl);
//! Spread, Softness, Variation and Ray count shape them.
//!
//! - Length sets how far a source's light reaches: the distance at which it
//!   has halved, in shorter edges at the source's depth (Scene), or where
//!   the rays end, in shorter edges of the image (2D).
//! - Penetration sets how much light passes what blocks it: how far light
//!   continues behind the first object it meets, fading as it goes, 0 a hard
//!   shadow and 100 most of the scene's depth (Scene); the share that passes
//!   the closed parts of the mask (2D).
//! - Warmth colours a source along the Planckian locus (warmth.rs).
//!
//! The values are packed as set; calibraw-gpu turns Length, the position and
//! Penetration into the light bake's frame, where the camera is known.

use super::instances::{mirrored_position, EffectInstances};
use super::lanes::{effect_lanes, options, EffectLanes, LaneName, PackContext};
use super::params::light_shafts::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};
use super::warmth::{warmth_swatch, warmth_tint};

/// How Light Rays draws its shafts, chosen with its card's Style control.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum LightShaftsMode {
    /// Light the air scatters, shadowed by the scene's depth.
    #[default]
    Scene,
    /// Procedural shafts through the mask's openings, without depth.
    Flat,
}

impl LightShaftsMode {
    pub const ALL: [Self; 2] = [Self::Scene, Self::Flat];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Scene => "Scene",
            Self::Flat => "2D",
        }
    }
}

/// Sources share the style, how much light passes what blocks it, which
/// belongs to the scene rather than to a light, and the 2D style's ray look.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct LightShaftsEffectSettings {
    pub mode: LightShaftsMode,
    pub penetration: f32,
    /// 2D: how wide each ray spreads around its direction, in degrees.
    pub spread: f32,
    /// 2D: the rays around a source, about.
    pub ray_count: f32,
    /// 2D: how much the rays' brightness varies.
    pub variation: f32,
    /// 2D: how soft the rays' edges are.
    pub softness: f32,
    #[serde(flatten)]
    pub sources: EffectInstances<LightShaftSource>,
}

/// One source of a Light Rays effect.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct LightShaftSource {
    pub amount: f32,
    pub length: f32,
    /// Percentages of the full, uncropped image; outside 0–100 beyond the
    /// frame.
    pub source: [f32; 2],
    pub warmth: f32,
}

impl Default for LightShaftsEffectSettings {
    fn default() -> Self {
        Self {
            mode: LightShaftsMode::Scene,
            penetration: PENETRATION.default,
            spread: SPREAD.default,
            ray_count: RAY_COUNT.default,
            variation: VARIATION.default,
            softness: SOFTNESS.default,
            sources: EffectInstances::default(),
        }
    }
}

impl Default for LightShaftSource {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            length: LENGTH.default,
            source: [SOURCE_X.default, SOURCE_Y.default],
            warmth: WARMTH.default,
        }
    }
}

impl LightShaftSource {
    fn is_active(&self) -> bool {
        self.amount > 1e-6 && self.length > 1e-6
    }

    /// A copy of this source placed across the frame from it.
    pub fn mirrored(&self) -> Self {
        Self {
            source: mirrored_position(self.source, [SOURCE_X, SOURCE_Y]),
            ..*self
        }
    }

    /// The source's colour for its swatch and canvas handle: sRGB encoded,
    /// brightest channel 1.
    pub fn swatch(&self) -> [f32; 3] {
        warmth_swatch(WARMTH.clamp(self.warmth) / 100.0)
    }
}

effect_lanes! {
    /// Amount in percent.
    AMOUNT = 0,
    /// Length in percent; calibraw-gpu turns it into the light's reach.
    LENGTH = 1,
    /// The source in percent of the full image.
    SOURCE_X = 2,
    SOURCE_Y = 3,
    /// The source's colour, scene-linear Rec.2020 with luminance 1.
    TINT = 4 (color),
    /// Penetration in percent; calibraw-gpu turns it into a distance.
    PENETRATION = 7,
    /// 2D: the ray look as set (Spread in degrees, the others in percent).
    SPREAD = 8,
    RAY_COUNT = 9,
    VARIATION = 10,
    SOFTNESS = 11,
}

impl EffectSettings for LightShaftsEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    fn is_active(&self) -> bool {
        self.sources
            .as_slice()
            .iter()
            .any(LightShaftSource::is_active)
    }

    fn wants_scene_depth(&self) -> bool {
        self.mode == LightShaftsMode::Scene
    }

    fn instance_count(&self) -> usize {
        self.sources.count()
    }

    fn controls(&mut self) -> EffectControls<'_> {
        let mut shown = Vec::new();
        for source in self.sources.as_mut_slice() {
            shown.extend([
                EffectControl::Slider(&mut source.amount, AMOUNT),
                EffectControl::Slider(&mut source.length, LENGTH),
                EffectControl::Position(&mut source.source, [SOURCE_X, SOURCE_Y]),
                EffectControl::Slider(&mut source.warmth, WARMTH),
            ]);
        }
        shown.push(EffectControl::Slider(&mut self.penetration, PENETRATION));
        EffectControls::new(shown).details(
            "Ray details",
            vec![
                EffectControl::Slider(&mut self.spread, SPREAD),
                EffectControl::Slider(&mut self.softness, SOFTNESS),
                EffectControl::Slider(&mut self.variation, VARIATION),
                EffectControl::Slider(&mut self.ray_count, RAY_COUNT),
            ],
        )
    }

    /// Each source's slot also holds the shared values: the style, Penetration,
    /// which its light in the light bake carries, and the 2D ray look.
    fn pack(&self, slot: &mut EffectLanes, context: &PackContext) {
        let Some(source) = self.sources.get(context.instance) else {
            return;
        };
        slot.set(lanes::AMOUNT, AMOUNT.clamp(source.amount));
        slot.set(lanes::LENGTH, LENGTH.clamp(source.length));
        slot.set(lanes::SOURCE_X, SOURCE_X.clamp(source.source[0]));
        slot.set(lanes::SOURCE_Y, SOURCE_Y.clamp(source.source[1]));
        slot.set_color(
            lanes::TINT,
            warmth_tint(WARMTH.clamp(source.warmth) / 100.0),
        );
        slot.set(lanes::PENETRATION, PENETRATION.clamp(self.penetration));
        slot.set(lanes::SPREAD, SPREAD.clamp(self.spread));
        slot.set(lanes::RAY_COUNT, RAY_COUNT.clamp(self.ray_count));
        slot.set(lanes::VARIATION, VARIATION.clamp(self.variation));
        slot.set(lanes::SOFTNESS, SOFTNESS.clamp(self.softness));
        slot.set_switch(
            options::LIGHT_SHAFTS_FLAT,
            self.mode == LightShaftsMode::Flat,
        );
    }
}
