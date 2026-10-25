//! Importing Adobe Lightroom preset `.xmp` files.
//!
//! Lightroom stores a preset as XMP: the top-level `rdf:Description` carries
//! the develop settings, either as `crs:*` attributes or inside a
//! `crs:Parameters` description. Lightroom also embeds a complete preset in a
//! camera-raw sidecar under `crs:Preset`; that subtree belongs to a different
//! preset and is ignored here.
//!
//! The conversion covers the settings CalibRaw has an equivalent for. Settings
//! that cannot be represented (lens corrections, perspective, defringing,
//! parametric curves, camera calibration and profiles) are reported as
//! warnings instead of failing the import. White balance is absolute in
//! CalibRaw, so Lightroom's named white balances use their usual temperature
//! and tint values.

use super::{Preset, PresetError, DEFAULT_PRESET_GROUP, MAX_PRESET_BYTES, MAX_PRESET_NAME_CHARS};
use crate::pipeline::{
    AdjustmentGroup, AdjustmentGroupSet, ColorGradeWheel, ColorGrading, EffectComponent,
    ExposureParams, GrainEffectSettings, MaskEffect, MaskEffectSettings, MaskStack, PointCurve,
    VignetteEffectSettings, WhiteBalance, HSL_HUE_LIMIT, MAX_POINT_CURVE_POINTS,
    MAX_TEMPERATURE_KELVIN, MAX_WHITE_BALANCE_TINT, MIN_TEMPERATURE_KELVIN, MIN_WHITE_BALANCE_TINT,
};
use crate::sidecar::{default_edit_state, EditSelection};
use quick_xml::events::{BytesEnd, BytesRef, BytesStart, Event};
use quick_xml::Reader;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

/// The file extension of an Adobe Lightroom preset.
pub const LIGHTROOM_PRESET_SUFFIX: &str = ".xmp";

/// A converted preset and the Lightroom settings that could not be imported.
#[derive(Clone, Debug, PartialEq)]
pub struct LightroomImport {
    pub preset: Preset,
    pub warnings: Vec<String>,
}

/// Whether `path` names an XMP file, compared without case.
pub fn is_lightroom_preset_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("xmp"))
}

/// Reads a Lightroom preset from `path`. See [`import_lightroom_xmp`].
pub fn read_lightroom_preset_file(path: &Path) -> Result<LightroomImport, PresetError> {
    let file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    if length > MAX_PRESET_BYTES {
        return Err(PresetError::TooLarge(length));
    }
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(MAX_PRESET_BYTES + 1).read_to_end(&mut bytes)?;
    let fallback = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Lightroom preset");
    import_lightroom_xmp_named(&bytes, fallback)
}

/// Converts a Lightroom preset XMP document into a CalibRaw preset.
///
/// The preset name and group come from the document and fall back to
/// "Lightroom preset" and [`DEFAULT_PRESET_GROUP`]. Unsupported settings are
/// returned in [`LightroomImport::warnings`].
pub fn import_lightroom_xmp(bytes: &[u8]) -> Result<LightroomImport, PresetError> {
    import_lightroom_xmp_named(bytes, "Lightroom preset")
}

fn import_lightroom_xmp_named(
    bytes: &[u8],
    fallback_name: &str,
) -> Result<LightroomImport, PresetError> {
    if bytes.len() as u64 > MAX_PRESET_BYTES {
        return Err(PresetError::TooLarge(bytes.len() as u64));
    }
    let text = decode_utf8(bytes)?;
    let parsed = XmpParser::default().parse(text)?;
    let mut converter = Converter::new(&parsed);
    let preset = converter.convert(fallback_name)?;
    Ok(LightroomImport {
        preset,
        warnings: converter.warnings,
    })
}

fn decode_utf8(bytes: &[u8]) -> Result<&str, PresetError> {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return Err(PresetError::Unsupported(
            "UTF-16 XMP presets are not supported; save the preset as UTF-8".to_owned(),
        ));
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    std::str::from_utf8(bytes)
        .map_err(|_| PresetError::Unsupported("the XMP preset is not valid UTF-8".to_owned()))
}

fn invalid_xml(message: impl Into<String>) -> PresetError {
    PresetError::Invalid(format!("invalid XMP: {}", message.into()))
}

// ---------------------------------------------------------------------------
// XMP parsing
// ---------------------------------------------------------------------------

/// One of the four curves CalibRaw stores.
const CURVE_RGB: usize = 0;
const CURVE_RED: usize = 1;
const CURVE_GREEN: usize = 2;
const CURVE_BLUE: usize = 3;

#[derive(Clone, Copy)]
enum CaptureKind {
    Name,
    Group,
    Curve { legacy: bool, channel: usize },
}

struct Capture {
    kind: CaptureKind,
    /// The depth of the element that started this capture.
    depth: usize,
    text: String,
    /// The depth of the open `rdf:li`, for curves.
    point_depth: Option<usize>,
    points: Vec<String>,
}

impl Capture {
    fn new(kind: CaptureKind, depth: usize) -> Self {
        Self {
            kind,
            depth,
            text: String::new(),
            point_depth: None,
            points: Vec::new(),
        }
    }

    fn is_curve(&self) -> bool {
        matches!(self.kind, CaptureKind::Curve { .. })
    }

