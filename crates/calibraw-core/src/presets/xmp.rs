//! Importing `.xmp` develop presets.
//!
//! An XMP preset stores develop settings in the Camera Raw settings schema
//! (the `crs:` namespace), the format Adobe Lightroom writes its presets in.
//! Each top-level `rdf:Description` holds the preset's own properties, either
//! as `crs:*` attributes or inside a `crs:Parameters` description. Only those
//! descriptions and their direct children are read: nested structures such
//! as `crs:Look` (the profile) and `crs:Preset` (a preset embedded in a
//! photo's sidecar) carry names and settings of their own.
//!
//! CalibRaw keeps the XMP as the preset source and translates it into a
//! [`Preset`] whenever presets load, so a later version can translate more
//! of it. Settings without an equivalent (perspective, defringing, camera
//! calibration, the black-and-white mixer, profiles and some local
//! adjustments) are reported as warnings instead of failing the import.
//!
//! Where CalibRaw's controls differ, the translation approximates: the
//! parametric curve is folded into the RGB point curve ([`curves`]), Point
//! Color is rescaled to CalibRaw's ranges ([`point_colors`]), lens profiles
//! are matched on the target photo, and white balance is absolute, so named
//! white balances use their usual temperature and tint. Masks are covered in
//! [`masks`].
//!
//! XMP writers use the conventional `rdf` and `crs` prefixes, so elements and
//! attributes are matched by prefix rather than by resolved namespace.

mod curves;
mod document;
mod masks;
mod point_colors;

use super::{read_preset_bytes, Preset, PresetError, DEFAULT_PRESET_GROUP, MAX_PRESET_NAME_CHARS};
use crate::pipeline::{
    AdjustmentGroup, AdjustmentGroupSet, ColorGradeWheel, ColorGrading, EffectComponent,
    ExposureParams, GrainEffectSettings, ManualLensCorrection, MaskEffect, MaskEffectSettings,
    MaskStack, VignetteEffectSettings, WhiteBalance, HSL_HUE_LIMIT, MAX_TEMPERATURE_KELVIN,
    MAX_WHITE_BALANCE_TINT, MIN_TEMPERATURE_KELVIN, MIN_WHITE_BALANCE_TINT,
};
use crate::sidecar::{default_edit_state, is_manual_mask_kind, EditSelection, LensEditState};
use curves::{compose, point_curve, read_points, ParametricCurve};
use document::{parse_document, split_bom, Document, Properties, Warnings};
use point_colors::read_point_colors;
use std::path::Path;
use std::sync::Arc;

pub(crate) use document::rewrite_xmp_labels;

/// The file extension of an XMP preset.
pub const XMP_PRESET_SUFFIX: &str = ".xmp";

const FALLBACK_NAME: &str = "Imported preset";

/// A translated preset and the settings that could not be translated.
#[derive(Clone, Debug, PartialEq)]
pub struct XmpImport {
    pub preset: Preset,
    pub warnings: Vec<String>,
}

/// Whether `path` names an XMP file, compared without case.
pub fn is_xmp_preset_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("xmp"))
}

/// Reads an XMP preset and the bytes it came from. The bytes are kept as the
/// preset source, so settings that cannot be translated yet survive. The
/// file name is the fallback preset name.
pub fn read_xmp_preset_file(path: &Path) -> Result<(XmpImport, Vec<u8>), PresetError> {
    let bytes = read_preset_bytes(path)?;
    let fallback = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.trim().is_empty())
        .unwrap_or(FALLBACK_NAME);
    let import = import(&bytes, fallback)?;
    Ok((import, bytes))
}

/// Translates an XMP preset document into a CalibRaw preset.
///
/// The name and group come from the document and fall back to "Imported
/// preset" and [`DEFAULT_PRESET_GROUP`]. Settings without an equivalent are
/// returned in [`XmpImport::warnings`].
pub fn import_xmp(bytes: &[u8]) -> Result<XmpImport, PresetError> {
    import(bytes, FALLBACK_NAME)
}

fn import(bytes: &[u8], fallback_name: &str) -> Result<XmpImport, PresetError> {
    let (_, text) = split_bom(bytes)?;
    let root = parse_document(text)?;
    let document = Document::new(&root)?;
    let mut converter = Converter {
        properties: document.properties(),
        warnings: Warnings::default(),
    };
    let preset = converter.convert(&document, fallback_name)?;
    Ok(XmpImport {
        preset,
        warnings: converter.warnings.into_vec(),
    })
}

