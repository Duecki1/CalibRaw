use super::lanes::{effect_lanes, EffectLanes, LaneName, PackContext};
use super::params::lens_blur::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct LensBlurEffectSettings {
    pub amount: f32,
    pub radius: f32,
    pub blades: f32,
    pub rotation: f32,
    pub highlight_boost: f32,
}

impl Default for LensBlurEffectSettings {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            radius: RADIUS.default,
            blades: BLADES.default,
            rotation: ROTATION.default,
            highlight_boost: HIGHLIGHTS.default,
        }
    }
}

effect_lanes! {
    AMOUNT = 0,
    RADIUS = 1,
    BLADES = 2,
    ROTATION = 3,
    HIGHLIGHTS = 4,
}

impl EffectSettings for LensBlurEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    fn is_active(&self) -> bool {
        self.amount.abs() > 1e-6 && self.radius > 1e-6
    }

    fn controls(&mut self) -> EffectControls<'_> {
        EffectControls::new(vec![
            EffectControl::Slider(&mut self.amount, AMOUNT),
            EffectControl::Slider(&mut self.radius, RADIUS),
            EffectControl::Slider(&mut self.highlight_boost, HIGHLIGHTS),
        ])
        .details(
            "Aperture details",
            vec![
                EffectControl::Slider(&mut self.blades, BLADES),
                EffectControl::Angle(&mut self.rotation, ROTATION),
            ],
        )
    }

    fn pack(&self, slot: &mut EffectLanes, _: &PackContext) {
        slot.set(lanes::AMOUNT, AMOUNT.clamp(self.amount));
        slot.set(lanes::RADIUS, RADIUS.clamp(self.radius));
        // The aperture has whole blades.
        slot.set(lanes::BLADES, BLADES.clamp(self.blades).round());
        slot.set(lanes::ROTATION, ROTATION.clamp(self.rotation));
        slot.set(lanes::HIGHLIGHTS, HIGHLIGHTS.clamp(self.highlight_boost));
    }
}
