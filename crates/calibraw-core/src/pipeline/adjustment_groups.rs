//! Resets scoped to a single adjustment card.
use super::{ExposureParams, LocalAdjustments};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdjustmentGroup {
    Light,
    ToneCurve,
    Color,
    ColorGrading,
    Detail,
    Effects,
    ColorMixer,
}

impl ExposureParams {
    pub fn reset_group(&mut self, group: AdjustmentGroup) {
        let defaults = Self::default();
        match group {
            AdjustmentGroup::Light => {
                self.exposure = defaults.exposure;
                self.contrast = defaults.contrast;
                self.highlights = defaults.highlights;
                self.shadows = defaults.shadows;
                self.whites = defaults.whites;
                self.blacks = defaults.blacks;
            }
            AdjustmentGroup::ToneCurve => {
                self.tone_curve = defaults.tone_curve;
                self.tone_curve_red = defaults.tone_curve_red;
                self.tone_curve_green = defaults.tone_curve_green;
                self.tone_curve_blue = defaults.tone_curve_blue;
            }
            AdjustmentGroup::Color => {
                self.temperature = defaults.temperature;
                self.tint = defaults.tint;
                self.hue = defaults.hue;
                self.saturation = defaults.saturation;
                self.vibrance = defaults.vibrance;
            }
            AdjustmentGroup::ColorGrading => {
                self.color_grading = defaults.color_grading;
            }
            AdjustmentGroup::Detail => {
                self.luminance_denoise = defaults.luminance_denoise;
                self.chroma_denoise = defaults.chroma_denoise;
                self.denoise_detail = defaults.denoise_detail;
                self.denoise_quality = defaults.denoise_quality;
                self.ai_denoise_enabled = defaults.ai_denoise_enabled;
                self.sharpen_amount = defaults.sharpen_amount;
                self.sharpen_radius = defaults.sharpen_radius;
                self.sharpen_detail = defaults.sharpen_detail;
                self.sharpen_masking = defaults.sharpen_masking;
            }
            AdjustmentGroup::Effects => {
                self.texture = defaults.texture;
                self.clarity = defaults.clarity;
                self.dehaze = defaults.dehaze;
                self.halation_amount = defaults.halation_amount;
                self.grain_amount = defaults.grain_amount;
                self.glow_amount = defaults.glow_amount;
                self.glow_radius = defaults.glow_radius;
                self.glow_threshold = defaults.glow_threshold;
                self.vignette_amount = defaults.vignette_amount;
                self.vignette_midpoint = defaults.vignette_midpoint;
                self.vignette_roundness = defaults.vignette_roundness;
                self.vignette_feather = defaults.vignette_feather;
                self.vignette_highlights = defaults.vignette_highlights;
            }
            AdjustmentGroup::ColorMixer => {
                self.hsl_hue = defaults.hsl_hue;
                self.hsl_saturation = defaults.hsl_saturation;
                self.hsl_luminance = defaults.hsl_luminance;
                self.point_colors = defaults.point_colors;
            }
        }
    }
}