const EXPOSURE_KEYS: &[&str] = &["Exposure2012", "Exposure"];
/// The -100..100 light sliders, each as its current and legacy key.
const TONE_SLIDER_KEYS: [[&str; 2]; 5] = [
    ["Contrast2012", "Contrast"],
    ["Highlights2012", "Highlights"],
    ["Shadows2012", "Shadows"],
    ["Whites2012", "Whites"],
    ["Blacks2012", "Blacks"],
];
const COLOR_KEYS: &[&str] = &[
    "WhiteBalance",
    "Temperature",
    "Tint",
    "IncrementalTemperature",
    "IncrementalTint",
    "Vibrance",
    "Saturation",
    "ConvertToGrayscale",
    "ShadowTint",
];
/// The color mixer channels, in CalibRaw's order.
const MIXER_COLORS: [&str; 8] = [
    "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
];
const MODERN_COLOR_GRADING_KEYS: &[&str] = &[
    "ColorGradeShadowHue",
    "ColorGradeShadowSat",
    "ColorGradeShadowLum",
    "ColorGradeMidtoneHue",
    "ColorGradeMidtoneSat",
    "ColorGradeMidtoneLum",
    "ColorGradeHighlightHue",
    "ColorGradeHighlightSat",
    "ColorGradeHighlightLum",
    "ColorGradeBlending",
    "ColorGradeGlobalHue",
    "ColorGradeGlobalSat",
    "ColorGradeGlobalLum",
];
const LEGACY_SPLIT_TONING_KEYS: &[&str] = &[
    "SplitToningShadowHue",
    "SplitToningShadowSaturation",
    "SplitToningHighlightHue",
    "SplitToningHighlightSaturation",
    "SplitToningBalance",
];
const DETAIL_KEYS: &[&str] = &[
    "Sharpness",
    "SharpenRadius",
    "SharpenDetail",
    "SharpenEdgeMasking",
    "LuminanceSmoothing",
    "LuminanceNoiseReductionDetail",
    "ColorNoiseReduction",
];
const PRESENCE_KEYS: &[&str] = &["Texture", "Clarity2012", "Clarity", "Dehaze"];
const GRAIN_KEYS: &[&str] = &["GrainAmount", "GrainSize", "GrainFrequency"];
const POST_CROP_VIGNETTE_KEYS: &[&str] = &[
    "PostCropVignetteAmount",
    "PostCropVignetteMidpoint",
    "PostCropVignetteFeather",
    "PostCropVignetteRoundness",
    "PostCropVignetteStyle",
    "PostCropVignetteHighlightContrast",
];

/// The curves CalibRaw stores, in its channel order, as the current key, the
/// legacy key and a label.
const TONE_CURVES: [(&str, &str, &str); 4] = [
    ("crs:ToneCurvePV2012", "crs:ToneCurve", "RGB"),
    ("crs:ToneCurvePV2012Red", "crs:ToneCurveRed", "red"),
    ("crs:ToneCurvePV2012Green", "crs:ToneCurveGreen", "green"),
    ("crs:ToneCurvePV2012Blue", "crs:ToneCurveBlue", "blue"),
];
/// Lens profile strengths, in percent; CalibRaw applies each part fully or
/// not at all.
const LENS_PROFILE_STRENGTH_KEYS: [&str; 3] = [
    "LensProfileDistortionScale",
    "LensProfileChromaticAberrationScale",
    "LensProfileVignettingScale",
];
/// Settings without an equivalent, each with its neutral value, and the
/// warning shown when any of them is away from neutral.
const UNSUPPORTED_SETTINGS: &[(&[(&str, f32)], &str)] = &[
    (
        &[
            ("PerspectiveUpright", 0.0),
            ("PerspectiveVertical", 0.0),
            ("PerspectiveHorizontal", 0.0),
            ("PerspectiveRotate", 0.0),
            ("PerspectiveAspect", 0.0),
            ("PerspectiveScale", 100.0),
            ("PerspectiveX", 0.0),
            ("PerspectiveY", 0.0),
        ],
        "Perspective corrections were not imported.",
    ),
    (
        &[("DefringePurpleAmount", 0.0), ("DefringeGreenAmount", 0.0)],
        "Defringing was not imported.",
    ),
    (
        &[
            ("RedHue", 0.0),
            ("RedSaturation", 0.0),
            ("GreenHue", 0.0),
            ("GreenSaturation", 0.0),
            ("BlueHue", 0.0),
            ("BlueSaturation", 0.0),
        ],
        "Camera calibration color adjustments were not imported.",
    ),
    // CalibRaw's single Denoise Detail slider follows the luminance detail.
    (
        &[
            ("ColorNoiseReductionDetail", 50.0),
            ("ColorNoiseReductionSmoothness", 50.0),
            ("LuminanceNoiseReductionContrast", 0.0),
        ],
        "Some noise reduction detail settings were not imported.",
    ),
    (
        &[("IncrementalTemperature", 0.0), ("IncrementalTint", 0.0)],
        "Incremental white balance adjustments were not imported.",
    ),
    (
        &[("ShadowTint", 0.0)],
        "The legacy shadow tint was not imported.",
    ),
];