    fn begin_point(&mut self, depth: usize) {
        self.point_depth = Some(depth);
        self.text.clear();
    }

    fn finish_point(&mut self) {
        let point = std::mem::take(&mut self.text);
        if !point.trim().is_empty() {
            self.points.push(point);
        }
        self.point_depth = None;
    }

    fn finish(mut self, parsed: &mut ParsedXmp) {
        if self.point_depth.is_some() {
            self.finish_point();
        }
        match self.kind {
            CaptureKind::Name => set_label(&mut parsed.name, &self.text),
            CaptureKind::Group => set_label(&mut parsed.group, &self.text),
            CaptureKind::Curve { legacy, channel } => {
                if legacy {
                    parsed.legacy_curves.set(channel, self.points);
                } else {
                    parsed.curves.set(channel, self.points);
                }
            }
        }
    }
}

fn set_label(slot: &mut Option<String>, text: &str) {
    let text = text.trim();
    if !text.is_empty() {
        *slot = Some(text.to_owned());
    }
}

#[derive(Clone, Default)]
struct CurveSet {
    present: [bool; 4],
    points: [Vec<String>; 4],
}

impl CurveSet {
    fn is_empty(&self) -> bool {
        !self.present.iter().any(|present| *present)
    }

    fn set(&mut self, channel: usize, points: Vec<String>) {
        self.present[channel] = true;
        self.points[channel] = points;
    }

    fn get(&self, channel: usize) -> Option<&[String]> {
        self.present[channel].then_some(self.points[channel].as_slice())
    }
}

#[derive(Default)]
struct ParsedXmp {
    attributes: BTreeMap<String, String>,
    name: Option<String>,
    group: Option<String>,
    curves: CurveSet,
    legacy_curves: CurveSet,
    description_found: bool,
}

#[derive(Default)]
struct XmpParser {
    depth: usize,
    rdf_depth: Option<usize>,
    top_depth: Option<usize>,
    skip_depth: Option<usize>,
    capture: Option<Capture>,
    parsed: ParsedXmp,
}

impl XmpParser {
    fn parse(mut self, text: &str) -> Result<ParsedXmp, PresetError> {
        let mut reader = Reader::from_str(text);
        reader.config_mut().trim_text(false);
        loop {
            match reader
                .read_event()
                .map_err(|error| invalid_xml(error.to_string()))?
            {
                Event::Start(start) => {
                    self.start(&start, false)?;
                    self.depth += 1;
                }
                Event::Empty(start) => self.start(&start, true)?,
                Event::End(end) => {
                    self.depth = self
                        .depth
                        .checked_sub(1)
                        .ok_or_else(|| invalid_xml("unexpected closing tag"))?;
                    self.end(&end);
                }
                Event::Text(text) => {
                    let text = text
                        .decode()
                        .map_err(|error| invalid_xml(format!("could not decode text: {error}")))?;
                    self.text(&text);
                }
                Event::CData(cdata) => {
                    let text = cdata
                        .decode()
                        .map_err(|error| invalid_xml(format!("could not decode text: {error}")))?;
                    self.text(&text);
                }
                Event::GeneralRef(reference) => self.reference(&reference)?,
                Event::Eof => break,
                _ => {}
            }
        }
        if self.top_depth.is_some() {
            return Err(invalid_xml("the preset description is not closed"));
        }
        if !self.parsed.description_found {
            return Err(invalid_xml("no rdf:Description was found"));
        }
        Ok(self.parsed)
    }

    fn start(&mut self, start: &BytesStart<'_>, empty: bool) -> Result<(), PresetError> {
        let (local, prefix) = start.name().decompose();
        let local = local.as_ref();
        let is_rdf = prefix.is_some_and(|prefix| prefix.as_ref() == b"rdf");
        let is_crs = prefix.is_some_and(|prefix| prefix.as_ref() == b"crs");

        if self.skip_depth.is_some() {
            return Ok(());
        }

        if is_rdf && local == b"RDF" {
            self.rdf_depth = Some(self.depth);
            return Ok(());
        }

        if is_rdf && local == b"Description" {
            let direct_child = match self.rdf_depth {
                Some(rdf_depth) => self.depth == rdf_depth + 1,
                None => true,
            };
            if direct_child && self.top_depth.is_none() {
                self.parsed.description_found = true;
                self.collect_attributes(start)?;
                // A self-closing description carries only attributes, so there
                // is no element to close later.
                if !empty {
                    self.top_depth = Some(self.depth);
                }
            } else if self.top_depth.is_some() {
                // The description inside crs:Parameters.
                self.collect_attributes(start)?;
            }
            return Ok(());
        }

        if self.top_depth.is_none() {
            return Ok(());
        }

        if is_crs && local == b"Preset" {
            if !empty {
                self.skip_depth = Some(self.depth);
            }
            return Ok(());
        }

        if self.capture.is_some() {
            if !empty
                && is_rdf
                && local == b"li"
                && self.capture.as_ref().is_some_and(Capture::is_curve)
            {
                if let Some(capture) = self.capture.as_mut() {
                    capture.begin_point(self.depth);
                }
            }
            return Ok(());
        }

        if !is_crs || empty {
            return Ok(());
        }

        let kind = match local {
            b"Name" => Some(CaptureKind::Name),
            b"Group" => Some(CaptureKind::Group),
            b"ToneCurvePV2012" => Some(CaptureKind::Curve {
                legacy: false,
                channel: CURVE_RGB,
            }),
            b"ToneCurvePV2012Red" => Some(CaptureKind::Curve {
                legacy: false,
                channel: CURVE_RED,
            }),
            b"ToneCurvePV2012Green" => Some(CaptureKind::Curve {
                legacy: false,
                channel: CURVE_GREEN,
            }),
            b"ToneCurvePV2012Blue" => Some(CaptureKind::Curve {
                legacy: false,
                channel: CURVE_BLUE,
            }),
            b"ToneCurve" => Some(CaptureKind::Curve {
                legacy: true,
                channel: CURVE_RGB,
            }),
            b"ToneCurveRed" => Some(CaptureKind::Curve {
                legacy: true,
                channel: CURVE_RED,
            }),
            b"ToneCurveGreen" => Some(CaptureKind::Curve {
                legacy: true,
                channel: CURVE_GREEN,
            }),
            b"ToneCurveBlue" => Some(CaptureKind::Curve {
                legacy: true,
                channel: CURVE_BLUE,
            }),
            _ => None,
        };
        if let Some(kind) = kind {
            self.capture = Some(Capture::new(kind, self.depth));
        }
        Ok(())
    }

