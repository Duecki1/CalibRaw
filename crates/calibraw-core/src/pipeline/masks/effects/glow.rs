use super::lanes::{effect_lanes, EffectLanes, LaneName, PackContext};
use super::params::glow::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct GlowEffectSettings {
    /// The mask emits the picked color itself, so the glow also shows on dark
    /// surfaces. Off, only bright pixels inside the mask bloom.
    pub self_illuminating: bool,
    pub amount: f32,
    pub radius: f32,
    pub core: f32,
    pub color: [f32; 3],
    /// How much a self-illuminating Glow lights the scene around it
    /// (scene_emission.wgsl).
    pub scene_light: f32,
}

/// The serialized baseline: a saved Glow whose settings were never changed
/// omits them and decodes to this, so it stays self-illuminating. Edits saved
/// before Scene light omit it and light nothing, as they did.
impl Default for GlowEffectSettings {
    fn default() -> Self {
        Self {
            self_illuminating: true,
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
    SELF_ILLUMINATING = 3,
    COLOR = 4 (color),
    SCENE_LIGHT = 7,
}

impl EffectSettings for GlowEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    /// New Glow blooms the bright areas inside its mask. Self-illumination is
    /// opt-in, and lights the scene once it is on.
    fn initial() -> Self {
        Self {
            self_illuminating: false,
            scene_light: SCENE_LIGHT.default,
            ..Self::default()
        }
    }

    fn is_active(&self) -> bool {
        self.amount.abs() > 1e-6
    }

    fn controls(&mut self) -> EffectControls<'_> {
        EffectControls::new(vec![
            EffectControl::Toggle(&mut self.self_illuminating, SELF_ILLUMINATING),
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
        slot.set(
            lanes::SELF_ILLUMINATING,
            if self.self_illuminating { 1.0 } else { 0.0 },
        );
        slot.set_color(lanes::COLOR, COLOR.clamp(self.color));
        slot.set(lanes::SCENE_LIGHT, SCENE_LIGHT.clamp(self.scene_light));
    }
}
