//! Glowing Area: the mask emits the picked colour itself, so its glow also
//! shows on dark surfaces, and it can light the scene around it (glow.wgsl,
//! scene_emission.wgsl). Older edits saved it as a Glow with Self-illuminating
//! on; one with it off became `LegacyHighlightGlow`
//! (migrations/highlight_glow.rs).

use super::lanes::{effect_lanes, EffectLanes, LaneName, PackContext};
use super::params::glow::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct GlowEffectSettings {
    pub amount: f32,
    pub radius: f32,
    pub core: f32,
    pub color: [f32; 3],
    /// How much it lights the scene around it (scene_emission.wgsl).
    pub scene_light: f32,
}

/// The serialized baseline: a saved Glowing Area whose settings were never
/// changed omits them and decodes to this. Edits saved before Scene light
/// omit it and light nothing, as they did.
impl Default for GlowEffectSettings {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            radius: RADIUS.default,
            core: CORE.default,
            color: COLOR.default,
            scene_light: 0.0,
        }
    }
}

effect_lanes! {
    AMOUNT = 0,
    RADIUS = 1,
    CORE = 2,
    COLOR = 4 (color),
    SCENE_LIGHT = 7,
}

impl EffectSettings for GlowEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    /// A new Glowing Area lights the scene.
    fn initial() -> Self {
        Self {
            scene_light: SCENE_LIGHT.default,
            ..Self::default()
        }
    }

    fn is_active(&self) -> bool {
        self.amount.abs() > 1e-6
    }

    fn controls(&mut self) -> EffectControls<'_> {
        EffectControls::new(vec![
            EffectControl::Slider(&mut self.amount, AMOUNT),
            EffectControl::Slider(&mut self.radius, RADIUS),
            EffectControl::Color(&mut self.color, COLOR),
            EffectControl::Slider(&mut self.scene_light, SCENE_LIGHT),
        ])
        .details(
            "Light details",
            vec![EffectControl::Slider(&mut self.core, CORE)],
        )
    }

    fn pack(&self, slot: &mut EffectLanes, _: &PackContext) {
        slot.set(lanes::AMOUNT, AMOUNT.clamp(self.amount));
        slot.set(lanes::RADIUS, RADIUS.clamp(self.radius));
        slot.set(lanes::CORE, CORE.clamp(self.core));
        slot.set_color(lanes::COLOR, COLOR.clamp(self.color));
        slot.set(lanes::SCENE_LIGHT, SCENE_LIGHT.clamp(self.scene_light));
    }
}
