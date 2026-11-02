use super::lanes::{effect_lanes, EffectLanes, LaneName, PackContext};
use super::params::relight::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};

/// A virtual point light that shades the scene reconstructed from scene depth.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct RelightEffectSettings {
    pub amount: f32,
    /// Percentages of the full, uncropped image, as for Light Rays.
    pub source: [f32; 2],
    /// Light depth: 0 is level with the nearest surface, 100 with the farthest;
    /// negative values move the light toward the camera (-100 is at the camera).
    pub depth: f32,
    pub reach: f32,
    pub size: f32,
    /// Whether nearer objects cast shadows; off keeps the Shadows strength.
    /// New lights start without shadows.
    pub shadows_enabled: bool,
    pub shadows: f32,
    pub relief: f32,
    pub ambient: f32,
    pub color: [f32; 3],
}

impl Default for RelightEffectSettings {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            source: [SOURCE_X.default, SOURCE_Y.default],
            depth: DEPTH.default,
            reach: REACH.default,
            size: SIZE.default,
            shadows_enabled: false,
            shadows: SHADOWS.default,
            relief: RELIEF.default,
            ambient: AMBIENT.default,
            color: COLOR.default,
        }
    }
}

effect_lanes! {
    AMOUNT = 0,
    REACH = 1,
    SOURCE_X = 2,
    SOURCE_Y = 3,
    COLOR = 4 (color),
    DEPTH = 7,
    SIZE = 8,
    SHADOWS = 9,
    RELIEF = 10,
    AMBIENT = 11,
}

impl EffectSettings for RelightEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    /// Active when it adds light or dims the existing light.
    fn is_active(&self) -> bool {
        self.amount.abs() > 1e-6 || self.ambient < AMBIENT.max - 1e-6
    }

    fn controls(&mut self) -> EffectControls<'_> {
        EffectControls::new(vec![
            EffectControl::Slider(&mut self.amount, AMOUNT),
            EffectControl::Position(&mut self.source, [SOURCE_X, SOURCE_Y]),
            EffectControl::Slider(&mut self.depth, DEPTH),
            EffectControl::Slider(&mut self.ambient, AMBIENT),
            EffectControl::Color(&mut self.color, COLOR),
            EffectControl::Toggle(&mut self.shadows_enabled, CAST_SHADOWS),
        ])
        .details(
            "Light details",
            vec![
                EffectControl::Slider(&mut self.reach, REACH),
                EffectControl::Slider(&mut self.size, SIZE),
                EffectControl::Slider(&mut self.shadows, SHADOWS),
                EffectControl::Slider(&mut self.relief, RELIEF),
            ],
        )
    }

    fn wants_scene_depth(&self) -> bool {
        true
    }

    fn pack(&self, slot: &mut EffectLanes, _: &PackContext) {
        slot.set(lanes::AMOUNT, AMOUNT.clamp(self.amount));
        slot.set(lanes::REACH, REACH.clamp(self.reach));
        slot.set(lanes::SOURCE_X, SOURCE_X.clamp(self.source[0]));
        slot.set(lanes::SOURCE_Y, SOURCE_Y.clamp(self.source[1]));
        slot.set_color(lanes::COLOR, COLOR.clamp(self.color));
        slot.set(lanes::DEPTH, DEPTH.clamp(self.depth));
        slot.set(lanes::SIZE, SIZE.clamp(self.size));
        // Switched-off shadows keep their strength in the settings only.
        let shadows = if self.shadows_enabled {
            SHADOWS.clamp(self.shadows)
        } else {
            0.0
        };
        slot.set(lanes::SHADOWS, shadows);
        slot.set(lanes::RELIEF, RELIEF.clamp(self.relief));
        slot.set(lanes::AMBIENT, AMBIENT.clamp(self.ambient));
    }
}
