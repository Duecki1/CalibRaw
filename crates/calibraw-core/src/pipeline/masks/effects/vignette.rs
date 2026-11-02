use super::lanes::{effect_lanes, EffectLanes, LaneName, PackContext};
use super::params::vignette::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct VignetteEffectSettings {
    pub amount: f32,
    pub midpoint: f32,
    pub roundness: f32,
    pub feather: f32,
    pub highlights: f32,
    /// Center percentages within the cropped and rotated frame, not an export tile.
    pub center: [f32; 2],
}

impl Default for VignetteEffectSettings {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            midpoint: MIDPOINT.default,
            roundness: ROUNDNESS.default,
            feather: FEATHER.default,
            highlights: HIGHLIGHTS.default,
            center: [CENTER_X.default, CENTER_Y.default],
        }
    }
}

effect_lanes! {
    AMOUNT = 0,
    MIDPOINT = 1,
    ROUNDNESS = 2,
    FEATHER = 3,
    HIGHLIGHTS = 4,
    CENTER_X = 5,
    CENTER_Y = 6,
}

impl EffectSettings for VignetteEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    fn is_active(&self) -> bool {
        self.amount.abs() > 1e-6
    }

    fn controls(&mut self) -> EffectControls<'_> {
        EffectControls::new(vec![
            EffectControl::Slider(&mut self.amount, AMOUNT),
            EffectControl::Slider(&mut self.midpoint, MIDPOINT),
            EffectControl::Slider(&mut self.feather, FEATHER),
        ])
        .details(
            "Shape details",
            vec![
                EffectControl::Slider(&mut self.roundness, ROUNDNESS),
                EffectControl::Slider(&mut self.highlights, HIGHLIGHTS),
                EffectControl::Position(&mut self.center, [CENTER_X, CENTER_Y]),
            ],
        )
    }

    fn pack(&self, slot: &mut EffectLanes, _: &PackContext) {
        slot.set(lanes::AMOUNT, AMOUNT.clamp(self.amount));
        slot.set(lanes::MIDPOINT, MIDPOINT.clamp(self.midpoint));
        slot.set(lanes::ROUNDNESS, ROUNDNESS.clamp(self.roundness));
        slot.set(lanes::FEATHER, FEATHER.clamp(self.feather));
        slot.set(lanes::HIGHLIGHTS, HIGHLIGHTS.clamp(self.highlights));
        slot.set(lanes::CENTER_X, CENTER_X.clamp(self.center[0]));
        slot.set(lanes::CENTER_Y, CENTER_Y.clamp(self.center[1]));
    }
}
