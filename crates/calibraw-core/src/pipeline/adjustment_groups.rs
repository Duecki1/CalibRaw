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

/// Which part of [`ExposureParams`] a copy covers: one sidebar card, the
/// Lens Corrections card's settings, which move with lens correction rather
/// than as an adjustment group, or the RAW decoding settings that no card owns.
#[derive(Clone, Copy)]
enum ExposureSection {
    Group(AdjustmentGroup),
    LensCorrection,
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

    /// Copies the RAW decoding settings: black point, tone mapper, demosaic
    /// and highlight reconstruction, which no sidebar card owns, and the film
    /// negative, which a roll of scans shares.
    pub fn copy_raw_processing_from(&mut self, source: &Self) {
        self.copy_section_from(source, ExposureSection::RawProcessing);
    }

    /// Copies the develop settings of the Lens Corrections card: chromatic
    /// aberration removal. The lens profile and manual corrections are not
    /// develop settings (`LensEditState`).
    pub fn copy_lens_correction_from(&mut self, source: &Self) {
        self.copy_section_from(source, ExposureSection::LensCorrection);
    }

    fn copy_section_from(&mut self, source: &Self, section: ExposureSection) {
        // Destructured without `..`: a new field does not compile until it is
        // assigned to exactly one section below.
        let Self {
            black_point,
            exposure,
            contrast,
            sigmoid,
            white_balance,
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
            ai_denoise_amount,
            demosaic_mode,
            dual_threshold,
            frequency_chroma,
            highlight_method,
            highlight_clip,
            highlight_reconstruction,
            chromatic_aberration,
            film,
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
                self.white_balance = white_balance;
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
                self.ai_denoise_amount = ai_denoise_amount;
                self.sharpen_amount = sharpen_amount;
                self.sharpen_radius = sharpen_radius;
                self.sharpen_detail = sharpen_detail;
                self.sharpen_masking = sharpen_masking;
            }
            ExposureSection::Group(AdjustmentGroup::Effects) => {
                self.texture = texture;
                self.clarity = clarity;
                self.dehaze = dehaze;
            }
            ExposureSection::Group(AdjustmentGroup::ColorMixer) => {
                self.hsl_hue = hsl_hue;
                self.hsl_saturation = hsl_saturation;
                self.hsl_luminance = hsl_luminance;
                self.point_colors = point_colors;
            }
            ExposureSection::LensCorrection => {
                self.chromatic_aberration = chromatic_aberration;
            }
            ExposureSection::RawProcessing => {
                self.black_point = black_point;
                self.sigmoid = sigmoid;
                self.demosaic_mode = demosaic_mode;
                self.dual_threshold = dual_threshold;
                self.frequency_chroma = frequency_chroma;
                self.highlight_method = highlight_method;
                self.highlight_clip = highlight_clip;
                self.highlight_reconstruction = highlight_reconstruction;
                self.film = film;
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
