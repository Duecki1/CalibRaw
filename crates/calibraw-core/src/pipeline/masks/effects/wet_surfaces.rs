use super::params::wet_surfaces::*;

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

impl WetSurfacesEffectSettings {
    pub fn is_active(&self) -> bool {
        self.wetness > 1e-6 || self.puddles > 1e-6
    }
}
