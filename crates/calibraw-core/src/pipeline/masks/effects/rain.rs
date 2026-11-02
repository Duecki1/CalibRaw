use super::params::rain::*;

/// A typical drop's streak at the nearest surface, in shorter edges, for an
/// exposure of [`REFERENCE_SHUTTER_SECONDS`].
const REFERENCE_LENGTH: f32 = 0.04;
/// Also the exposure of a photo that does not record one.
const REFERENCE_SHUTTER_SECONDS: f32 = 1.0 / 125.0;
/// How streak length grows with exposure. A drop falls in proportion to it, but a
/// photo does not record its scale, and a streak as short as a fast shutter would
/// make it reads as snow.
const SHUTTER_EXPONENT: f32 = 0.5;
/// Stops of length from Streak length 0 to either end of the control.
const LENGTH_STOPS: f32 = 4.0;
/// Shortest and longest streak at the nearest surface, in shorter edges: a drop
/// frozen into a short dash, and one that crosses the frame.
const LENGTH_LIMITS: [f32; 2] = [0.0012, 0.6];

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct RainEffectSettings {
    /// Whether streaks and mist follow scene depth. Without it rain falls in
    /// front of the whole photo and the mist is an even veil.
    pub depth_enabled: bool,
    pub amount: f32,
    /// How much longer or shorter streaks are than the photo's shutter speed
    /// gives (`streak_length`).
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
    /// Crowns on the ground. They need the ground found in scene depth.
    pub splashes: f32,
    /// Where the lens is focused without scene depth. With it the blur at each
    /// distance is measured in the photo.
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
            splashes: SPLASHES.default,
            focus: FOCUS.default,
            seed: SEED.default,
        }
    }
}

impl RainEffectSettings {
    pub fn is_active(&self) -> bool {
        self.amount > 1e-6
    }

    /// A typical drop's streak during an exposure of `shutter_seconds`, at the
    /// nearest surface, in shorter edges. A shutter speed of 0 means the photo
    /// records none.
    pub fn streak_length(&self, shutter_seconds: f32) -> f32 {
        let seconds = if shutter_seconds > 0.0 {
            shutter_seconds
        } else {
            REFERENCE_SHUTTER_SECONDS
        };
        let stops = LENGTH.clamp(self.length) / LENGTH.max * LENGTH_STOPS;
        let length = REFERENCE_LENGTH
            * (seconds / REFERENCE_SHUTTER_SECONDS).powf(SHUTTER_EXPONENT)
            * stops.exp2();
        length.clamp(LENGTH_LIMITS[0], LENGTH_LIMITS[1])
    }
}
