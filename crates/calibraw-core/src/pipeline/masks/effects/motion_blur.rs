use super::lanes::{effect_lanes, EffectLanes, LaneName, PackContext};
use super::params::motion_blur::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct MotionBlurEffectSettings {
    pub amount: f32,
    pub distance: f32,
    pub angle: f32,
}

impl Default for MotionBlurEffectSettings {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            distance: DISTANCE.default,
            angle: ANGLE.default,
        }
    }
}

effect_lanes! {
    AMOUNT = 0,
    DISTANCE = 1,
    ANGLE = 2,
}

impl EffectSettings for MotionBlurEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    fn is_active(&self) -> bool {
        self.amount.abs() > 1e-6 && self.distance > 1e-6
    }

    fn controls(&mut self) -> EffectControls<'_> {
        EffectControls::new(vec![
            EffectControl::Slider(&mut self.amount, AMOUNT),
            EffectControl::Slider(&mut self.distance, DISTANCE),
            EffectControl::Angle(&mut self.angle, ANGLE),
        ])
    }

    fn pack(&self, slot: &mut EffectLanes, _: &PackContext) {
        slot.set(lanes::AMOUNT, AMOUNT.clamp(self.amount));
        slot.set(lanes::DISTANCE, DISTANCE.clamp(self.distance));
        slot.set(lanes::ANGLE, ANGLE.clamp(self.angle));
    }
}
