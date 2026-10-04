//! Develop settings grouped by the sidebar card that owns them. Card resets,
//! adjustment paste and presets all move settings one group at a time.
use super::{ExposureParams, LocalAdjustments};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdjustmentGroup {
    Light,
    ToneCurve,
    Color,
    ColorGrading,
    Detail,
    Effects,
    ColorMixer,
}

impl AdjustmentGroup {
    pub const ALL: [Self; 7] = [
        Self::Light,
        Self::ToneCurve,
        Self::Color,
        Self::ColorGrading,
        Self::Detail,
        Self::Effects,
        Self::ColorMixer,
    ];

    /// The title of the sidebar card that owns this group.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::ToneCurve => "Tone Curve",
            Self::Color => "Color",
            Self::ColorGrading => "Color Grading",
            Self::Detail => "Detail",
            Self::Effects => "Effects",
            Self::ColorMixer => "Color Mixer",
        }
    }

    const fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// A set of [`AdjustmentGroup`]s. Serialized as a list of group names so files
/// stay readable and independent of the bit layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct AdjustmentGroupSet(u8);

impl AdjustmentGroupSet {
    pub const EMPTY: Self = Self(0);
    pub const ALL: Self = {
        let mut bits = 0;
        let mut index = 0;
        while index < AdjustmentGroup::ALL.len() {
            bits |= AdjustmentGroup::ALL[index].bit();
            index += 1;
        }
        Self(bits)
    };

    pub const fn contains(self, group: AdjustmentGroup) -> bool {
        self.0 & group.bit() != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn set(&mut self, group: AdjustmentGroup, included: bool) {
        if included {
            self.0 |= group.bit();
        } else {
            self.0 &= !group.bit();
        }
    }

    pub fn iter(self) -> impl Iterator<Item = AdjustmentGroup> {
        AdjustmentGroup::ALL
            .into_iter()
            .filter(move |group| self.contains(*group))
    }
}

impl FromIterator<AdjustmentGroup> for AdjustmentGroupSet {
    fn from_iter<I: IntoIterator<Item = AdjustmentGroup>>(groups: I) -> Self {
        let mut set = Self::EMPTY;
        for group in groups {
            set.set(group, true);
        }
        set
    }
}

impl Serialize for AdjustmentGroupSet {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}

impl<'de> Deserialize<'de> for AdjustmentGroupSet {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<AdjustmentGroup>::deserialize(deserializer).map(|groups| groups.into_iter().collect())
    }
}

/// Which part of [`ExposureParams`] a copy covers: one sidebar card, or the
/// RAW decoding settings that no card owns.
#[derive(Clone, Copy)]
enum ExposureSection {
    Group(AdjustmentGroup),
    RawProcessing,
}

impl ExposureParams {
    pub fn reset_group(&mut self, group: AdjustmentGroup) {
        self.copy_group_from(&Self::default(), group);
    }

    /// Copies the settings owned by one sidebar card from `source`.
    pub fn copy_group_from(&mut self, source: &Self, group: AdjustmentGroup) {
        self.copy_section_from(source, ExposureSection::Group(group));
    }

    /// Copies the RAW decoding settings that no sidebar card owns: black point,
    /// tone mapper, demosaic, chromatic aberration and highlight reconstruction.
    pub fn copy_raw_processing_from(&mut self, source: &Self) {
        self.copy_section_from(source, ExposureSection::RawProcessing);
    }