struct Converter<'a> {
    properties: Properties<'a>,
    warnings: Warnings,
}

impl Converter<'_> {
    fn convert(
        &mut self,
        document: &Document<'_>,
        fallback_name: &str,
    ) -> Result<Preset, PresetError> {
        let name = self.label(document.label("crs:Name"), fallback_name, "preset name");
        let group = self.label(
            document.label("crs:Group"),
            DEFAULT_PRESET_GROUP,
            "group name",
        );
        let mut exposure = ExposureParams::default();
        let mut groups = AdjustmentGroupSet::EMPTY;
        let mut effects = Vec::new();

        self.convert_light(&mut exposure, &mut groups);
        self.convert_tone_curves(document, &mut exposure, &mut groups);
        self.convert_color(&mut exposure, &mut groups);
        self.convert_color_mixer(document, &mut exposure, &mut groups);
        self.convert_color_grading(&mut exposure, &mut groups);
        self.convert_detail(&mut exposure, &mut groups);
        self.convert_effects(&mut exposure, &mut groups, &mut effects);
        let lens = self.convert_lens(&mut exposure);
        let masks = masks::convert_masks(&document.corrections(), &mut self.warnings);
        self.warn_unsupported_settings();

        if groups.is_empty() && masks.is_empty() && lens.is_none() {
            self.warnings
                .push("No settings in this preset can be translated yet.");
        }

        let components = || masks.iter().flat_map(|mask| &mask.components);
        let selection = EditSelection {
            adjustment_groups: groups,
            masks: components().any(|component| is_manual_mask_kind(component.kind)),
            ai_masks: components().any(|component| !is_manual_mask_kind(component.kind)),
            lens_correction: lens.is_some(),
            ..EditSelection::default()
        };
        let mut edits = default_edit_state();
        edits.exposure = exposure;
        edits.lens = lens.unwrap_or_default();
        edits.masks = Arc::new(MaskStack {
            masks,
            global_effects: effects,
            ..MaskStack::default()
        });
        Preset::from_xmp(&name, &group, selection, &edits)
    }

    /// A preset label, cleaned and shortened to the label limit.
    fn label(&mut self, raw: Option<&str>, fallback: &str, what: &str) -> String {
        let (label, shortened) =
            sanitize_label(raw.unwrap_or(fallback), fallback, MAX_PRESET_NAME_CHARS);
        if shortened {
            self.warnings.push(format!(
                "The {what} was shortened to {MAX_PRESET_NAME_CHARS} characters."
            ));
        }
        label
    }

    fn number(&mut self, key: &str) -> Option<f32> {
        self.properties.number(key, &mut self.warnings)
    }

    fn first_number(&mut self, keys: &[&str]) -> Option<f32> {
        self.properties.first_number(keys, &mut self.warnings)
    }

    fn convert_light(&mut self, exposure: &mut ExposureParams, groups: &mut AdjustmentGroupSet) {
        if !self.properties.has_any(EXPOSURE_KEYS)
            && !TONE_SLIDER_KEYS
                .iter()
                .any(|keys| self.properties.has_any(keys))
        {
            return;
        }
        groups.set(AdjustmentGroup::Light, true);
        if let Some(value) = self.first_number(EXPOSURE_KEYS) {
            exposure.exposure = value.clamp(-5.0, 5.0);
        }
        let targets = [
            &mut exposure.contrast,
            &mut exposure.highlights,
            &mut exposure.shadows,
            &mut exposure.whites,
            &mut exposure.blacks,
        ];
        for (keys, target) in TONE_SLIDER_KEYS.iter().zip(targets) {
            if let Some(value) = self.first_number(keys) {
                *target = value.clamp(-100.0, 100.0);
            }
        }
    }

    fn convert_tone_curves(
        &mut self,
        document: &Document<'_>,
        exposure: &mut ExposureParams,
        groups: &mut AdjustmentGroupSet,
    ) {
        // The current layout wins per channel, so a file that mixes layouts
        // still keeps the channels only the legacy layout has.
        let elements = TONE_CURVES.map(|(current, legacy, _)| {
            document
                .property_element(current)
                .or_else(|| document.property_element(legacy))
        });
        let parametric = ParametricCurve::from_properties(&self.properties, &mut self.warnings);
        if parametric.is_none() && elements.iter().all(Option::is_none) {
            return;
        }
        groups.set(AdjustmentGroup::ToneCurve, true);
        let targets = [
            &mut exposure.tone_curve,
            &mut exposure.tone_curve_red,
            &mut exposure.tone_curve_green,
            &mut exposure.tone_curve_blue,
        ];
        for (channel, ((element, (_, _, label)), target)) in
            elements.iter().zip(TONE_CURVES).zip(targets).enumerate()
        {
            let points = element.and_then(|element| {
                read_points(
                    element,
                    &format!("The {label} tone curve"),
                    &mut self.warnings,
                )
            });
            // The parametric curve shapes the RGB curve only.
            match (parametric.filter(|_| channel == 0), points) {
                (Some(parametric), points) => *target = compose(&parametric, points.as_deref()),
                (None, Some(points)) => *target = point_curve(&points, &mut self.warnings),
                (None, None) => {}
            }
        }
    }

    fn convert_color(&mut self, exposure: &mut ExposureParams, groups: &mut AdjustmentGroupSet) {
        if !self.properties.has_any(COLOR_KEYS) {
            return;
        }
        groups.set(AdjustmentGroup::Color, true);

        exposure.white_balance = self.white_balance();
        if let Some(value) = self.number("Vibrance") {
            exposure.vibrance = value.clamp(-100.0, 100.0);
        }
        if let Some(value) = self.number("Saturation") {
            exposure.saturation = value.clamp(-100.0, 100.0);
        }
        if self.properties.boolean("ConvertToGrayscale") == Some(true) {
            exposure.saturation = -100.0;
            if MIXER_COLORS
                .iter()
                .any(|color| self.properties.differs(&format!("GrayMixer{color}"), 0.0))
            {
                self.warnings.push(
                    "Grayscale mix settings were not imported; the preset is imported as a black-and-white conversion.",
                );
            }
        }
    }

    fn white_balance(&mut self) -> WhiteBalance {
        let Some(value) = self.properties.text("WhiteBalance") else {
            if let Some(temperature) = self.number("Temperature") {
                let tint = self.number("Tint").unwrap_or(0.0);
                return custom_white_balance(temperature, tint);
            }
            return WhiteBalance::AsShot;
        };

        if value.eq_ignore_ascii_case("As Shot") || value.eq_ignore_ascii_case("Auto") {
            return WhiteBalance::AsShot;
        }

        if let Some(temperature) = self.number("Temperature") {
            let tint = self.number("Tint").unwrap_or(0.0);
            return custom_white_balance(temperature, tint);
        }

        let typical = match value.to_ascii_lowercase().as_str() {
            "daylight" => Some((5_500.0, 0.0)),
            "cloudy" => Some((6_500.0, 10.0)),
            "shade" => Some((7_500.0, 10.0)),
            "tungsten" => Some((2_850.0, 0.0)),
            "fluorescent" => Some((3_800.0, 15.0)),
            "flash" => Some((5_500.0, 0.0)),
            _ => None,
        };
        if let Some((temperature, tint)) = typical {
            return custom_white_balance(temperature, tint);
        }

        if value.eq_ignore_ascii_case("Custom") {
            self.warnings
                .push("A custom white balance without temperature and tint was left as shot.");
        } else {
            self.warnings.push(format!(
                "The white balance “{value}” is not supported; it was left as shot."
            ));
        }
        WhiteBalance::AsShot
    }

    fn convert_color_mixer(
        &mut self,
        document: &Document<'_>,
        exposure: &mut ExposureParams,
        groups: &mut AdjustmentGroupSet,
    ) {
        let keys = MIXER_COLORS.map(|color| {
            [
                format!("HueAdjustment{color}"),
                format!("SaturationAdjustment{color}"),
                format!("LuminanceAdjustment{color}"),
            ]
        });
        let point_colors = document
            .property_element("crs:PointColors")
            .map(|swatches| {
                read_point_colors(
                    swatches,
                    document.property_element("crs:ColorVariance"),
                    "",
                    &mut self.warnings,
                )
            });
        if point_colors.is_none() && !keys.iter().flatten().any(|key| self.properties.has(key)) {
            return;
        }
        groups.set(AdjustmentGroup::ColorMixer, true);
        exposure.point_colors = point_colors.unwrap_or_default();
        for (index, [hue, saturation, luminance]) in keys.iter().enumerate() {
            // The ±100 hue slider spans CalibRaw's ±200.
            if let Some(value) = self.number(hue) {
                exposure.hsl_hue[index] = (value * 2.0).clamp(-HSL_HUE_LIMIT, HSL_HUE_LIMIT);
            }
            if let Some(value) = self.number(saturation) {
                exposure.hsl_saturation[index] = value.clamp(-100.0, 100.0);
            }
            if let Some(value) = self.number(luminance) {
                exposure.hsl_luminance[index] = value.clamp(-100.0, 100.0);
            }
        }
    }

    fn convert_color_grading(
        &mut self,
        exposure: &mut ExposureParams,
        groups: &mut AdjustmentGroupSet,
    ) {
        let has_modern = self.properties.has_any(MODERN_COLOR_GRADING_KEYS);
        let has_legacy = self.properties.has_any(LEGACY_SPLIT_TONING_KEYS);
        if !has_modern && !has_legacy {
            return;
        }
        groups.set(AdjustmentGroup::ColorGrading, true);
        let mut grading = ColorGrading {
            shadows: self.color_grade_wheel(
                &["ColorGradeShadowHue", "SplitToningShadowHue"],
                &["ColorGradeShadowSat", "SplitToningShadowSaturation"],
                &["ColorGradeShadowLum"],
            ),
            midtones: self.color_grade_wheel(
                &["ColorGradeMidtoneHue"],
                &["ColorGradeMidtoneSat"],
                &["ColorGradeMidtoneLum"],
            ),
            highlights: self.color_grade_wheel(
                &["ColorGradeHighlightHue", "SplitToningHighlightHue"],
                &["ColorGradeHighlightSat", "SplitToningHighlightSaturation"],
                &["ColorGradeHighlightLum"],
            ),
            global: self.color_grade_wheel(
                &["ColorGradeGlobalHue"],
                &["ColorGradeGlobalSat"],
                &["ColorGradeGlobalLum"],
            ),
            ..ColorGrading::default()
        };
        if let Some(value) = self.number("ColorGradeBlending") {
            grading.blending = value.clamp(0.0, 100.0);
        } else if !has_modern {
            // A legacy split-toning preset renders with Blending at 100, so
            // its two tints cover the tonal ranges like split toning.
            grading.blending = 100.0;
        }
        if let Some(value) = self.number("SplitToningBalance") {
            grading.balance = value.clamp(-100.0, 100.0);
        }
        exposure.color_grading = grading;
    }

    fn color_grade_wheel(
        &mut self,
        hue_keys: &[&str],
        saturation_keys: &[&str],
        luminance_keys: &[&str],
    ) -> ColorGradeWheel {
        ColorGradeWheel {
            hue: self.first_number(hue_keys).unwrap_or(0.0).rem_euclid(360.0),
            saturation: self
                .first_number(saturation_keys)
                .unwrap_or(0.0)
                .clamp(0.0, 100.0),
            luminance: self
                .first_number(luminance_keys)
                .unwrap_or(0.0)
                .clamp(-100.0, 100.0),
        }
    }

    fn convert_detail(&mut self, exposure: &mut ExposureParams, groups: &mut AdjustmentGroupSet) {
        if !self.properties.has_any(DETAIL_KEYS) {
            return;
        }
        groups.set(AdjustmentGroup::Detail, true);
        for (key, target, min, max) in [
            ("Sharpness", &mut exposure.sharpen_amount, 0.0, 150.0),
            ("SharpenRadius", &mut exposure.sharpen_radius, 0.5, 3.0),
            ("SharpenDetail", &mut exposure.sharpen_detail, 0.0, 100.0),
            (
                "SharpenEdgeMasking",
                &mut exposure.sharpen_masking,
                0.0,
                100.0,
            ),
            (
                "LuminanceSmoothing",
                &mut exposure.luminance_denoise,
                0.0,
                100.0,
            ),
            (
                "LuminanceNoiseReductionDetail",
                &mut exposure.denoise_detail,
                0.0,
                100.0,
            ),
        ] {
            if let Some(value) = self.number(key) {
                *target = value.clamp(min, max);
            }
        }
        if let Some(value) = self.number("ColorNoiseReduction") {
            exposure.chroma_denoise = (value / 100.0).clamp(0.0, 1.0);
        }
    }

    fn convert_effects(
        &mut self,
        exposure: &mut ExposureParams,
        groups: &mut AdjustmentGroupSet,
        effects: &mut Vec<EffectComponent>,
    ) {
        let has_grain = self.properties.has_any(GRAIN_KEYS);
        let has_vignette = self.properties.has_any(POST_CROP_VIGNETTE_KEYS);
        if !self.properties.has_any(PRESENCE_KEYS) && !has_grain && !has_vignette {
            return;
        }
        groups.set(AdjustmentGroup::Effects, true);
        if let Some(value) = self.number("Texture") {
            exposure.texture = value.clamp(-100.0, 100.0);
        }
        if let Some(value) = self.first_number(&["Clarity2012", "Clarity"]) {
            exposure.clarity = value.clamp(-100.0, 100.0);
        }
        if let Some(value) = self.number("Dehaze") {
            exposure.dehaze = value.clamp(-100.0, 100.0);
        }

        if has_grain {
            let grain = self.grain_settings();
            if grain.amount > 1e-6 {
                effects.push(EffectComponent {
                    effect: MaskEffect::Grain,
                    enabled: true,
                    settings: MaskEffectSettings {
                        grain,
                        ..MaskEffectSettings::default()
                    },
                });
            }
        }
        if has_vignette {
            let vignette = self.vignette_settings();
            if vignette.amount.abs() > 1e-6 {
                effects.push(EffectComponent {
                    effect: MaskEffect::Vignette,
                    enabled: true,
                    settings: MaskEffectSettings {
                        vignette,
                        ..MaskEffectSettings::default()
                    },
                });
            }
        }
    }

    fn grain_settings(&mut self) -> GrainEffectSettings {
        let defaults = GrainEffectSettings::default();
        GrainEffectSettings {
            amount: self.number("GrainAmount").unwrap_or(0.0).clamp(0.0, 100.0),
            size: self
                .number("GrainSize")
                .map(grain_size)
                .unwrap_or(defaults.size),
            roughness: self
                .number("GrainFrequency")
                .unwrap_or(defaults.roughness)
                .clamp(0.0, 100.0),
            ..defaults
        }
    }

    fn vignette_settings(&mut self) -> VignetteEffectSettings {
        // The format's defaults differ from CalibRaw's, so a preset that
        // omits a slider must not inherit CalibRaw's stronger default.
        let mut settings = VignetteEffectSettings {
            amount: 0.0,
            midpoint: 50.0,
            roundness: 0.0,
            feather: 50.0,
            highlights: 0.0,
            center: [50.0, 50.0],
        };
        for (key, target, min) in [
            ("PostCropVignetteAmount", &mut settings.amount, -100.0),
            ("PostCropVignetteMidpoint", &mut settings.midpoint, 0.0),
            ("PostCropVignetteFeather", &mut settings.feather, 0.0),
            ("PostCropVignetteRoundness", &mut settings.roundness, -100.0),
            (
                "PostCropVignetteHighlightContrast",
                &mut settings.highlights,
                0.0,
            ),
        ] {
            if let Some(value) = self.number(key) {
                *target = value.clamp(min, 100.0);
            }
        }
        settings
    }

    /// The lens corrections, or `None` when the preset turns none on, so
    /// applying it leaves the photo's own lens corrections alone.
    ///
    /// The profile is matched on the target photo, as lens names differ
    /// between profile catalogs. Manual corrections keep their -100..100
    /// sliders, though their strength is CalibRaw's. CalibRaw applies them
    /// instead of the profile, so a preset with both keeps the manual ones.
    fn convert_lens(&mut self, exposure: &mut ExposureParams) -> Option<LensEditState> {
        let profile = self.properties.boolean("LensProfileEnable") == Some(true);
        let manual = ManualLensCorrection {
            distortion: self
                .number("LensManualDistortionAmount")
                .unwrap_or(0.0)
                .clamp(-100.0, 100.0),
            vignetting: self
                .number("VignetteAmount")
                .unwrap_or(0.0)
                .clamp(-100.0, 100.0),
        };
        let remove_aberration = self.properties.boolean("AutoLateralCA") == Some(true);
        if !profile && manual.is_identity() && !remove_aberration {
            return None;
        }
        exposure.chromatic_aberration = if remove_aberration { 100.0 } else { 0.0 };

        let manual = (!manual.is_identity()).then_some(manual);
        let [distortion, _, vignetting] =
            LENS_PROFILE_STRENGTH_KEYS.map(|key| self.properties.value(key).unwrap_or(100.0));
        if profile && manual.is_some() {
            self.warnings.push(
                "A lens profile and manual lens corrections cannot be combined; the manual corrections were imported.",
            );
        } else if profile
            && LENS_PROFILE_STRENGTH_KEYS
                .iter()
                .any(|key| self.properties.differs(key, 100.0) && self.properties.differs(key, 0.0))
        {
            self.warnings
                .push("Lens profile strengths other than 0 and 100 were not imported.");
        }
        if manual.is_some_and(|manual| manual.vignetting != 0.0)
            && self.properties.differs("VignetteMidpoint", 50.0)
        {
            self.warnings
                .push("The lens vignetting midpoint was not imported.");
        }
        Some(LensEditState {
            enabled: profile || manual.is_some(),
            geometry: distortion > 0.0,
            vignetting: vignetting > 0.0,
            manual,
            ..LensEditState::default()
        })
    }

    fn warn_unsupported_settings(&mut self) {
        let properties = &self.properties;
        for (settings, warning) in UNSUPPORTED_SETTINGS {
            if settings
                .iter()
                .any(|(key, neutral)| properties.differs(key, *neutral))
            {
                self.warnings.push(*warning);
            }
        }

        // Style 1 is Highlight Priority, which CalibRaw's vignette follows;
        // Color Priority (2) and Paint Overlay (3) do not map onto it.
        if properties
            .value("PostCropVignetteStyle")
            .is_some_and(|style| style > 1.0)
        {
            self.warnings
                .push("The post-crop vignette style was not imported.");
        }
        if let Some(version) = properties
            .value("ProcessVersion")
            .filter(|version| *version < 11.0)
        {
            self.warnings.push(format!(
                "Process version {version} is older than 11; the preset may look different."
            ));
        }
        if let Some(profile) = properties
            .text("CameraProfile")
            .filter(|profile| !profile.eq_ignore_ascii_case("Embedded"))
        {
            self.warnings
                .push(format!("Camera profile “{profile}” was not imported."));
        }
        if properties.boolean("HasCrop") == Some(true) {
            self.warnings.push("Crop settings were not imported.");
        }
    }
}

