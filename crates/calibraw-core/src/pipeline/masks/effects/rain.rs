use super::params::rain::*;

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct RainEffectSettings {
    /// Whether streaks and mist follow scene depth. Without it rain falls in
    /// front of the whole photo and the mist is an even veil.
    pub depth_enabled: bool,
    pub amount: f32,
    pub length: f32,
    pub angle: f32,
    pub drop_size: f32,
    pub mist: f32,
    /// How strongly scene lights (Relight, Light Rays) light the drops and the
    /// mist.
    pub light_glow: f32,
    /// Whether light sources in the photograph also light the drops and the
    /// mist, in their own colours, at the Light glow strength.
    pub image_lights: bool,
    pub gusts: f32,
    pub perspective: f32,
    pub focus: f32,
    pub seed: f32,
}

impl Default for RainEffectSettings {
    fn default() -> Self {
        Self {
            depth_enabled: true,
            amount: AMOUNT.default,
            length: LENGTH.default,
            angle: ANGLE.default,
            drop_size: DROP_SIZE.default,
            mist: MIST.default,
            light_glow: LIGHT_GLOW.default,
            image_lights: true,
            gusts: GUSTS.default,
            perspective: PERSPECTIVE.default,
            focus: FOCUS.default,
            seed: SEED.default,
        }
    }
}

impl RainEffectSettings {
    pub fn is_active(&self) -> bool {
        self.amount > 1e-6
    }
}
