use super::params::sharpen::*;

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct SharpenEffectSettings {
    pub amount: f32,
    pub radius: f32,
    pub detail: f32,
    pub masking: f32,
}

impl Default for SharpenEffectSettings {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            radius: RADIUS.default,
            detail: DETAIL.default,
            masking: MASKING.default,
        }
    }
}

impl SharpenEffectSettings {
    pub fn is_active(&self) -> bool {
        self.amount > 1e-6
    }
}
