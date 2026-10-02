use super::params::vignette::*;

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct VignetteEffectSettings {
    pub amount: f32,
    pub midpoint: f32,
    pub roundness: f32,
    pub feather: f32,
    pub highlights: f32,
    /// Center percentages within the cropped and rotated frame, not an export tile.
    pub center: [f32; 2],
}

impl Default for VignetteEffectSettings {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            midpoint: MIDPOINT.default,
            roundness: ROUNDNESS.default,
            feather: FEATHER.default,
            highlights: HIGHLIGHTS.default,
            center: [CENTER_X.default, CENTER_Y.default],
        }
    }
}

impl VignetteEffectSettings {
    pub fn is_active(&self) -> bool {
        self.amount.abs() > 1e-6
    }
}