    fn end(&mut self, end: &BytesEnd<'_>) {
        if let Some(skip_depth) = self.skip_depth {
            if self.depth == skip_depth {
                self.skip_depth = None;
            }
            return;
        }

        if self.capture.is_some() {
            let (local, prefix) = end.name().decompose();
            let closes_point =
                prefix.is_some_and(|prefix| prefix.as_ref() == b"rdf") && local.as_ref() == b"li";
            let mut closes_capture = false;
            if let Some(capture) = self.capture.as_mut() {
                if closes_point && capture.point_depth == Some(self.depth) {
                    capture.finish_point();
                }
                closes_capture = capture.depth == self.depth;
            }
            if closes_capture {
                if let Some(capture) = self.capture.take() {
                    capture.finish(&mut self.parsed);
                }
            }
        }

        if self.top_depth == Some(self.depth) {
            self.top_depth = None;
        }
    }

    fn text(&mut self, text: &str) {
        if self.skip_depth.is_some() {
            return;
        }
        let Some(capture) = self.capture.as_mut() else {
            return;
        };
        if capture.is_curve() && capture.point_depth.is_none() {
            return;
        }
        capture.text.push_str(text);
    }

    fn reference(&mut self, reference: &BytesRef<'_>) -> Result<(), PresetError> {
        if self.skip_depth.is_some() {
            return Ok(());
        }
        let Some(capture) = self.capture.as_mut() else {
            return Ok(());
        };
        if capture.is_curve() && capture.point_depth.is_none() {
            return Ok(());
        }
        let name = reference
            .decode()
            .map_err(|error| invalid_xml(format!("could not decode an entity: {error}")))?;
        if let Some(character) = reference
            .resolve_char_ref()
            .map_err(|error| invalid_xml(format!("invalid character reference: {error}")))?
        {
            capture.text.push(character);
        } else if let Some(entity) = quick_xml::escape::resolve_predefined_entity(&name) {
            capture.text.push_str(entity);
        } else {
            return Err(invalid_xml(format!("unknown XML entity &{name};")));
        }
        Ok(())
    }

