use super::params::smoke::*;

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
    /// How strongly scene lights (Relight, Light Rays) glow in it. Edits
    /// saved before it existed load with 0, so they render as before.
    #[serde(default = "no_light_glow")]
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
            light_glow: LIGHT_GLOW.default,
            image_lights: false,
        }
    }
}

fn no_light_glow() -> f32 {
    0.0
}

impl SmokeEffectSettings {
    pub fn is_active(&self) -> bool {
        self.amount.abs() > 1e-6 && self.density > 1e-6
    }
}
