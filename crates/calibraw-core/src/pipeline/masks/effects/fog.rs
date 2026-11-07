use super::lanes::{effect_lanes, options, EffectLanes, LaneName, PackContext};
use super::params::fog::*;
use super::params::IMAGE_LIGHTS;
use super::settings::{EffectControl, EffectControls, EffectSettings};

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct FogEffectSettings {
    pub depth_enabled: bool,
    pub amount: f32,
    pub density: f32,
    pub scale: f32,
    pub softness: f32,
    pub variation: f32,
    pub seed: f32,
    /// Where fog begins, in control depth (`migrations::scene_depth_controls`
    /// converts the linear `start` older edits saved).
    pub start_depth: f32,
    pub depth_influence: f32,
    pub color: [f32; 3],
    /// How strongly scene lights (Relight, Light Rays, Glow, Neon) glow in it. The
    /// serialized baseline is 0, so edits saved before it existed, including
    /// untouched ones that omit their settings, render as before; new effects
    /// start at the parameter default (`EffectSettings::initial`).
    pub light_glow: f32,
    /// Whether light sources in the photograph (lamps, lit windows) also glow
    /// in the fog, in their own colours, at the Light glow strength.
    pub image_lights: bool,
}

impl Default for FogEffectSettings {
    fn default() -> Self {
        Self {
            depth_enabled: true,
            amount: AMOUNT.default,
            density: DENSITY.default,
            scale: SCALE.default,
            softness: SOFTNESS.default,
            variation: VARIATION.default,
            seed: SEED.default,
            start_depth: START.default,
            depth_influence: DEPTH_INFLUENCE.default,
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
    SOFTNESS = 3,
    COLOR = 4 (color),
    VARIATION = 7,
    SEED = 8,
    START = 9,
    DEPTH_INFLUENCE = 10,
    LIGHT_GLOW = 11,
}

impl EffectSettings for FogEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    /// New Fog glows around scene lights; edits saved before Light glow keep 0.
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
            EffectControl::Toggle(&mut self.depth_enabled, SCENE_DEPTH),
            EffectControl::Slider(&mut self.start_depth, START),
            EffectControl::Color(&mut self.color, COLOR),
            EffectControl::Slider(&mut self.light_glow, LIGHT_GLOW),
            EffectControl::Toggle(&mut self.image_lights, IMAGE_LIGHTS),
        ])
        .details(
            "Atmosphere details",
            vec![
                EffectControl::Slider(&mut self.depth_influence, DEPTH_INFLUENCE),
                EffectControl::Slider(&mut self.softness, SOFTNESS),
                EffectControl::Slider(&mut self.variation, VARIATION),
                EffectControl::Slider(&mut self.scale, SCALE),
                EffectControl::Seed(&mut self.seed, SEED),
            ],
        )
    }

    fn wants_scene_depth(&self) -> bool {
        self.depth_enabled
    }

    fn pack(&self, slot: &mut EffectLanes, _: &PackContext) {
        slot.set(lanes::AMOUNT, AMOUNT.clamp(self.amount));
        slot.set(lanes::DENSITY, DENSITY.clamp(self.density));
        slot.set(lanes::SCALE, SCALE.clamp(self.scale));
        slot.set(lanes::SOFTNESS, SOFTNESS.clamp(self.softness));
        slot.set_color(lanes::COLOR, COLOR.clamp(self.color));
        slot.set(lanes::VARIATION, VARIATION.clamp(self.variation));
        slot.set(lanes::SEED, SEED.clamp(self.seed));
        // Without scene depth the fog is an even veil from the camera on.
        if self.depth_enabled {
            slot.set(lanes::START, START.clamp(self.start_depth));
            slot.set(
                lanes::DEPTH_INFLUENCE,
                DEPTH_INFLUENCE.clamp(self.depth_influence),
            );
        }
        slot.set(lanes::LIGHT_GLOW, LIGHT_GLOW.clamp(self.light_glow));
        slot.set_switch(options::MEDIUM_IMAGE_LIGHTS, self.image_lights);
    }
}
