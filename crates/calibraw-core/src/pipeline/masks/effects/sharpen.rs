use super::lanes::{effect_lanes, EffectLanes, LaneName, PackContext};
use super::params::sharpen::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct SharpenEffectSettings {
    pub amount: f32,
    pub radius: f32,
    pub detail: f32,
    pub masking: f32,
}

impl Default for SharpenEffectSettings {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            radius: RADIUS.default,
            detail: DETAIL.default,
            masking: MASKING.default,
        }
    }
}

// The layout of the Detail card's sharpening (`EffectsUniforms::sharpen`):
// `capture_sharpening` in detail_capture.wgsl reads both by position, so these
// lanes keep this order.
effect_lanes! {
    AMOUNT = 0,
    RADIUS = 1,
    DETAIL = 2,
    MASKING = 3,
}

impl EffectSettings for SharpenEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    fn is_active(&self) -> bool {
        self.amount > 1e-6
    }

    fn controls(&mut self) -> EffectControls<'_> {
        EffectControls::new(vec![
            EffectControl::Slider(&mut self.amount, AMOUNT),
            EffectControl::Slider(&mut self.radius, RADIUS),
            EffectControl::Slider(&mut self.detail, DETAIL),
            EffectControl::Slider(&mut self.masking, MASKING),
        ])
    }

    fn pack(&self, slot: &mut EffectLanes, _: &PackContext) {
        slot.set(lanes::AMOUNT, AMOUNT.clamp(self.amount));
        slot.set(lanes::RADIUS, RADIUS.clamp(self.radius));
        slot.set(lanes::DETAIL, DETAIL.clamp(self.detail));
        slot.set(lanes::MASKING, MASKING.clamp(self.masking));
    }
}
