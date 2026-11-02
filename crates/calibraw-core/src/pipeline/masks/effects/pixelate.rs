use super::lanes::{effect_lanes, EffectLanes, LaneName, PackContext};
use super::params::pixelate::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct PixelateEffectSettings {
    pub amount: f32,
    pub block_size: f32,
}

impl Default for PixelateEffectSettings {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            block_size: BLOCK_SIZE.default,
        }
    }
}

effect_lanes! {
    AMOUNT = 0,
    BLOCK_SIZE = 1,
}

impl EffectSettings for PixelateEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    fn is_active(&self) -> bool {
        self.amount.abs() > 1e-6 && self.block_size > 1.0
    }

    fn controls(&mut self) -> EffectControls<'_> {
        EffectControls::new(vec![
            EffectControl::Slider(&mut self.amount, AMOUNT),
            EffectControl::Slider(&mut self.block_size, BLOCK_SIZE),
        ])
    }

    fn pack(&self, slot: &mut EffectLanes, _: &PackContext) {
        slot.set(lanes::AMOUNT, AMOUNT.clamp(self.amount));
        slot.set(lanes::BLOCK_SIZE, BLOCK_SIZE.clamp(self.block_size));
    }
}
