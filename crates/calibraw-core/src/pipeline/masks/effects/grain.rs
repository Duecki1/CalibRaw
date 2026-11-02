use super::lanes::{effect_lanes, EffectLanes, LaneName, PackContext};
use super::params::grain::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct GrainEffectSettings {
    pub amount: f32,
    pub size: f32,
    pub roughness: f32,
    pub color: f32,
    pub seed: f32,
}

impl Default for GrainEffectSettings {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            size: SIZE.default,
            roughness: ROUGHNESS.default,
            color: COLOR.default,
            seed: SEED.default,
        }
    }
}

effect_lanes! {
    AMOUNT = 0,
    SIZE = 1,
    ROUGHNESS = 2,
    COLOR = 3,
    SEED = 4,
}

impl EffectSettings for GrainEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    fn is_active(&self) -> bool {
        self.amount > 1e-6
    }

    fn controls(&mut self) -> EffectControls<'_> {
        EffectControls::new(vec![
            EffectControl::Slider(&mut self.amount, AMOUNT),
            EffectControl::Slider(&mut self.size, SIZE),
        ])
        .details(
            "Texture details",
            vec![
                EffectControl::Slider(&mut self.roughness, ROUGHNESS),
                EffectControl::Slider(&mut self.color, COLOR),
                EffectControl::Seed(&mut self.seed, SEED),
            ],
        )
    }

    fn pack(&self, slot: &mut EffectLanes, _: &PackContext) {
        slot.set(lanes::AMOUNT, AMOUNT.clamp(self.amount));
        slot.set(lanes::SIZE, SIZE.clamp(self.size));
        slot.set(lanes::ROUGHNESS, ROUGHNESS.clamp(self.roughness));
        slot.set(lanes::COLOR, COLOR.clamp(self.color));
        // The shader hashes the seed as a whole number.
        slot.set(lanes::SEED, SEED.clamp(self.seed).round());
    }
}
