use super::params::grain::*;

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct GrainEffectSettings {
    pub amount: f32,
    pub size: f32,
    pub roughness: f32,
    pub color: f32,
    pub seed: f32,
}

impl Default for GrainEffectSettings {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            size: SIZE.default,
            roughness: ROUGHNESS.default,
            color: COLOR.default,
            seed: SEED.default,
        }
    }
}

impl GrainEffectSettings {
    pub fn is_active(&self) -> bool {
        self.amount > 1e-6
    }
}
