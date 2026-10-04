//! `LocalAdjustments`: the adjustments a mask applies.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct LocalAdjustments {
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub temperature: f32,
    pub tint: f32,
    #[serde(default)]
    pub hue: f32,
    pub saturation: f32,
    pub texture: f32,
    pub clarity: f32,
    pub dehaze: f32,
    #[serde(default)]
    pub halation_amount: f32,
    pub tone_curve: crate::pipeline::PointCurve,
    pub tone_curve_red: crate::pipeline::PointCurve,
    pub tone_curve_green: crate::pipeline::PointCurve,
    pub tone_curve_blue: crate::pipeline::PointCurve,
    pub hsl_hue: [f32; 8],
    pub hsl_saturation: [f32; 8],
    pub hsl_luminance: [f32; 8],
    #[serde(default)]
    pub point_colors: crate::pipeline::PointColors,
    #[serde(skip)]
    pub point_color_visualize: Option<usize>,
    pub color_grading: crate::pipeline::ColorGrading,
}

impl Default for LocalAdjustments {
    fn default() -> Self {
        use effect_params::adjustment;

        Self {
            exposure: adjustment::EXPOSURE.default,
            contrast: adjustment::CONTRAST.default,
            highlights: adjustment::HIGHLIGHTS.default,
            shadows: adjustment::SHADOWS.default,
            whites: adjustment::WHITES.default,
            blacks: adjustment::BLACKS.default,
            temperature: adjustment::TEMPERATURE.default,
            tint: adjustment::TINT.default,
            hue: adjustment::HUE.default,
            saturation: adjustment::SATURATION.default,
            texture: adjustment::TEXTURE.default,
            clarity: adjustment::CLARITY.default,
            dehaze: adjustment::DEHAZE.default,
            halation_amount: adjustment::HALATION.default,
            tone_curve: crate::pipeline::PointCurve::linear(),
            tone_curve_red: crate::pipeline::PointCurve::linear(),
            tone_curve_green: crate::pipeline::PointCurve::linear(),
            tone_curve_blue: crate::pipeline::PointCurve::linear(),
            hsl_hue: [0.0; 8],
            hsl_saturation: [0.0; 8],
            hsl_luminance: [0.0; 8],
            point_colors: crate::pipeline::PointColors::default(),
            point_color_visualize: None,
            color_grading: crate::pipeline::ColorGrading::default(),
        }
    }
}

impl LocalAdjustments {
    pub fn curve_feature_flags(self) -> u32 {
        u32::from(!self.tone_curve.is_identity())
            | (u32::from(!self.tone_curve_red.is_identity()) << 1)
            | (u32::from(!self.tone_curve_green.is_identity()) << 2)
            | (u32::from(!self.tone_curve_blue.is_identity()) << 3)
    }

    pub fn has_color_mixer(self) -> bool {
        self.hsl_hue
            .iter()
            .chain(&self.hsl_saturation)
            .chain(&self.hsl_luminance)
            .any(|value| value.abs() > 1e-6)
    }

    pub fn has_color_grading(self) -> bool {
        !self.color_grading.is_neutral()
    }

    pub fn is_neutral(self) -> bool {
        let mut normalized = self;
        if normalized.color_grading.is_neutral() {
            normalized.color_grading = crate::pipeline::ColorGrading::default();
        }
        if !normalized.point_colors.has_adjustments() {
            normalized.point_colors.clear();
        }
        normalized.point_color_visualize = None;
        normalized == Self::default()
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn sanitize_tone_curves(&mut self) {
        self.tone_curve.sanitize();
        self.tone_curve_red.sanitize();
        self.tone_curve_green.sanitize();
        self.tone_curve_blue.sanitize();
    }
}
