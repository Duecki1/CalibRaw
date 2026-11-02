use super::lanes::{effect_lanes, EffectLanes, LaneName, PackContext};
use super::params::tilt_shift::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct TiltShiftEffectSettings {
    pub amount: f32,
    pub radius: f32,
    pub center: [f32; 2],
    pub angle: f32,
    pub focus_width: f32,
    pub feather: f32,
}

impl Default for TiltShiftEffectSettings {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            radius: RADIUS.default,
            center: [CENTER_X.default, CENTER_Y.default],
            angle: ANGLE.default,
            focus_width: FOCUS_WIDTH.default,
            feather: FEATHER.default,
        }
    }
}

effect_lanes! {
    AMOUNT = 0,
    RADIUS = 1,
    CENTER_X = 2,
    CENTER_Y = 3,
    ANGLE = 4,
    FOCUS_WIDTH = 5,
    FEATHER = 6,
}

impl EffectSettings for TiltShiftEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    fn is_active(&self) -> bool {
        self.amount.abs() > 1e-6 && self.radius > 1e-6
    }

    fn controls(&mut self) -> EffectControls<'_> {
        EffectControls::new(vec![
            EffectControl::Slider(&mut self.amount, AMOUNT),
            EffectControl::Slider(&mut self.radius, RADIUS),
            EffectControl::Slider(&mut self.focus_width, FOCUS_WIDTH),
        ])
        .details(
            "Focus band details",
            vec![
                EffectControl::Position(&mut self.center, [CENTER_X, CENTER_Y]),
                EffectControl::Angle(&mut self.angle, ANGLE),
                EffectControl::Slider(&mut self.feather, FEATHER),
            ],
        )
    }

    fn pack(&self, slot: &mut EffectLanes, _: &PackContext) {
        slot.set(lanes::AMOUNT, AMOUNT.clamp(self.amount));
        slot.set(lanes::RADIUS, RADIUS.clamp(self.radius));
        slot.set(lanes::CENTER_X, CENTER_X.clamp(self.center[0]));
        slot.set(lanes::CENTER_Y, CENTER_Y.clamp(self.center[1]));
        slot.set(lanes::ANGLE, ANGLE.clamp(self.angle));
        slot.set(lanes::FOCUS_WIDTH, FOCUS_WIDTH.clamp(self.focus_width));
        slot.set(lanes::FEATHER, FEATHER.clamp(self.feather));
    }
}