    fn copy_section_from(&mut self, source: &Self, section: ExposureSection) {
        // Destructured without `..`: a new field does not compile until it is
        // assigned to exactly one section below.
        let Self {
            black_point,
            exposure,
            contrast,
            sigmoid,
            temperature,
            tint,
            hue,
            saturation,
            vibrance,
            tone_curve,
            tone_curve_red,
            tone_curve_green,
            tone_curve_blue,
            chroma_denoise,
            luminance_denoise,
            denoise_detail,
            denoise_quality,
            ai_denoise_enabled,
            demosaic_mode,
            dual_threshold,
            frequency_chroma,
            ca_red,
            ca_blue,
            highlight_method,
            highlight_clip,
            highlight_reconstruction,
            highlights,
            shadows,
            whites,
            blacks,
            texture,
            clarity,
            dehaze,
            sharpen_amount,
            sharpen_radius,
            sharpen_detail,
            sharpen_masking,
            halation_amount,
            grain_amount,
            glow_amount,
            glow_radius,
            glow_threshold,
            vignette_amount,
            vignette_midpoint,
            vignette_roundness,
            vignette_feather,
            vignette_highlights,
            hsl_hue,
            hsl_saturation,
            hsl_luminance,
            point_colors,
            // Transient picker state of the image being edited, never copied.
            point_color_visualize: _,
            color_grading,
        } = *source;

        match section {
            ExposureSection::Group(AdjustmentGroup::Light) => {
                self.exposure = exposure;
                self.contrast = contrast;
                self.highlights = highlights;
                self.shadows = shadows;
                self.whites = whites;
                self.blacks = blacks;
            }
            ExposureSection::Group(AdjustmentGroup::ToneCurve) => {
                self.tone_curve = tone_curve;
                self.tone_curve_red = tone_curve_red;
                self.tone_curve_green = tone_curve_green;
                self.tone_curve_blue = tone_curve_blue;
            }
            ExposureSection::Group(AdjustmentGroup::Color) => {
                self.temperature = temperature;
                self.tint = tint;
                self.hue = hue;
                self.saturation = saturation;
                self.vibrance = vibrance;
            }
            ExposureSection::Group(AdjustmentGroup::ColorGrading) => {
                self.color_grading = color_grading;
            }
            ExposureSection::Group(AdjustmentGroup::Detail) => {
                self.luminance_denoise = luminance_denoise;
                self.chroma_denoise = chroma_denoise;
                self.denoise_detail = denoise_detail;
                self.denoise_quality = denoise_quality;
                self.ai_denoise_enabled = ai_denoise_enabled;
                self.sharpen_amount = sharpen_amount;
                self.sharpen_radius = sharpen_radius;
                self.sharpen_detail = sharpen_detail;
                self.sharpen_masking = sharpen_masking;
            }
            ExposureSection::Group(AdjustmentGroup::Effects) => {
                self.texture = texture;
                self.clarity = clarity;
                self.dehaze = dehaze;
                self.halation_amount = halation_amount;
                self.grain_amount = grain_amount;
                self.glow_amount = glow_amount;
                self.glow_radius = glow_radius;
                self.glow_threshold = glow_threshold;
                self.vignette_amount = vignette_amount;
                self.vignette_midpoint = vignette_midpoint;
                self.vignette_roundness = vignette_roundness;
                self.vignette_feather = vignette_feather;
                self.vignette_highlights = vignette_highlights;
            }
            ExposureSection::Group(AdjustmentGroup::ColorMixer) => {
                self.hsl_hue = hsl_hue;
                self.hsl_saturation = hsl_saturation;
                self.hsl_luminance = hsl_luminance;
                self.point_colors = point_colors;
            }
            ExposureSection::RawProcessing => {
                self.black_point = black_point;
                self.sigmoid = sigmoid;
                self.demosaic_mode = demosaic_mode;
                self.dual_threshold = dual_threshold;
                self.frequency_chroma = frequency_chroma;
                self.ca_red = ca_red;
                self.ca_blue = ca_blue;
                self.highlight_method = highlight_method;
                self.highlight_clip = highlight_clip;
                self.highlight_reconstruction = highlight_reconstruction;
            }
        }
    }

    /// Whether any setting owned by `group` differs from its default.
    pub fn group_is_edited(&self, group: AdjustmentGroup) -> bool {
        let defaults = Self::default();
        let mut probe = defaults;
        probe.copy_group_from(self, group);
        probe != defaults
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

    fn edited_exposure() -> ExposureParams {
        let mut value = serde_json::to_value(ExposureParams::default()).unwrap();
        edited(&mut value);
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn groups_and_raw_processing_cover_every_exposure_setting() {
        let source = edited_exposure();
        let mut copied = ExposureParams::default();
        for group in AdjustmentGroup::ALL {
            copied.copy_group_from(&source, group);
        }
        copied.copy_raw_processing_from(&source);
        assert_eq!(
            serde_json::to_value(copied).unwrap(),
            serde_json::to_value(source).unwrap()
        );
    }

    #[test]
    fn copying_one_group_leaves_other_settings_untouched() {
        let source = edited_exposure();
        let mut destination = ExposureParams::default();
        destination.copy_group_from(&source, AdjustmentGroup::Effects);

        assert_eq!(destination.clarity, source.clarity);
        assert_eq!(destination.exposure, ExposureParams::default().exposure);
        assert_eq!(
            destination.demosaic_mode,
            ExposureParams::default().demosaic_mode
        );
        assert!(destination.group_is_edited(AdjustmentGroup::Effects));
        for group in AdjustmentGroup::ALL {
            if group != AdjustmentGroup::Effects {
                assert!(!destination.group_is_edited(group), "{group:?}");
            }
        }
    }

    #[test]
    fn group_sets_serialize_as_group_names() {
        let set: AdjustmentGroupSet = [AdjustmentGroup::Light, AdjustmentGroup::ColorMixer]
            .into_iter()
            .collect();
        let json = serde_json::to_string(&set).unwrap();
        assert_eq!(json, r#"["light","color_mixer"]"#);
        assert_eq!(
            serde_json::from_str::<AdjustmentGroupSet>(&json).unwrap(),
            set
        );
        assert_eq!(
            AdjustmentGroupSet::ALL.iter().count(),
            AdjustmentGroup::ALL.len()
        );
        assert!(serde_json::from_str::<AdjustmentGroupSet>(r#"["optics"]"#).is_err());
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
