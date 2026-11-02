use super::lanes::{effect_lanes, EffectLanes, LaneName, PackContext};
use super::params::halation::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct HalationEffectSettings {
    pub amount: f32,
    pub radius: f32,
    pub threshold: f32,
    pub warmth: f32,
}

impl Default for HalationEffectSettings {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            radius: RADIUS.default,
            threshold: THRESHOLD.default,
            warmth: WARMTH.default,
        }
    }
}

effect_lanes! {
    AMOUNT = 0,
    RADIUS = 1,
    THRESHOLD = 2,
    WARMTH = 3,
}

impl EffectSettings for HalationEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    fn is_active(&self) -> bool {
        self.amount > 1e-6 && self.radius > 1e-6
    }

    fn controls(&mut self) -> EffectControls<'_> {
        EffectControls::new(vec![
            EffectControl::Slider(&mut self.amount, AMOUNT),
            EffectControl::Slider(&mut self.radius, RADIUS),
            EffectControl::Slider(&mut self.threshold, THRESHOLD),
        ])
        .details(
            "Color details",
            vec![EffectControl::Slider(&mut self.warmth, WARMTH)],
        )
    }

    fn pack(&self, slot: &mut EffectLanes, _: &PackContext) {
        slot.set(lanes::AMOUNT, AMOUNT.clamp(self.amount));
        slot.set(lanes::RADIUS, RADIUS.clamp(self.radius));
        slot.set(lanes::THRESHOLD, THRESHOLD.clamp(self.threshold));
        slot.set(lanes::WARMTH, WARMTH.clamp(self.warmth));
    }
}
