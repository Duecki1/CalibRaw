use super::params::light_beams::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum LightBeamPreset {
    #[default]
    Headlight,
    Flashlight,
    Spotlight,
    Streetlamp,
}

impl LightBeamPreset {
    pub const ALL: [Self; 4] = [
        Self::Headlight,
        Self::Flashlight,
        Self::Spotlight,
        Self::Streetlamp,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Headlight => "Headlight",
            Self::Flashlight => "Flashlight",
            Self::Spotlight => "Spotlight",
            Self::Streetlamp => "Streetlamp",
        }
    }
}

/// Twelve shader parameters plus the selected preset, which is editor metadata.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct LightBeamsEffectSettings {
    pub preset: LightBeamPreset,
    pub amount: f32,
    /// Beam reach as a percentage of the full image height.
    pub length: f32,
    /// Source position as percentages of full image width and height.
    pub source: [f32; 2],
    /// Clockwise degrees in image coordinates: 0 points right, 90 points down.
    pub direction: f32,
    /// Full cone angle in degrees.
    pub spread: f32,
    pub softness: f32,
    /// Relative scene distance, from 0 (near) to 100 (far).
    pub source_depth: f32,
    /// Strength of the broad isotropic light pool around the source.
    pub scattering: f32,
    pub color: [f32; 3],
}

impl Default for LightBeamsEffectSettings {
    fn default() -> Self {
        Self {
            preset: LightBeamPreset::default(),
            amount: AMOUNT.default,
            length: LENGTH.default,
            source: [SOURCE_X.default, SOURCE_Y.default],
            direction: DIRECTION.default,
            spread: SPREAD.default,
            softness: SOFTNESS.default,
            source_depth: SOURCE_DEPTH.default,
            scattering: SCATTERING.default,
            color: COLOR.default,
        }
    }
}

impl LightBeamsEffectSettings {
    pub fn from_preset(preset: LightBeamPreset) -> Self {
        let defaults = Self::default();
        match preset {
            LightBeamPreset::Headlight => defaults,
            LightBeamPreset::Flashlight => Self {
                preset,
                length: 115.0,
                spread: 14.0,
                softness: 50.0,
                source_depth: 30.0,
                scattering: 4.0,
                color: [0.95, 0.97, 1.0],
                ..defaults
            },
            LightBeamPreset::Spotlight => Self {
                preset,
                amount: 75.0,
                length: 140.0,
                source: [50.0, 10.0],
                direction: 60.0,
                spread: 20.0,
                softness: 40.0,
                source_depth: 55.0,
                color: [1.0, 0.95, 0.88],
                ..defaults
            },
            LightBeamPreset::Streetlamp => Self {
                preset,
                amount: 70.0,
                length: 65.0,
                source: [50.0, 20.0],
                direction: 90.0,
                spread: 120.0,
                softness: 80.0,
                source_depth: 45.0,
                scattering: 80.0,
                color: [1.0, 0.72, 0.38],
                ..defaults
            },
        }
    }

    pub fn is_active(&self) -> bool {
        self.amount.abs() > 1e-6 && self.length > 1e-6
    }
}
