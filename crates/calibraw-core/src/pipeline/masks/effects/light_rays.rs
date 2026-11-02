use super::lanes::{effect_lanes, EffectLanes, LaneName, PackContext};
use super::params::light_rays::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct LightRaysEffectSettings {
    pub amount: f32,
    pub length: f32,
    pub source: [f32; 2],
    pub spread: f32,
    pub fade: f32,
    pub ray_count: f32,
    pub variation: f32,
    pub softness: f32,
    pub color: [f32; 3],
}

impl Default for LightRaysEffectSettings {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            length: LENGTH.default,
            source: [SOURCE_X.default, SOURCE_Y.default],
            spread: SPREAD.default,
            fade: FADE.default,
            ray_count: RAY_COUNT.default,
            variation: VARIATION.default,
            softness: SOFTNESS.default,
            color: COLOR.default,
        }
    }
}

effect_lanes! {
    AMOUNT = 0,
    LENGTH = 1,
    SOURCE_X = 2,
    SOURCE_Y = 3,
    COLOR = 4 (color),
    FADE = 7,
    SPREAD = 8,
    RAY_COUNT = 9,
    VARIATION = 10,
    SOFTNESS = 11,
}

impl EffectSettings for LightRaysEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    fn is_active(&self) -> bool {
        self.amount.abs() > 1e-6 && self.length > 1e-6
    }

    fn controls(&mut self) -> EffectControls<'_> {
        EffectControls::new(vec![
            EffectControl::Slider(&mut self.amount, AMOUNT),
            EffectControl::Slider(&mut self.length, LENGTH),
            EffectControl::Position(&mut self.source, [SOURCE_X, SOURCE_Y]),
            EffectControl::Color(&mut self.color, COLOR),
        ])
        .details(
            "Ray details",
            vec![
                EffectControl::Slider(&mut self.spread, SPREAD),
                EffectControl::Slider(&mut self.fade, FADE),
                EffectControl::Slider(&mut self.softness, SOFTNESS),
                EffectControl::Slider(&mut self.variation, VARIATION),
                EffectControl::Slider(&mut self.ray_count, RAY_COUNT),
            ],
        )
    }

    fn pack(&self, slot: &mut EffectLanes, _: &PackContext) {
        slot.set(lanes::AMOUNT, AMOUNT.clamp(self.amount));
        slot.set(lanes::LENGTH, LENGTH.clamp(self.length));
        slot.set(lanes::SOURCE_X, SOURCE_X.clamp(self.source[0]));
        slot.set(lanes::SOURCE_Y, SOURCE_Y.clamp(self.source[1]));
        slot.set_color(lanes::COLOR, COLOR.clamp(self.color));
        slot.set(lanes::FADE, FADE.clamp(self.fade));
        slot.set(lanes::SPREAD, SPREAD.clamp(self.spread));
        slot.set(lanes::RAY_COUNT, RAY_COUNT.clamp(self.ray_count));
        slot.set(lanes::VARIATION, VARIATION.clamp(self.variation));
        slot.set(lanes::SOFTNESS, SOFTNESS.clamp(self.softness));
    }
}