impl LocalAdjustments {
    pub fn reset_group(&mut self, group: AdjustmentGroup) {
        let defaults = Self::default();
        match group {
            AdjustmentGroup::Light => {
                self.exposure = defaults.exposure;
                self.contrast = defaults.contrast;
                self.highlights = defaults.highlights;
                self.shadows = defaults.shadows;
                self.whites = defaults.whites;
                self.blacks = defaults.blacks;
            }
            AdjustmentGroup::ToneCurve => {
                self.tone_curve = defaults.tone_curve;
                self.tone_curve_red = defaults.tone_curve_red;
                self.tone_curve_green = defaults.tone_curve_green;
                self.tone_curve_blue = defaults.tone_curve_blue;
            }
            AdjustmentGroup::Color => {
                self.temperature = defaults.temperature;
                self.tint = defaults.tint;
                self.hue = defaults.hue;
                self.saturation = defaults.saturation;
            }
            AdjustmentGroup::ColorGrading => {
                self.color_grading = defaults.color_grading;
            }
            AdjustmentGroup::Detail => {}
            AdjustmentGroup::Effects => {
                self.texture = defaults.texture;
                self.clarity = defaults.clarity;
                self.dehaze = defaults.dehaze;
                self.halation_amount = defaults.halation_amount;
            }
            AdjustmentGroup::ColorMixer => {
                self.hsl_hue = defaults.hsl_hue;
                self.hsl_saturation = defaults.hsl_saturation;
                self.hsl_luminance = defaults.hsl_luminance;
                self.point_colors = defaults.point_colors;
                self.point_color_visualize = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::PointColor;
    use serde_json::Value;

    fn edited(value: &mut Value) {
        match value {
            Value::Number(number) if number.is_f64() => *value = Value::from(0.23),
            Value::Bool(value) => *value = !*value,
            Value::Array(values) => values.iter_mut().for_each(edited),
            Value::Object(values) => values.values_mut().for_each(edited),
            _ => {}
        }
    }

    #[test]
    fn every_card_reset_preserves_every_unrelated_field() {
        let groups = [
            (AdjustmentGroup::Light, "exposure contrast highlights shadows whites blacks"),
            (AdjustmentGroup::ToneCurve, "tone_curve tone_curve_red tone_curve_green tone_curve_blue"),
            (AdjustmentGroup::Color, "temperature tint hue saturation vibrance"),
            (AdjustmentGroup::ColorGrading, "color_grading"),
            (AdjustmentGroup::Detail, "luminance_denoise chroma_denoise denoise_detail denoise_quality ai_denoise_enabled sharpen_amount sharpen_radius sharpen_detail sharpen_masking"),
            (AdjustmentGroup::Effects, "texture clarity dehaze halation_amount grain_amount glow_amount glow_radius glow_threshold vignette_amount vignette_midpoint vignette_roundness vignette_feather vignette_highlights"),
            (AdjustmentGroup::ColorMixer, "hsl_hue hsl_saturation hsl_luminance point_colors"),
        ];
        for local in [false, true] {
            let defaults = if local {
                serde_json::to_value(LocalAdjustments::default()).unwrap()
            } else {
                serde_json::to_value(ExposureParams::default()).unwrap()
            };
            let mut original = defaults.clone();
            edited(&mut original);
            original = if local {
                serde_json::to_value(serde_json::from_value::<LocalAdjustments>(original).unwrap())
                    .unwrap()
            } else {
                serde_json::to_value(serde_json::from_value::<ExposureParams>(original).unwrap())
                    .unwrap()
            };
            for (group, fields) in groups {
                let after = if local {
                    let mut edits: LocalAdjustments =
                        serde_json::from_value(original.clone()).unwrap();
                    edits.reset_group(group);
                    serde_json::to_value(edits).unwrap()
                } else {
                    let mut edits: ExposureParams =
                        serde_json::from_value(original.clone()).unwrap();
                    edits.reset_group(group);
                    serde_json::to_value(edits).unwrap()
                };
                for (field, value) in original.as_object().unwrap() {
                    let expected = if fields.split_whitespace().any(|name| name == field) {
                        &defaults[field]
                    } else {
                        value
                    };
                    assert_eq!(
                        &after[field], expected,
                        "local={local}, card={group:?}, field={field}"
                    );
                }
            }
        }
    }

    #[test]
    fn local_point_colors_round_trip_and_old_masks_default_to_empty() {
        let mut edits = LocalAdjustments::default();
        edits
            .point_colors
            .push(PointColor::from_srgb([0.8, 0.2, 0.1]));
        assert!(edits.is_neutral());
        edits.point_colors[0].hue_shift = 20.0;
        assert!(!edits.is_neutral());
        edits.point_color_visualize = Some(0);
        let saved = serde_json::to_value(edits).unwrap();
        assert!(saved.get("point_color_visualize").is_none());
        let restored: LocalAdjustments = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(restored.point_colors, edits.point_colors);
        assert_eq!(restored.point_color_visualize, None);

        let mut old = saved;
        old.as_object_mut().unwrap().remove("point_colors");
        let restored: LocalAdjustments = serde_json::from_value(old).unwrap();
        assert!(restored.point_colors.is_empty());

        edits.reset_group(AdjustmentGroup::ColorMixer);
        assert!(edits.point_colors.is_empty());
        assert_eq!(edits.point_color_visualize, None);
    }
}