/// Replaces control characters, collapses whitespace and shortens `text` to
/// `max_chars`. Returns the label and whether it was shortened; an empty
/// label becomes `fallback`.
fn sanitize_label(text: &str, fallback: &str, max_chars: usize) -> (String, bool) {
    let collapsed = text
        .split(|character: char| character.is_whitespace() || character.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let shortened = collapsed.chars().count() > max_chars;
    let label: String = collapsed.chars().take(max_chars).collect();
    let label = label.trim();
    if label.is_empty() {
        (fallback.to_owned(), false)
    } else {
        (label.to_owned(), shortened)
    }
}

fn custom_white_balance(temperature: f32, tint: f32) -> WhiteBalance {
    WhiteBalance::Custom {
        kelvin: temperature.clamp(MIN_TEMPERATURE_KELVIN, MAX_TEMPERATURE_KELVIN),
        tint: tint.clamp(MIN_WHITE_BALANCE_TINT, MAX_WHITE_BALANCE_TINT),
        older_reader_offsets: [0.0, 0.0],
    }
}

/// Maps the 0–100 grain size to CalibRaw's 0.5–4.0. The format's default 25
/// lands on CalibRaw's default 1.0.
fn grain_size(value: f32) -> f32 {
    super::grain_size_from_import(value)
}
