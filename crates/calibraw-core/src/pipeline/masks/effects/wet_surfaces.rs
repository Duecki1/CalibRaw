use super::lanes::{effect_lanes, EffectLanes, LaneName, PackContext};
use super::params::wet_surfaces::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct WetSurfacesEffectSettings {
    pub wetness: f32,
    pub puddles: f32,
    pub puddle_size: f32,
    pub gloss: f32,
    pub reflections: f32,
    /// How strongly a Rain effect in the same edit disturbs the puddles.
    pub ripples: f32,
    pub seed: f32,
}

impl Default for WetSurfacesEffectSettings {
    fn default() -> Self {
        Self {
            wetness: WETNESS.default,
            puddles: PUDDLES.default,
            puddle_size: PUDDLE_SIZE.default,
            gloss: GLOSS.default,
            reflections: REFLECTIONS.default,
            ripples: RIPPLES.default,
            seed: SEED.default,
        }
    }
}

effect_lanes! {
    WETNESS = 0,
    PUDDLES = 1,
    PUDDLE_SIZE = 2,
    GLOSS = 3,
    REFLECTIONS = 4,
    RIPPLES = 5,
    SEED = 6,
}

impl EffectSettings for WetSurfacesEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    fn is_active(&self) -> bool {
        self.wetness > 1e-6 || self.puddles > 1e-6
    }

    fn controls(&mut self) -> EffectControls<'_> {
        EffectControls::new(vec![
            EffectControl::Slider(&mut self.wetness, WETNESS),
            EffectControl::Slider(&mut self.puddles, PUDDLES),
            EffectControl::Slider(&mut self.reflections, REFLECTIONS),
            EffectControl::Slider(&mut self.gloss, GLOSS),
        ])
        .details(
            "Water details",
            vec![
                EffectControl::Slider(&mut self.puddle_size, PUDDLE_SIZE),
                EffectControl::Slider(&mut self.ripples, RIPPLES),
                EffectControl::Seed(&mut self.seed, SEED),
            ],
        )
    }

    fn wants_scene_depth(&self) -> bool {
        true
    }

    fn pack(&self, slot: &mut EffectLanes, _: &PackContext) {
        slot.set(lanes::WETNESS, WETNESS.clamp(self.wetness));
        slot.set(lanes::PUDDLES, PUDDLES.clamp(self.puddles));
        slot.set(lanes::PUDDLE_SIZE, PUDDLE_SIZE.clamp(self.puddle_size));
        slot.set(lanes::GLOSS, GLOSS.clamp(self.gloss));
        slot.set(lanes::REFLECTIONS, REFLECTIONS.clamp(self.reflections));
        slot.set(lanes::RIPPLES, RIPPLES.clamp(self.ripples));
        slot.set(lanes::SEED, SEED.clamp(self.seed));
    }
}
