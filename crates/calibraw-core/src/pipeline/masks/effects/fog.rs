use super::params::fog::*;

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
    pub start: f32,
    pub depth_influence: f32,
    pub color: [f32; 3],
    /// How strongly scene lights (Relight, Light Rays) glow in it. The
    /// serialized baseline is 0, so edits saved before it existed, including
    /// untouched ones that omit their settings, render as before; new effects
    /// start at the parameter default (`InitialEffectSettings`).
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
            start: START.default,
            depth_influence: DEPTH_INFLUENCE.default,
            color: COLOR.default,
            light_glow: 0.0,
            image_lights: false,
        }
    }
}

impl FogEffectSettings {
    pub fn is_active(&self) -> bool {
        self.amount.abs() > 1e-6 && self.density > 1e-6
    }
}

/// New Fog glows around scene lights; edits saved before Light glow keep 0.
impl super::InitialEffectSettings for FogEffectSettings {
    fn initial() -> Self {
        Self {
            light_glow: LIGHT_GLOW.default,
            ..Self::default()
        }
    }
}
