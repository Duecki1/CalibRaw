use super::lanes::{effect_lanes, options, EffectLanes, LaneName, PackContext};
use super::params::smoke::*;
use super::params::IMAGE_LIGHTS;
use super::settings::{EffectControl, EffectControls, EffectSettings};

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct SmokeEffectSettings {
    pub amount: f32,
    pub density: f32,
    pub scale: f32,
    pub turbulence: f32,
    pub softness: f32,
    pub angle: f32,
    pub seed: f32,
    pub color: [f32; 3],
    /// How strongly scene lights (Relight, Light Rays, Glow, Neon) glow in it. The
    /// serialized baseline is 0, so edits saved before it existed, including
    /// untouched ones that omit their settings, render as before; new effects
    /// start at the parameter default (`EffectSettings::initial`).
    pub light_glow: f32,
    /// Whether light sources in the photograph (lamps, lit windows) also glow
    /// in the smoke, in their own colours, at the Light glow strength.
    pub image_lights: bool,
}

impl Default for SmokeEffectSettings {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            density: DENSITY.default,
            scale: SCALE.default,
            turbulence: TURBULENCE.default,
            softness: SOFTNESS.default,
            angle: ANGLE.default,
            seed: SEED.default,
            color: COLOR.default,
            light_glow: 0.0,
            image_lights: false,
        }
    }
}

effect_lanes! {
    AMOUNT = 0,
    DENSITY = 1,
    SCALE = 2,
    TURBULENCE = 3,
    COLOR = 4 (color),
    ANGLE = 7,
    SOFTNESS = 8,
    SEED = 9,
    LIGHT_GLOW = 10,
}

impl EffectSettings for SmokeEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    /// New Smoke glows around scene lights; edits saved before Light glow keep 0.
    fn initial() -> Self {
        Self {
            light_glow: LIGHT_GLOW.default,
            ..Self::default()
        }
    }

    fn is_active(&self) -> bool {
        self.amount.abs() > 1e-6 && self.density > 1e-6
    }

    fn controls(&mut self) -> EffectControls<'_> {
        EffectControls::new(vec![
            EffectControl::Slider(&mut self.amount, AMOUNT),
            EffectControl::Slider(&mut self.density, DENSITY),
            EffectControl::Slider(&mut self.scale, SCALE),
            EffectControl::Color(&mut self.color, COLOR),
            EffectControl::Slider(&mut self.light_glow, LIGHT_GLOW),
            EffectControl::Toggle(&mut self.image_lights, IMAGE_LIGHTS),
        ])
        .details(
            "Texture details",
            vec![
                EffectControl::Slider(&mut self.turbulence, TURBULENCE),
                EffectControl::Slider(&mut self.softness, SOFTNESS),
                EffectControl::Angle(&mut self.angle, ANGLE),
                EffectControl::Seed(&mut self.seed, SEED),
            ],
        )
    }

    fn pack(&self, slot: &mut EffectLanes, _: &PackContext) {
        slot.set(lanes::AMOUNT, AMOUNT.clamp(self.amount));
        slot.set(lanes::DENSITY, DENSITY.clamp(self.density));
        slot.set(lanes::SCALE, SCALE.clamp(self.scale));
        slot.set(lanes::TURBULENCE, TURBULENCE.clamp(self.turbulence));
        slot.set_color(lanes::COLOR, COLOR.clamp(self.color));
        slot.set(lanes::ANGLE, ANGLE.clamp(self.angle));
        slot.set(lanes::SOFTNESS, SOFTNESS.clamp(self.softness));
        slot.set(lanes::SEED, SEED.clamp(self.seed));
        slot.set(lanes::LIGHT_GLOW, LIGHT_GLOW.clamp(self.light_glow));
        slot.set_switch(options::MEDIUM_IMAGE_LIGHTS, self.image_lights);
    }
}
