use super::lanes::{effect_lanes, EffectLanes, LaneName, PackContext};
use super::params::neon::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct NeonEffectSettings {
    pub amount: f32,
    pub edge_width: f32,
    pub detail: f32,
    pub glow: f32,
    pub background: f32,
    pub color: [f32; 3],
}

impl Default for NeonEffectSettings {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            edge_width: EDGE_WIDTH.default,
            detail: DETAIL.default,
            glow: GLOW.default,
            background: BACKGROUND.default,
            color: COLOR.default,
        }
    }
}

effect_lanes! {
    AMOUNT = 0,
    EDGE_WIDTH = 1,
    DETAIL = 2,
    GLOW = 3,
    COLOR = 4 (color),
    BACKGROUND = 7,
}

impl EffectSettings for NeonEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    fn is_active(&self) -> bool {
        self.amount.abs() > 1e-6
    }

    fn controls(&mut self) -> EffectControls<'_> {
        EffectControls::new(vec![
            EffectControl::Slider(&mut self.amount, AMOUNT),
            EffectControl::Slider(&mut self.edge_width, EDGE_WIDTH),
            EffectControl::Color(&mut self.color, COLOR),
        ])
        .details(
            "Neon details",
            vec![
                EffectControl::Slider(&mut self.glow, GLOW),
                EffectControl::Slider(&mut self.detail, DETAIL),
                EffectControl::Slider(&mut self.background, BACKGROUND),
            ],
        )
    }

    fn pack(&self, slot: &mut EffectLanes, _: &PackContext) {
        slot.set(lanes::AMOUNT, AMOUNT.clamp(self.amount));
        slot.set(lanes::EDGE_WIDTH, EDGE_WIDTH.clamp(self.edge_width));
        slot.set(lanes::DETAIL, DETAIL.clamp(self.detail));
        slot.set(lanes::GLOW, GLOW.clamp(self.glow));
        slot.set_color(lanes::COLOR, COLOR.clamp(self.color));
        slot.set(lanes::BACKGROUND, BACKGROUND.clamp(self.background));
    }
}