    fn collect_attributes(&mut self, start: &BytesStart<'_>) -> Result<(), PresetError> {
        for attribute in start.attributes() {
            let attribute = attribute
                .map_err(|error| invalid_xml(format!("could not read an attribute: {error}")))?;
            let (local, prefix) = attribute.key.decompose();
            if prefix.is_none_or(|prefix| prefix.as_ref() != b"crs") {
                continue;
            }
            let name = std::str::from_utf8(local.as_ref())
                .map_err(|error| invalid_xml(format!("invalid attribute name: {error}")))?;
            let value = attribute
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map_err(|error| invalid_xml(format!("invalid attribute value: {error}")))?;
            self.parsed
                .attributes
                .insert(name.to_owned(), value.into_owned());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Conversion
// ---------------------------------------------------------------------------

const LIGHT_KEYS: &[&str] = &[
    "Exposure2012",
    "Exposure",
    "Contrast2012",
    "Contrast",
    "Highlights2012",
    "Highlights",
    "Shadows2012",
    "Shadows",
    "Whites2012",
    "Whites",
    "Blacks2012",
    "Blacks",
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

const DETAIL_KEYS: &[&str] = &[
    "Sharpness",
    "SharpenRadius",
    "SharpenDetail",
    "SharpenEdgeMasking",
    "LuminanceSmoothing",
    "LuminanceNoiseReductionDetail",
    "ColorNoiseReduction",
];

const EFFECTS_KEYS: &[&str] = &[
    "Texture",
    "Clarity2012",
    "Clarity",
    "Dehaze",
    "GrainAmount",
    "GrainSize",
    "GrainFrequency",
    "PostCropVignetteAmount",
    "PostCropVignetteMidpoint",
    "PostCropVignetteFeather",
    "PostCropVignetteRoundness",
    "PostCropVignetteStyle",
    "PostCropVignetteHighlightContrast",
];

const HSL_CHANNELS: [(&str, &str, &str); 8] = [
    (
        "HueAdjustmentRed",
        "SaturationAdjustmentRed",
        "LuminanceAdjustmentRed",
    ),
    (
        "HueAdjustmentOrange",
        "SaturationAdjustmentOrange",
        "LuminanceAdjustmentOrange",
    ),
    (
        "HueAdjustmentYellow",
        "SaturationAdjustmentYellow",
        "LuminanceAdjustmentYellow",
    ),
    (
        "HueAdjustmentGreen",
        "SaturationAdjustmentGreen",
        "LuminanceAdjustmentGreen",
    ),
    (
        "HueAdjustmentAqua",
        "SaturationAdjustmentAqua",
        "LuminanceAdjustmentAqua",
    ),
    (
        "HueAdjustmentBlue",
        "SaturationAdjustmentBlue",
        "LuminanceAdjustmentBlue",
    ),
    (
        "HueAdjustmentPurple",
        "SaturationAdjustmentPurple",
        "LuminanceAdjustmentPurple",
    ),
    (
        "HueAdjustmentMagenta",
        "SaturationAdjustmentMagenta",
        "LuminanceAdjustmentMagenta",
    ),
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

const GRAIN_KEYS: &[&str] = &["GrainAmount", "GrainSize", "GrainFrequency"];
const POST_CROP_VIGNETTE_KEYS: &[&str] = &[
    "PostCropVignetteAmount",
    "PostCropVignetteMidpoint",
    "PostCropVignetteFeather",
    "PostCropVignetteRoundness",
    "PostCropVignetteStyle",
    "PostCropVignetteHighlightContrast",
];
const CAMERA_CALIBRATION_KEYS: &[&str] = &[
    "RedHue",
    "RedSaturation",
    "GreenHue",
    "GreenSaturation",
    "BlueHue",
    "BlueSaturation",
];
const GRAY_MIX_KEYS: &[&str] = &[
    "GrayMixerRed",
    "GrayMixerOrange",
    "GrayMixerYellow",
    "GrayMixerGreen",
    "GrayMixerAqua",
    "GrayMixerBlue",
    "GrayMixerPurple",
    "GrayMixerMagenta",
];

struct Converter<'a> {
    attributes: &'a BTreeMap<String, String>,
    name: Option<&'a str>,
    group: Option<&'a str>,
    curves: CurveSet,
    legacy_curves: CurveSet,
    warnings: Vec<String>,
}

impl<'a> Converter<'a> {
    fn new(parsed: &'a ParsedXmp) -> Self {
        Self {
            attributes: &parsed.attributes,
            name: parsed.name.as_deref(),
            group: parsed.group.as_deref(),
            curves: parsed.curves.clone(),
            legacy_curves: parsed.legacy_curves.clone(),
            warnings: Vec::new(),
        }
    }

    fn convert(&mut self, fallback_name: &str) -> Result<Preset, PresetError> {
        let name = self.preset_name(fallback_name);
        let group = self.preset_group();
        let mut exposure = ExposureParams::default();
        let mut groups = AdjustmentGroupSet::EMPTY;
        let mut effects = Vec::new();

        self.convert_light(&mut exposure, &mut groups);
        self.convert_tone_curves(&mut exposure, &mut groups);
        self.convert_color(&mut exposure, &mut groups);
        self.convert_color_mixer(&mut exposure, &mut groups);
        self.convert_color_grading(&mut exposure, &mut groups);
        self.convert_detail(&mut exposure, &mut groups);
        self.convert_effects(&mut exposure, &mut groups, &mut effects);
        self.warn_unsupported_settings();

        if groups.is_empty() {
            let notes = if self.warnings.is_empty() {
                String::new()
            } else {
                format!(" Notes: {}.", self.warnings.join("; "))
            };
            return Err(PresetError::Unsupported(format!(
                "the XMP file contains no Lightroom settings CalibRaw can import.{notes}"
            )));
        }

        let selection = EditSelection {
            adjustment_groups: groups,
            ..EditSelection::default()
        };
        let mut edits = default_edit_state();
        edits.exposure = exposure;
        edits.masks = Arc::new(MaskStack {
            global_effects: effects,
            ..MaskStack::default()
        });
        Preset::new(&name, &group, selection, &edits)
    }

    fn preset_name(&mut self, fallback: &str) -> String {
        let fallback = if fallback.trim().is_empty() {
            "Lightroom preset"
        } else {
            fallback
        };
        let raw = self
            .parsed_name()
            .map(str::to_owned)
            .unwrap_or_else(|| fallback.to_owned());
        let (name, shortened) = sanitize_label(&raw, fallback, MAX_PRESET_NAME_CHARS);
        if shortened {
            self.warn(format!(
                "The preset name was shortened to {MAX_PRESET_NAME_CHARS} characters."
            ));
        }
        name
    }

    fn preset_group(&mut self) -> String {
        let raw = self
            .parsed_group()
            .map(str::to_owned)
            .unwrap_or_else(|| DEFAULT_PRESET_GROUP.to_owned());
        let (group, shortened) = sanitize_label(&raw, DEFAULT_PRESET_GROUP, MAX_PRESET_NAME_CHARS);
        if shortened {
            self.warn(format!(
                "The group name was shortened to {MAX_PRESET_NAME_CHARS} characters."
            ));
        }
        group
    }

    fn parsed_name(&self) -> Option<&str> {
        self.name
            .or_else(|| self.attributes.get("Name").map(String::as_str))
    }

    fn parsed_group(&self) -> Option<&str> {
        self.group
            .or_else(|| self.attributes.get("Group").map(String::as_str))
    }

    fn has(&self, key: &str) -> bool {
        self.attributes.contains_key(key)
    }

    fn has_any(&self, keys: &[&str]) -> bool {
        keys.iter().any(|key| self.has(key))
    }

    fn text(&self, key: &str) -> Option<&str> {
        self.attributes.get(key).map(|value| value.trim())
    }

    fn number(&mut self, key: &str) -> Option<f32> {
        let raw = self.attributes.get(key)?.clone();
        match parse_number(&raw) {
            Some(value) => Some(value),
            None => {
                self.warn(format!("The value “{raw}” of {key} could not be read."));
                None
            }
        }
    }

    fn number_any(&mut self, keys: &[&str]) -> Option<f32> {
        for key in keys {
            if let Some(value) = self.number(key) {
                return Some(value);
            }
        }
        None
    }

    fn boolean(&mut self, key: &str) -> Option<bool> {
        let raw = self.attributes.get(key)?.clone();
        match raw.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => {
                self.warn(format!("The value “{raw}” of {key} could not be read."));
                None
            }
        }
    }

    fn warn(&mut self, message: impl Into<String>) {
        let message = message.into();
        if !self.warnings.contains(&message) {
            self.warnings.push(message);
        }
    }

    fn warn_if_nonzero(&mut self, key: &str, message: &str) {
        if self.number(key).is_some_and(|value| value.abs() > 1e-6) {
            self.warn(message);
        }
    }

    fn convert_light(&mut self, exposure: &mut ExposureParams, groups: &mut AdjustmentGroupSet) {
        if !self.has_any(LIGHT_KEYS) {
            return;
        }
        groups.set(AdjustmentGroup::Light, true);
        if let Some(value) = self.number_any(&["Exposure2012", "Exposure"]) {
            exposure.exposure = value.clamp(-5.0, 5.0);
        }
        if let Some(value) = self.number_any(&["Contrast2012", "Contrast"]) {
            exposure.contrast = value.clamp(-100.0, 100.0);
        }
        if let Some(value) = self.number_any(&["Highlights2012", "Highlights"]) {
            exposure.highlights = value.clamp(-100.0, 100.0);
        }
        if let Some(value) = self.number_any(&["Shadows2012", "Shadows"]) {
            exposure.shadows = value.clamp(-100.0, 100.0);
        }
        if let Some(value) = self.number_any(&["Whites2012", "Whites"]) {
            exposure.whites = value.clamp(-100.0, 100.0);
        }
        if let Some(value) = self.number_any(&["Blacks2012", "Blacks"]) {
            exposure.blacks = value.clamp(-100.0, 100.0);
        }
    }

    fn convert_tone_curves(
        &mut self,
        exposure: &mut ExposureParams,
        groups: &mut AdjustmentGroupSet,
    ) {
        if self.curves.is_empty() && self.legacy_curves.is_empty() {
            return;
        }
        groups.set(AdjustmentGroup::ToneCurve, true);
        let mut warnings = std::mem::take(&mut self.warnings);
        for (channel, label, target) in [
            (CURVE_RGB, "RGB", &mut exposure.tone_curve),
            (CURVE_RED, "red", &mut exposure.tone_curve_red),
            (CURVE_GREEN, "green", &mut exposure.tone_curve_green),
            (CURVE_BLUE, "blue", &mut exposure.tone_curve_blue),
        ] {
            // A modern curve wins per channel, so a file that mixes layouts
            // still keeps the channels only the legacy layout has.
            if let Some(raw_points) = self
                .curves
                .get(channel)
                .or_else(|| self.legacy_curves.get(channel))
            {
                match parse_curve(raw_points, &mut warnings) {
                    Some(points) => *target = point_curve_from_points(&points),
                    None => push_warning(
                        &mut warnings,
                        format!("The {label} tone curve could not be read; it was left linear."),
                    ),
                }
            }
        }
        self.warnings = warnings;
    }

    fn convert_color(&mut self, exposure: &mut ExposureParams, groups: &mut AdjustmentGroupSet) {
        if !self.has_any(COLOR_KEYS) {
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
        if self.boolean("ConvertToGrayscale") == Some(true) {
            exposure.saturation = -100.0;
            if GRAY_MIX_KEYS
                .iter()
                .any(|key| self.number(key).is_some_and(|value| value.abs() > 1e-6))
            {
                self.warn(
                    "Grayscale mix settings were not imported; the preset is imported as a black-and-white conversion.",
                );
            }
        }

        self.warn_if_nonzero(
            "IncrementalTemperature",
            "Incremental white balance adjustments were not imported.",
        );
        self.warn_if_nonzero(
            "IncrementalTint",
            "Incremental white balance adjustments were not imported.",
        );
        self.warn_if_nonzero("ShadowTint", "The legacy shadow tint was not imported.");
    }

    fn white_balance(&mut self) -> WhiteBalance {
        let Some(value) = self.text("WhiteBalance").map(str::to_owned) else {
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
            self.warn("A custom white balance without temperature and tint was left as shot.");
        } else {
            self.warn(format!(
                "The white balance “{value}” is not supported; it was left as shot."
            ));
        }
        WhiteBalance::AsShot
    }

    fn convert_color_mixer(
        &mut self,
        exposure: &mut ExposureParams,
        groups: &mut AdjustmentGroupSet,
    ) {
        if !HSL_CHANNELS.iter().any(|(hue, saturation, luminance)| {
            self.has(hue) || self.has(saturation) || self.has(luminance)
        }) {
            return;
        }
        groups.set(AdjustmentGroup::ColorMixer, true);
        for (index, (hue, saturation, luminance)) in HSL_CHANNELS.iter().enumerate() {
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
        let has_modern = self.has_any(MODERN_COLOR_GRADING_KEYS);
        let has_legacy = self.has_any(LEGACY_SPLIT_TONING_KEYS);
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
        } else if !has_modern && has_legacy {
            // Lightroom renders a legacy split-toning preset with Blending at
            // 100 so its two tints cover the tonal ranges like split toning.
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
            hue: self.number_any(hue_keys).unwrap_or(0.0).rem_euclid(360.0),
            saturation: self
                .number_any(saturation_keys)
                .unwrap_or(0.0)
                .clamp(0.0, 100.0),
            luminance: self
                .number_any(luminance_keys)
                .unwrap_or(0.0)
                .clamp(-100.0, 100.0),
        }
    }

    fn convert_detail(&mut self, exposure: &mut ExposureParams, groups: &mut AdjustmentGroupSet) {
        if !self.has_any(DETAIL_KEYS) {
            return;
        }
        groups.set(AdjustmentGroup::Detail, true);
        if let Some(value) = self.number("Sharpness") {
            exposure.sharpen_amount = value.clamp(0.0, 150.0);
        }
        if let Some(value) = self.number("SharpenRadius") {
            exposure.sharpen_radius = value.clamp(0.5, 3.0);
        }
        if let Some(value) = self.number("SharpenDetail") {
            exposure.sharpen_detail = value.clamp(0.0, 100.0);
        }
        if let Some(value) = self.number("SharpenEdgeMasking") {
            exposure.sharpen_masking = value.clamp(0.0, 100.0);
        }
        if let Some(value) = self.number("LuminanceSmoothing") {
            exposure.luminance_denoise = value.clamp(0.0, 100.0);
        }
        if let Some(value) = self.number("ColorNoiseReduction") {
            exposure.chroma_denoise = (value / 100.0).clamp(0.0, 1.0);
        }
        if let Some(value) = self.number("LuminanceNoiseReductionDetail") {
            exposure.denoise_detail = value.clamp(0.0, 100.0);
        }
    }

    fn convert_effects(
        &mut self,
        exposure: &mut ExposureParams,
        groups: &mut AdjustmentGroupSet,
        effects: &mut Vec<EffectComponent>,
    ) {
        if !self.has_any(EFFECTS_KEYS) {
            return;
        }
        groups.set(AdjustmentGroup::Effects, true);
        if let Some(value) = self.number("Texture") {
            exposure.texture = value.clamp(-100.0, 100.0);
        }
        if let Some(value) = self.number_any(&["Clarity2012", "Clarity"]) {
            exposure.clarity = value.clamp(-100.0, 100.0);
        }
        if let Some(value) = self.number("Dehaze") {
            exposure.dehaze = value.clamp(-100.0, 100.0);
        }

        if GRAIN_KEYS.iter().any(|key| self.has(key)) {
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

        if POST_CROP_VIGNETTE_KEYS.iter().any(|key| self.has(key)) {
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
                .map(grain_size_from_lightroom)
                .unwrap_or(defaults.size),
            roughness: self
                .number("GrainFrequency")
                .unwrap_or(defaults.roughness)
                .clamp(0.0, 100.0),
            ..defaults
        }
    }

    fn vignette_settings(&mut self) -> VignetteEffectSettings {
        // Lightroom's defaults differ from CalibRaw's, so a preset that omits
        // a slider must not inherit CalibRaw's stronger default vignette.
        let mut settings = VignetteEffectSettings {
            amount: 0.0,
            midpoint: 50.0,
            roundness: 0.0,
            feather: 50.0,
            highlights: 0.0,
            center: [50.0, 50.0],
        };
        if let Some(value) = self.number("PostCropVignetteAmount") {
            settings.amount = value.clamp(-100.0, 100.0);
        }
        if let Some(value) = self.number("PostCropVignetteMidpoint") {
            settings.midpoint = value.clamp(0.0, 100.0);
        }
        if let Some(value) = self.number("PostCropVignetteFeather") {
            settings.feather = value.clamp(0.0, 100.0);
        }
        if let Some(value) = self.number("PostCropVignetteRoundness") {
            settings.roundness = value.clamp(-100.0, 100.0);
        }
        if let Some(value) = self.number("PostCropVignetteHighlightContrast") {
            settings.highlights = value.clamp(0.0, 100.0);
        }
        settings.center = [50.0, 50.0];
        settings
    }

    fn warn_unsupported_settings(&mut self) {
        self.warn_lens_corrections();
        self.warn_perspective();
        self.warn_defringe();
        self.warn_parametric_curve();
        self.warn_process_version();
        self.warn_camera_profile();
        self.warn_camera_calibration();
        self.warn_noise_reduction_detail();
        self.warn_vignette_style();
        if self.boolean("HasCrop") == Some(true) {
            self.warn("Crop settings were not imported.");
        }
    }

    fn warn_lens_corrections(&mut self) {
        // A profile name can stay in the file while the profile is disabled;
        // only an enabled profile changes the photo.
        let profile_changed = match self.number("LensProfileEnable") {
            Some(enabled) => enabled != 0.0,
            None => {
                let setup_changed = self.text("LensProfileSetup").is_some_and(|value| {
                    !value.is_empty() && !value.eq_ignore_ascii_case("LensProfileDefault")
                });
                let name_changed = self
                    .text("LensProfileName")
                    .is_some_and(|value| !value.is_empty());
                let scale_changed = self
                    .number("LensProfileDistortionScale")
                    .is_some_and(|value| (value - 100.0).abs() > 1e-6)
                    || self
                        .number("LensProfileChromaticAberrationScale")
                        .is_some_and(|value| (value - 100.0).abs() > 1e-6)
                    || self
                        .number("LensProfileVignettingScale")
                        .is_some_and(|value| (value - 100.0).abs() > 1e-6);
                setup_changed || name_changed || scale_changed
            }
        };
        let manual_changed = self
            .number("LensManualDistortionAmount")
            .is_some_and(|value| value.abs() > 1e-6)
            || self
                .number("AutoLateralCA")
                .is_some_and(|value| value != 0.0)
            || self
                .number("VignetteAmount")
                .is_some_and(|value| value.abs() > 1e-6);
        if profile_changed || manual_changed {
            self.warn("Lens corrections were not imported.");
        }
    }

    fn warn_perspective(&mut self) {
        let changed = self
            .number("PerspectiveUpright")
            .is_some_and(|value| value != 0.0)
            || self
                .number("PerspectiveVertical")
                .is_some_and(|value| value.abs() > 1e-6)
            || self
                .number("PerspectiveHorizontal")
                .is_some_and(|value| value.abs() > 1e-6)
            || self
                .number("PerspectiveRotate")
                .is_some_and(|value| value.abs() > 1e-6)
            || self
                .number("PerspectiveAspect")
                .is_some_and(|value| value.abs() > 1e-6)
            || self
                .number("PerspectiveScale")
                .is_some_and(|value| (value - 100.0).abs() > 1e-6)
            || self
                .number("PerspectiveX")
                .is_some_and(|value| value.abs() > 1e-6)
            || self
                .number("PerspectiveY")
                .is_some_and(|value| value.abs() > 1e-6);
        if changed {
            self.warn("Perspective corrections were not imported.");
        }
    }

    fn warn_defringe(&mut self) {
        let changed = self
            .number("DefringePurpleAmount")
            .is_some_and(|value| value.abs() > 1e-6)
            || self
                .number("DefringeGreenAmount")
                .is_some_and(|value| value.abs() > 1e-6);
        if changed {
            self.warn("Defringing was not imported.");
        }
    }

    fn warn_parametric_curve(&mut self) {
        let changed = [
            "ParametricShadows",
            "ParametricDarks",
            "ParametricLights",
            "ParametricHighlights",
        ]
        .iter()
        .any(|key| self.number(key).is_some_and(|value| value.abs() > 1e-6))
            || self
                .number("ParametricShadowSplit")
                .is_some_and(|value| (value - 25.0).abs() > 1e-6)
            || self
                .number("ParametricMidtoneSplit")
                .is_some_and(|value| (value - 50.0).abs() > 1e-6)
            || self
                .number("ParametricHighlightSplit")
                .is_some_and(|value| (value - 75.0).abs() > 1e-6);
        if changed {
            self.warn("The parametric tone curve was not imported.");
        }
    }

    fn warn_process_version(&mut self) {
        if let Some(version) = self.number("ProcessVersion") {
            if version < 11.0 {
                self.warn(format!(
                    "Process version {version} is older than 11; the preset may look different."
                ));
            }
        }
    }

    fn warn_camera_profile(&mut self) {
        if let Some(profile) = self.text("CameraProfile") {
            if !profile.is_empty() && !profile.eq_ignore_ascii_case("Embedded") {
                self.warn(format!("Camera profile “{profile}” was not imported."));
            }
        }
    }

    fn warn_camera_calibration(&mut self) {
        if CAMERA_CALIBRATION_KEYS
            .iter()
            .any(|key| self.number(key).is_some_and(|value| value.abs() > 1e-6))
        {
            self.warn("Camera calibration color adjustments were not imported.");
        }
    }

    fn warn_noise_reduction_detail(&mut self) {
        // CalibRaw's single Denoise Detail slider follows Lightroom's
        // luminance detail; chroma detail, chroma smoothness and luminance
        // contrast have no equivalent.
        let changed = self
            .number("ColorNoiseReductionDetail")
            .is_some_and(|value| (value - 50.0).abs() > 1e-6)
            || self
                .number("ColorNoiseReductionSmoothness")
                .is_some_and(|value| (value - 50.0).abs() > 1e-6)
            || self
                .number("LuminanceNoiseReductionContrast")
                .is_some_and(|value| value.abs() > 1e-6);
        if changed {
            self.warn("Some noise reduction detail settings were not imported.");
        }
    }

    fn warn_vignette_style(&mut self) {
        // CalibRaw's vignette preserves highlights like Lightroom's default
        // Highlight Priority style (0); Color Priority and Paint Overlay do not
        // map onto it.
        if self
            .number("PostCropVignetteStyle")
            .is_some_and(|value| value.abs() > 1e-6)
        {
            self.warn("The post-crop vignette style was not imported.");
        }
    }
}

fn sanitize_label(text: &str, fallback: &str, max_chars: usize) -> (String, bool) {
    let mut cleaned = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_control() {
            cleaned.push(' ');
        } else {
            cleaned.push(character);
        }
    }
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
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

fn parse_number(raw: &str) -> Option<f32> {
    let raw = raw.trim();
    let raw = raw.strip_prefix('+').unwrap_or(raw);
    raw.parse::<f32>().ok().filter(|value| value.is_finite())
}

fn push_warning(warnings: &mut Vec<String>, message: impl Into<String>) {
    let message = message.into();
    if !warnings.contains(&message) {
        warnings.push(message);
    }
}

fn parse_curve(raw_points: &[String], warnings: &mut Vec<String>) -> Option<Vec<[f32; 2]>> {
    let mut points = Vec::with_capacity(raw_points.len());
    for raw in raw_points {
        let Some((x, y)) = raw.split_once(',') else {
            push_warning(
                warnings,
                format!("The tone curve point “{}” could not be read.", raw.trim()),
            );
            continue;
        };
        let (Some(x), Some(y)) = (parse_number(x), parse_number(y)) else {
            push_warning(
                warnings,
                format!("The tone curve point “{}” could not be read.", raw.trim()),
            );
            continue;
        };
        if !(0.0..=255.0).contains(&x) || !(0.0..=255.0).contains(&y) {
            push_warning(
                warnings,
                format!(
                    "The tone curve point “{}” is outside the 0–255 range and was skipped.",
                    raw.trim()
                ),
            );
            continue;
        }
        points.push([x / 255.0, y / 255.0]);
    }
    points.sort_by(|left, right| left[0].total_cmp(&right[0]));
    points.dedup_by(|left, right| (left[0] - right[0]).abs() < f32::EPSILON);
    if points.len() < 2 {
        return None;
    }
    if points.len() > MAX_POINT_CURVE_POINTS {
        push_warning(
            warnings,
            format!(
                "The tone curve has {} points; it was reduced to {MAX_POINT_CURVE_POINTS}.",
                points.len()
            ),
        );
        points = resample_curve(&points, MAX_POINT_CURVE_POINTS);
    }
    Some(points)
}

fn resample_curve(points: &[[f32; 2]], max_points: usize) -> Vec<[f32; 2]> {
    if points.len() <= max_points {
        return points.to_vec();
    }
    let first_x = points[0][0];
    let last_x = points[points.len() - 1][0];
    let span = last_x - first_x;
    (0..max_points)
        .map(|index| {
            let fraction = index as f32 / (max_points - 1) as f32;
            let x = first_x + span * fraction;
            [x, interpolate_curve(points, x)]
        })
        .collect()
}

fn interpolate_curve(points: &[[f32; 2]], x: f32) -> f32 {
    if x <= points[0][0] {
        return points[0][1];
    }
    if x >= points[points.len() - 1][0] {
        return points[points.len() - 1][1];
    }
    for pair in points.windows(2) {
        let [x0, y0] = pair[0];
        let [x1, y1] = pair[1];
        if x <= x1 {
            let width = x1 - x0;
            let fraction = if width.abs() < f32::EPSILON {
                0.0
            } else {
                (x - x0) / width
            };
            return y0 + (y1 - y0) * fraction;
        }
    }
    points[points.len() - 1][1]
}

fn point_curve_from_points(points: &[[f32; 2]]) -> PointCurve {
    let mut curve = PointCurve::linear();
    curve.points = [[1.0, 1.0]; MAX_POINT_CURVE_POINTS];
    let len = points.len().min(MAX_POINT_CURVE_POINTS);
    for (slot, point) in curve.points.iter_mut().zip(&points[..len]) {
        *slot = *point;
    }
    curve.len = len as u32;
    curve.sanitize();
    curve
}

/// Maps Lightroom's 0–100 grain size to CalibRaw's 0.5–4.0. Lightroom's
/// default 25 lands on CalibRaw's default 1.0.
fn grain_size_from_lightroom(value: f32) -> f32 {
    let value = value.clamp(0.0, 100.0);
    if value <= 25.0 {
        0.5 + value / 25.0 * 0.5
    } else {
        1.0 + (value - 25.0) / 75.0 * 3.0
    }
}

#[cfg(test)]
mod tests;
