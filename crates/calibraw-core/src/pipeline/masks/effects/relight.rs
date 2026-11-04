use super::instances::{mirrored_position, EffectInstances};
use super::lanes::{effect_lanes, EffectLanes, LaneName, PackContext};
use super::params::relight::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};

/// Virtual point lights that shade the scene reconstructed from scene depth.
/// The lights share the scene's Relief and one Ambient level, and add up on
/// the surface they light (relight.wgsl).
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct RelightEffectSettings {
    pub relief: f32,
    pub ambient: f32,
    #[serde(flatten)]
    pub lights: EffectInstances<RelightLight>,
}

/// One light of a Relight effect.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct RelightLight {
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
    pub color: [f32; 3],
}

impl Default for RelightEffectSettings {
    fn default() -> Self {
        Self {
            relief: RELIEF.default,
            ambient: AMBIENT.default,
            lights: EffectInstances::default(),
        }
    }
}

impl Default for RelightLight {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            source: [SOURCE_X.default, SOURCE_Y.default],
            depth: DEPTH.default,
            reach: REACH.default,
            size: SIZE.default,
            shadows_enabled: false,
            shadows: SHADOWS.default,
            color: COLOR.default,
        }
    }
}

impl RelightLight {
    /// A copy of this light placed across the frame from it, where a second
    /// light usually goes and where its handle does not hide this one's.
    pub fn mirrored(&self) -> Self {
        Self {
            source: mirrored_position(self.source, [SOURCE_X, SOURCE_Y]),
            ..*self
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

    /// Active when a light adds light or the existing light is dimmed.
    fn is_active(&self) -> bool {
        self.lights
            .as_slice()
            .iter()
            .any(|light| light.amount.abs() > 1e-6)
            || self.ambient < AMBIENT.max - 1e-6
    }

    fn instance_count(&self) -> usize {
        self.lights.count()
    }

    fn controls(&mut self) -> EffectControls<'_> {
        let mut shown = Vec::new();
        let mut details = Vec::new();
        for light in self.lights.as_mut_slice() {
            shown.extend([
                EffectControl::Slider(&mut light.amount, AMOUNT),
                EffectControl::Position(&mut light.source, [SOURCE_X, SOURCE_Y]),
                EffectControl::Slider(&mut light.depth, DEPTH),
                EffectControl::Color(&mut light.color, COLOR),
                EffectControl::Toggle(&mut light.shadows_enabled, CAST_SHADOWS),
            ]);
            details.extend([
                EffectControl::Slider(&mut light.reach, REACH),
                EffectControl::Slider(&mut light.size, SIZE),
                EffectControl::Slider(&mut light.shadows, SHADOWS),
            ]);
        }
        shown.push(EffectControl::Slider(&mut self.ambient, AMBIENT));
        details.push(EffectControl::Slider(&mut self.relief, RELIEF));
        EffectControls::new(shown).details("Light details", details)
    }

    fn wants_scene_depth(&self) -> bool {
        true
    }

    /// Each light's slot also holds the shared Relief and Ambient: a light
    /// is placed in its slot's scene frame (`relight_scene_light`).
    fn pack(&self, slot: &mut EffectLanes, context: &PackContext) {
        let Some(light) = self.lights.get(context.instance) else {
            return;
        };
        slot.set(lanes::AMOUNT, AMOUNT.clamp(light.amount));
        slot.set(lanes::REACH, REACH.clamp(light.reach));
        slot.set(lanes::SOURCE_X, SOURCE_X.clamp(light.source[0]));
        slot.set(lanes::SOURCE_Y, SOURCE_Y.clamp(light.source[1]));
        slot.set_color(lanes::COLOR, COLOR.clamp(light.color));
        slot.set(lanes::DEPTH, DEPTH.clamp(light.depth));
        slot.set(lanes::SIZE, SIZE.clamp(light.size));
        // Switched-off shadows keep their strength in the settings only.
        let shadows = if light.shadows_enabled {
            SHADOWS.clamp(light.shadows)
        } else {
            0.0
        };
        slot.set(lanes::SHADOWS, shadows);
        slot.set(lanes::RELIEF, RELIEF.clamp(self.relief));
        slot.set(lanes::AMBIENT, AMBIENT.clamp(self.ambient));
    }
}
