//! `SensorSaturation`: where photosites saturate relative to the white level.

/// The level at which a photo's photosites saturate, in normalized sensor units:
/// `(code − black) / (white − black)` with the [`LoadedRaw`] black and white levels.
///
/// It is 1 when the white level is the saturation point. A white level can instead be a
/// conservative linearity limit, such as the one LibRaw reads from Canon metadata, while the
/// sensor keeps responding linearly up to a higher code. Normalization keeps the white level, so
/// a photo's exposure does not depend on this level; highlight reconstruction scales its clip
/// threshold by it, so measured samples between the two are not reconstructed as clipped.
///
/// [`LoadedRaw`]: super::LoadedRaw
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SensorSaturation(f32);

impl SensorSaturation {
    /// Photosites saturate at the white level.
    pub const AT_WHITE: Self = Self(1.0);

    /// Saturation at `level` normalized units. Levels that are not finite or lie below the
    /// white level are taken as the white level.
    pub fn new(level: f32) -> Self {
        if level.is_finite() && level > 1.0 {
            Self(level)
        } else {
            Self::AT_WHITE
        }
    }

    pub fn level(self) -> f32 {
        self.0
    }

    /// The normalized sensor level highlight reconstruction treats as clipped for
    /// `highlight_clip`, the Develop setting given as a fraction of saturation.
    pub fn clip_level(self, highlight_clip: f32) -> f32 {
        highlight_clip * self.0
    }
}

impl Default for SensorSaturation {
    fn default() -> Self {
        Self::AT_WHITE
    }
}

#[cfg(test)]
mod tests {
    use super::SensorSaturation;

    #[test]
    fn levels_at_or_below_white_and_invalid_levels_mean_saturation_at_white() {
        for level in [1.0, 0.8, 0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(SensorSaturation::new(level), SensorSaturation::AT_WHITE);
        }
        assert_eq!(SensorSaturation::default(), SensorSaturation::AT_WHITE);
    }

    #[test]
    fn clip_level_scales_the_highlight_clip_by_saturation() {
        assert_eq!(SensorSaturation::AT_WHITE.clip_level(0.9), 0.9);
        let saturation = SensorSaturation::new(1.25);
        assert_eq!(saturation.level(), 1.25);
        assert_eq!(saturation.clip_level(0.8), 1.0);
    }
}
