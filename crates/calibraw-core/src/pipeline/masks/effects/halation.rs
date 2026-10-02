use super::params::halation::*;

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct HalationEffectSettings {
    pub amount: f32,
    pub radius: f32,
    pub threshold: f32,
    pub warmth: f32,
}

impl Default for HalationEffectSettings {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            radius: RADIUS.default,
            threshold: THRESHOLD.default,
            warmth: WARMTH.default,
        }
    }
}

impl HalationEffectSettings {
    pub fn is_active(&self) -> bool {
        self.amount > 1e-6 && self.radius > 1e-6
    }
}
