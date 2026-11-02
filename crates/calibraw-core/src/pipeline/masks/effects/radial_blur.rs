use super::lanes::{effect_lanes, EffectLanes, LaneName, PackContext};
use super::params::radial_blur::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum RadialBlurMode {
    #[default]
    Zoom,
    Spin,
}

impl RadialBlurMode {
    pub const ALL: [Self; 2] = [Self::Zoom, Self::Spin];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Zoom => "Zoom",
            Self::Spin => "Spin",
        }
    }

    pub const fn shader_value(self) -> f32 {
        match self {
            Self::Zoom => 0.0,
            Self::Spin => 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct RadialBlurEffectSettings {
    pub amount: f32,
    pub strength: f32,
    pub center: [f32; 2],
    pub mode: RadialBlurMode,
}

impl Default for RadialBlurEffectSettings {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            strength: STRENGTH.default,
            center: [CENTER_X.default, CENTER_Y.default],
            mode: RadialBlurMode::Zoom,
        }
    }
}

effect_lanes! {
    AMOUNT = 0,
    STRENGTH = 1,
    CENTER_X = 2,
    CENTER_Y = 3,
    MODE = 4,
}

impl EffectSettings for RadialBlurEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    fn is_active(&self) -> bool {
        self.amount.abs() > 1e-6 && self.strength > 1e-6
    }

    fn controls(&mut self) -> EffectControls<'_> {
        EffectControls::new(vec![
            EffectControl::Slider(&mut self.amount, AMOUNT),
            EffectControl::Slider(&mut self.strength, STRENGTH),
            EffectControl::Position(&mut self.center, [CENTER_X, CENTER_Y]),
        ])
    }

    fn pack(&self, slot: &mut EffectLanes, _: &PackContext) {
        slot.set(lanes::AMOUNT, AMOUNT.clamp(self.amount));
        slot.set(lanes::STRENGTH, STRENGTH.clamp(self.strength));
        slot.set(lanes::CENTER_X, CENTER_X.clamp(self.center[0]));
        slot.set(lanes::CENTER_Y, CENTER_Y.clamp(self.center[1]));
        slot.set(lanes::MODE, self.mode.shader_value());
    }
}
