use super::params::relight::*;

/// A virtual point light that shades the scene reconstructed from scene depth.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct RelightEffectSettings {
    pub amount: f32,
    /// Percentages of the full, uncropped image, as for Light Rays.
    pub source: [f32; 2],
    /// Light depth: 0 is level with the nearest surface, 100 with the farthest;
    /// negative values move the light toward the camera (-100 is at the camera).
    pub depth: f32,
    pub reach: f32,
    pub size: f32,
    /// Whether nearer objects cast shadows; off keeps the Shadows strength.
    pub shadows_enabled: bool,
    pub shadows: f32,
    pub relief: f32,
    pub ambient: f32,
    pub color: [f32; 3],
}

impl Default for RelightEffectSettings {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            source: [SOURCE_X.default, SOURCE_Y.default],
            depth: DEPTH.default,
            reach: REACH.default,
            size: SIZE.default,
            shadows_enabled: true,
            shadows: SHADOWS.default,
            relief: RELIEF.default,
            ambient: AMBIENT.default,
            color: COLOR.default,
        }
    }
}

impl RelightEffectSettings {
    /// Active when it adds light or dims the existing light.
    pub fn is_active(&self) -> bool {
        self.amount.abs() > 1e-6 || self.ambient < AMBIENT.max - 1e-6
    }
}
