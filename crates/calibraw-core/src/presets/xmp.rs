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
//! [`Preset`] whenever presets load. Settings without an equivalent (lens
//! corrections, perspective, defringing, parametric curves, camera
//! calibration and profiles) are reported as warnings instead of failing the
//! import, and stay in the XMP for a later version to translate. White
//! balance is absolute in CalibRaw, so named white balances use their usual
//! temperature and tint.
//!
//! Subject, background and sky masks are carried over without their raster;
//! CalibRaw regenerates them on the target photo. Masks with other components
//! are skipped with a warning, as are local adjustments without an equivalent.
//!
//! XMP writers use the conventional `rdf` and `crs` prefixes, so elements and
//! attributes are matched by prefix rather than by resolved namespace.

use super::{
    read_preset_bytes, Preset, PresetError, DEFAULT_PRESET_GROUP, MAX_PRESET_BYTES,
    MAX_PRESET_NAME_CHARS,
};
use crate::pipeline::{
    AdjustmentGroup, AdjustmentGroupSet, ColorGradeWheel, ColorGrading, EffectComponent,
    ExposureParams, GrainEffectSettings, LocalAdjustments, LocalMask, MaskCombineMode,
    MaskComponent, MaskEffect, MaskEffectSettings, MaskKind, MaskStack, PointCurve,
    VignetteEffectSettings, WhiteBalance, HSL_HUE_LIMIT, HUE_ROTATION_LIMIT_DEGREES,
    MAX_LOCAL_MASKS, MAX_MASK_COMPONENTS, MAX_POINT_CURVE_POINTS, MAX_TEMPERATURE_KELVIN,
    MAX_WHITE_BALANCE_TINT, MIN_TEMPERATURE_KELVIN, MIN_WHITE_BALANCE_TINT,
};
use crate::sidecar::{default_edit_state, EditSelection};
use quick_xml::escape::{escape, resolve_predefined_entity};
use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use std::collections::BTreeMap;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

/// The file extension of an XMP preset.
pub const XMP_PRESET_SUFFIX: &str = ".xmp";

const CRS_NAMESPACE: &str = "http://ns.adobe.com/camera-raw-settings/1.0/";
const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
/// Real presets nest about a dozen elements deep; the limit keeps a hostile
/// file from building an arbitrarily deep tree.
const MAX_ELEMENT_DEPTH: usize = 64;
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
        warnings: converter.warnings.0,
    })
}

/// Rewrites the preset name and group stored in `bytes`, leaving every other
/// byte of the XMP untouched. A label the document lacks is added to its
/// first description. Used when a copied XMP preset is renamed, moved
/// between groups, or imported under a unique name.
pub(crate) fn rewrite_xmp_labels(
    bytes: &[u8],
    name: &str,
    group: &str,
) -> Result<Vec<u8>, PresetError> {
    let (bom, text) = split_bom(bytes)?;
    let root = parse_document(text)?;
    let document = Document::new(&root)?;

    let mut edits = Vec::new();
    let mut missing = String::new();
    for (key, label) in [("crs:Name", name), ("crs:Group", group)] {
        if !document.label_edits(text, key, label, &mut edits) {
            missing.push_str(&format!(" {key}=\"{}\"", escape(label)));
        }
    }
    if !missing.is_empty() {
        if !document.crs_declared {
            missing.push_str(&format!(" xmlns:crs=\"{CRS_NAMESPACE}\""));
        }
        let tag = &document.descriptions[0].start_tag;
        let closing = if text[tag.clone()].ends_with("/>") {
            2
        } else {
            1
        };
        let at = tag.end - closing;
        edits.push((at..at, missing));
    }
    edits.sort_by_key(|(range, _)| range.start);

    let mut output = Vec::with_capacity(bytes.len() + 64);
    output.extend_from_slice(bom);
    let source = text.as_bytes();
    let mut cursor = 0;
    for (range, replacement) in edits {
        output.extend_from_slice(&source[cursor..range.start]);
        output.extend_from_slice(replacement.as_bytes());
        cursor = range.end;
    }
    output.extend_from_slice(&source[cursor..]);
    if output.len() as u64 > MAX_PRESET_BYTES {
        return Err(PresetError::TooLarge(output.len() as u64));
    }
    Ok(output)
}

/// Splits `bytes` into its UTF-8 byte-order mark, if any, and the text.
fn split_bom(bytes: &[u8]) -> Result<(&[u8], &str), PresetError> {
    if bytes.len() as u64 > MAX_PRESET_BYTES {
        return Err(PresetError::TooLarge(bytes.len() as u64));
    }
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return Err(PresetError::Unsupported(
            "UTF-16 XMP presets are not supported; save the preset as UTF-8".to_owned(),
        ));
    }
    let bom_len = if bytes.starts_with(UTF8_BOM) {
        UTF8_BOM.len()
    } else {
        0
    };
    let (bom, rest) = bytes.split_at(bom_len);
    let text = std::str::from_utf8(rest)
        .map_err(|_| PresetError::Unsupported("the XMP preset is not valid UTF-8".to_owned()))?;
    Ok((bom, text))
}

fn invalid_xml(message: impl Into<String>) -> PresetError {
    PresetError::Invalid(format!("invalid XMP: {}", message.into()))
}

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

/// One element of an XMP document.
#[derive(Default)]
struct Element {
    /// The qualified name, such as `crs:Name`.
    name: String,
    /// Attributes by qualified name, with entities resolved.
    attributes: Vec<(String, String)>,
    children: Vec<Element>,
    /// The element's own character data, with entities resolved.
    text: String,
    /// Byte range of the start tag, `<…>` or `<…/>`.
    start_tag: Range<usize>,
    /// Byte range between the start and end tags; `None` for `<…/>`.
    content: Option<Range<usize>>,
}

impl Element {
    fn open(start: &BytesStart<'_>, start_tag: Range<usize>) -> Result<Self, PresetError> {
        let attributes = start
            .attributes()
            .map(|attribute| {
                let attribute = attribute.map_err(|error| {
                    invalid_xml(format!("could not read an attribute: {error}"))
                })?;
                let value = attribute
                    .normalized_value(XmlVersion::Implicit1_0)
                    .map_err(|error| invalid_xml(format!("invalid attribute value: {error}")))?;
                Ok((utf8(attribute.key.as_ref())?.to_owned(), value.into_owned()))
            })
            .collect::<Result<_, PresetError>>()?;
        Ok(Self {
            name: utf8(start.name().as_ref())?.to_owned(),
            attributes,
            start_tag,
            ..Self::default()
        })
    }

    fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|child| child.name == name)
    }

    fn children_named(&self, name: &'static str) -> impl Iterator<Item = &Element> + '_ {
        self.children.iter().filter(move |child| child.name == name)
    }

    /// The fields of a struct value: the `rdf:Description` inside a property
    /// or list item, or the element itself when the fields are written on it.
    fn resource(&self) -> &Element {
        self.child("rdf:Description").unwrap_or(self)
    }

    /// The items of an `rdf:Seq` property.
    fn seq_items(&self) -> impl Iterator<Item = &Element> + '_ {
        self.child("rdf:Seq")
            .into_iter()
            .flat_map(|seq| seq.children_named("rdf:li"))
    }

    /// The text of a language-alternative property: its default-language
    /// item, else its first item. A plain text property is accepted too.
    fn alt_text(&self) -> Option<&str> {
        let text = match self.child("rdf:Alt") {
            Some(alt) => {
                &alt.children_named("rdf:li")
                    .find(|item| item.attribute("xml:lang") == Some("x-default"))
                    .or_else(|| alt.child("rdf:li"))?
                    .text
            }
            None => &self.text,
        };
        Some(text.trim()).filter(|text| !text.is_empty())
    }
}

fn utf8(bytes: &[u8]) -> Result<&str, PresetError> {
    std::str::from_utf8(bytes).map_err(|_| invalid_xml("a name is not valid UTF-8"))
}

/// Parses `text` into a tree under an unnamed root element.
fn parse_document(text: &str) -> Result<Element, PresetError> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(false);
    let mut root = Element::default();
    // The open elements below the root, outermost first.
    let mut open: Vec<Element> = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event()
            .map_err(|error| invalid_xml(error.to_string()))?;
        let after = reader.buffer_position() as usize;
        let innermost = open.last_mut().unwrap_or(&mut root);
        match event {
            Event::Start(start) => {
                if open.len() >= MAX_ELEMENT_DEPTH {
                    return Err(invalid_xml("the document is nested too deeply"));
                }
                open.push(Element::open(&start, before..after)?);
            }
            Event::Empty(start) => innermost
                .children
                .push(Element::open(&start, before..after)?),
            Event::End(_) => {
                let mut element = open
                    .pop()
                    .ok_or_else(|| invalid_xml("unexpected closing tag"))?;
                element.content = Some(element.start_tag.end..before);
                open.last_mut().unwrap_or(&mut root).children.push(element);
            }
            Event::Text(text) => innermost.text.push_str(
                &text
                    .decode()
                    .map_err(|error| invalid_xml(format!("could not decode text: {error}")))?,
            ),
            Event::CData(cdata) => innermost.text.push_str(
                &cdata
                    .decode()
                    .map_err(|error| invalid_xml(format!("could not decode text: {error}")))?,
            ),
            Event::GeneralRef(reference) => {
                innermost.text.push_str(&decoded_reference(&reference)?);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if let Some(element) = open.last() {
        return Err(invalid_xml(format!("<{}> is not closed", element.name)));
    }
    Ok(root)
}

fn decoded_reference(reference: &BytesRef<'_>) -> Result<String, PresetError> {
    if let Some(character) = reference
        .resolve_char_ref()
        .map_err(|error| invalid_xml(format!("invalid character reference: {error}")))?
    {
        return Ok(character.to_string());
    }
    let name = reference
        .decode()
        .map_err(|error| invalid_xml(format!("could not decode an entity: {error}")))?;
    resolve_predefined_entity(&name)
        .map(str::to_owned)
        .ok_or_else(|| invalid_xml(format!("unknown XML entity &{name};")))
}

/// The top-level descriptions of an XMP document, which hold the preset's
/// own properties.
struct Document<'a> {
    descriptions: Vec<&'a Element>,
    /// Whether `xmlns:crs` is in scope on the first description.
    crs_declared: bool,
}

impl<'a> Document<'a> {
    fn new(root: &'a Element) -> Result<Self, PresetError> {
        // `x:xmpmeta` and `rdf:RDF` wrap the descriptions; both are optional.
        let mut parent = root;
        let mut crs_declared = false;
        for wrapper in ["x:xmpmeta", "rdf:RDF"] {
            if let Some(inner) = parent.child(wrapper) {
                parent = inner;
                crs_declared |= declares_crs(inner);
            }
        }
        let descriptions: Vec<&Element> = parent.children_named("rdf:Description").collect();
        let first = descriptions
            .first()
            .ok_or_else(|| invalid_xml("no rdf:Description was found"))?;
        crs_declared |= declares_crs(first);
        Ok(Self {
            descriptions,
            crs_declared,
        })
    }

    /// The elements that hold settings: each description and its
    /// `crs:Parameters`.
    fn settings(&self) -> Vec<&'a Element> {
        self.descriptions
            .iter()
            .flat_map(|&description| {
                std::iter::once(description)
                    .chain(description.child("crs:Parameters").map(Element::resource))
            })
            .collect()
    }

    fn properties(&self) -> Properties<'a> {
        let mut properties = Properties::default();
        for element in self.settings() {
            properties.extend(element);
        }
        properties
    }

    /// The first settings property written as an element, such as a curve.
    fn property_element(&self, key: &str) -> Option<&'a Element> {
        self.settings()
            .into_iter()
            .find_map(|element| element.child(key))
    }

    /// The local adjustment groups, called corrections in XMP.
    fn corrections(&self) -> Vec<&'a Element> {
        self.settings()
            .into_iter()
            .filter_map(|element| element.child("crs:MaskGroupBasedCorrections"))
            .flat_map(|corrections| corrections.seq_items())
            .map(Element::resource)
            .collect()
    }

    /// The preset label `key` (`crs:Name` or `crs:Group`). An element wins
    /// over an attribute, as XMP writers put the localized label there.
    fn label(&self, key: &'static str) -> Option<&'a str> {
        let element = self
            .descriptions
            .iter()
            .find_map(|&description| description.children_named(key).find_map(Element::alt_text));
        element.or_else(|| {
            self.descriptions.iter().find_map(|&description| {
                description
                    .attribute(key)
                    .map(str::trim)
                    .filter(|label| !label.is_empty())
            })
        })
    }

    /// Adds to `edits` the replacements that set every occurrence of the
    /// label `key` to `label`. Returns whether the document has the label.
    fn label_edits(
        &self,
        text: &str,
        key: &'static str,
        label: &str,
        edits: &mut Vec<(Range<usize>, String)>,
    ) -> bool {
        let escaped = escape(label);
        let mut found = false;
        for &description in &self.descriptions {
            if let Some(current) = description.attribute(key) {
                found = true;
                let tag = &description.start_tag;
                if current != label {
                    if let Some(value) = attribute_value_span(&text[tag.clone()], key) {
                        let span = tag.start + value.start..tag.start + value.end;
                        edits.push((span, escaped.clone().into_owned()));
                    }
                }
            }
            for element in description.children_named(key) {
                found = true;
                if element.alt_text() == Some(label) {
                    continue;
                }
                let alt = format!(
                    r#"<rdf:Alt><rdf:li xml:lang="x-default">{escaped}</rdf:li></rdf:Alt>"#
                );
                match &element.content {
                    Some(content) => edits.push((content.clone(), alt)),
                    None => {
                        // `<crs:Name/>` becomes `<crs:Name>…</crs:Name>`.
                        let tag = &text[element.start_tag.clone()];
                        let open = tag.strip_suffix("/>").unwrap_or(tag);
                        edits.push((element.start_tag.clone(), format!("{open}>{alt}</{key}>")));
                    }
                }
            }
        }
        found
    }
}

fn declares_crs(element: &Element) -> bool {
    element.attribute("xmlns:crs").is_some()
}

/// The byte range of the value of attribute `key` within `tag`, the source
/// of a start tag the XML reader has accepted.
fn attribute_value_span(tag: &str, key: &str) -> Option<Range<usize>> {
    let bytes = tag.as_bytes();
    // Past `<` and the element name.
    let mut cursor = bytes.iter().position(u8::is_ascii_whitespace)?;
    loop {
        cursor += bytes[cursor..]
            .iter()
            .position(|byte| !byte.is_ascii_whitespace())?;
        let name_end = cursor
            + bytes[cursor..]
                .iter()
                .position(|byte| *byte == b'=' || byte.is_ascii_whitespace())?;
        let quote_at = name_end
            + bytes[name_end..]
                .iter()
                .position(|byte| matches!(byte, b'"' | b'\''))?;
        let value_start = quote_at + 1;
        let value_end = value_start
            + bytes[value_start..]
                .iter()
                .position(|byte| *byte == bytes[quote_at])?;
        if &tag[cursor..name_end] == key {
            return Some(value_start..value_end);
        }
        cursor = value_end + 1;
    }
}

/// The `crs:` attributes of the elements read so far, by local name. Later
/// elements override earlier ones.
#[derive(Default)]
struct Properties<'a>(BTreeMap<&'a str, &'a str>);

impl<'a> Properties<'a> {
    fn of(element: &'a Element) -> Self {
        let mut properties = Self::default();
        properties.extend(element);
        properties
    }

    fn extend(&mut self, element: &'a Element) {
        for (name, value) in &element.attributes {
            if let Some(key) = name.strip_prefix("crs:") {
                self.0.insert(key, value);
            }
        }
    }

    fn has(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    fn has_any(&self, keys: &[&str]) -> bool {
        keys.iter().any(|key| self.has(key))
    }

    fn text(&self, key: &str) -> Option<&'a str> {
        self.0
            .get(key)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
    }

    /// The number stored as `key`, or `None` when it is missing or unreadable.
    fn value(&self, key: &str) -> Option<f32> {
        self.0.get(key).and_then(|raw| parse_number(raw))
    }

    /// Like [`Self::value`], but reports an unreadable value.
    fn number(&self, key: &str, warnings: &mut Warnings) -> Option<f32> {
        let raw = self.0.get(key)?;
        let value = parse_number(raw);
        if value.is_none() {
            warnings.push(format!("The value “{raw}” of {key} could not be read."));
        }
        value
    }

    /// The first readable number among `keys`, such as a current key and
    /// its legacy name.
    fn first_number(&self, keys: &[&str], warnings: &mut Warnings) -> Option<f32> {
        keys.iter().find_map(|key| self.number(key, warnings))
    }

    /// Whether `key` holds a number other than `neutral`.
    fn differs(&self, key: &str, neutral: f32) -> bool {
        self.value(key)
            .is_some_and(|value| (value - neutral).abs() > 1e-6)
    }

    fn boolean(&self, key: &str) -> Option<bool> {
        match self.0.get(key)?.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        }
    }
}

/// Warnings in the order they arose, without duplicates.
#[derive(Default)]
struct Warnings(Vec<String>);

impl Warnings {
    fn push(&mut self, message: impl Into<String>) {
        let message = message.into();
        if !self.0.contains(&message) {
            self.0.push(message);
        }
    }
}

// ---------------------------------------------------------------------------
// Conversion
// ---------------------------------------------------------------------------

const EXPOSURE_KEYS: &[&str] = &["Exposure2012", "Exposure"];
/// The -100..100 light sliders, each as its current and legacy key.
const TONE_SLIDER_KEYS: [[&str; 2]; 5] = [
    ["Contrast2012", "Contrast"],
    ["Highlights2012", "Highlights"],
    ["Shadows2012", "Shadows"],
    ["Whites2012", "Whites"],
    ["Blacks2012", "Blacks"],
];
/// The curves CalibRaw stores, in its channel order, as the current key, the
/// legacy key and a label.
const TONE_CURVES: [(&str, &str, &str); 4] = [
    ("crs:ToneCurvePV2012", "crs:ToneCurve", "RGB"),
    ("crs:ToneCurvePV2012Red", "crs:ToneCurveRed", "red"),
    ("crs:ToneCurvePV2012Green", "crs:ToneCurveGreen", "green"),
    ("crs:ToneCurvePV2012Blue", "crs:ToneCurveBlue", "blue"),
];
const LOCAL_TONE_CURVES: [(&str, &str); 4] = [
    ("crs:MainCurve", "RGB"),
    ("crs:RedCurve", "red"),
    ("crs:GreenCurve", "green"),
    ("crs:BlueCurve", "blue"),
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

const LENS_WARNING: &str = "Lens corrections were not imported.";
/// Settings without an equivalent, each with its neutral value, and the
/// warning shown when any of them is away from neutral.
const UNSUPPORTED_SETTINGS: &[(&[(&str, f32)], &str)] = &[
    (
        &[
            ("LensManualDistortionAmount", 0.0),
            ("AutoLateralCA", 0.0),
            ("VignetteAmount", 0.0),
        ],
        LENS_WARNING,
    ),
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
            ("ParametricShadows", 0.0),
            ("ParametricDarks", 0.0),
            ("ParametricLights", 0.0),
            ("ParametricHighlights", 0.0),
            ("ParametricShadowSplit", 25.0),
            ("ParametricMidtoneSplit", 50.0),
            ("ParametricHighlightSplit", 75.0),
        ],
        "The parametric tone curve was not imported.",
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
/// Local adjustments without an equivalent, with their neutral values.
const UNSUPPORTED_LOCAL_SETTINGS: &[(&str, f32)] = &[
    ("LocalSharpness", 0.0),
    ("LocalLuminanceNoise", 0.0),
    ("LocalMoire", 0.0),
    ("LocalDefringe", 0.0),
    ("LocalGrain", 0.0),
    ("LocalBrightness", 0.0),
    ("LocalToningHue", 0.0),
    ("LocalToningSaturation", 0.0),
    ("LocalCorrectedDepth", 0.0),
    ("LocalCurveRefineSaturation", 100.0),
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
        self.convert_color_mixer(&mut exposure, &mut groups);
        self.convert_color_grading(&mut exposure, &mut groups);
        self.convert_detail(&mut exposure, &mut groups);
        self.convert_effects(&mut exposure, &mut groups, &mut effects);
        let masks = self.convert_masks(document);
        self.warn_unsupported_settings();

        if groups.is_empty() && masks.is_empty() {
            self.warnings
                .push("No settings in this preset can be translated yet.");
        }

        let selection = EditSelection {
            adjustment_groups: groups,
            ai_masks: !masks.is_empty(),
            ..EditSelection::default()
        };
        let mut edits = default_edit_state();
        edits.exposure = exposure;
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
        let curves = TONE_CURVES.map(|(current, legacy, _)| {
            document
                .property_element(current)
                .or_else(|| document.property_element(legacy))
        });
        if curves.iter().all(Option::is_none) {
            return;
        }
        groups.set(AdjustmentGroup::ToneCurve, true);
        let targets = [
            &mut exposure.tone_curve,
            &mut exposure.tone_curve_red,
            &mut exposure.tone_curve_green,
            &mut exposure.tone_curve_blue,
        ];
        for ((curve, (_, _, label)), target) in curves.iter().zip(TONE_CURVES).zip(targets) {
            let Some(curve) = curve else {
                continue;
            };
            if let Some(curve) = read_curve(
                curve,
                &format!("The {label} tone curve"),
                &mut self.warnings,
            ) {
                *target = curve;
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
        if !keys.iter().flatten().any(|key| self.properties.has(key)) {
            return;
        }
        groups.set(AdjustmentGroup::ColorMixer, true);
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

    fn convert_masks(&mut self, document: &Document<'_>) -> Vec<LocalMask> {
        let mut masks = Vec::new();
        for (index, correction) in document.corrections().into_iter().enumerate() {
            if masks.len() >= MAX_LOCAL_MASKS {
                self.warnings.push(format!(
                    "Only the first {MAX_LOCAL_MASKS} masks were imported."
                ));
                break;
            }
            if let Some(mask) = convert_correction(correction, index, &mut self.warnings) {
                masks.push(mask);
            }
        }
        masks
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

        // A profile name can stay in the file while the profile is disabled;
        // only an enabled profile changes the photo.
        let lens_profile = match properties.value("LensProfileEnable") {
            Some(enabled) => enabled != 0.0,
            None => {
                properties
                    .text("LensProfileSetup")
                    .is_some_and(|setup| !setup.eq_ignore_ascii_case("LensProfileDefault"))
                    || properties.text("LensProfileName").is_some()
                    || [
                        "LensProfileDistortionScale",
                        "LensProfileChromaticAberrationScale",
                        "LensProfileVignettingScale",
                    ]
                    .iter()
                    .any(|key| properties.differs(key, 100.0))
            }
        };
        if lens_profile {
            self.warnings.push(LENS_WARNING);
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

fn convert_correction(
    correction: &Element,
    index: usize,
    warnings: &mut Warnings,
) -> Option<LocalMask> {
    let properties = Properties::of(correction);
    let fallback_name = format!("Mask {}", index + 1);
    let (name, shortened) = sanitize_label(
        properties.text("CorrectionName").unwrap_or(&fallback_name),
        &fallback_name,
        MAX_PRESET_NAME_CHARS,
    );
    if shortened {
        warnings.push(format!(
            "A mask name was shortened to {MAX_PRESET_NAME_CHARS} characters."
        ));
    }

    let mut components = Vec::new();
    let mut unsupported = Vec::new();
    let items = correction
        .child("crs:CorrectionMasks")
        .into_iter()
        .flat_map(|masks| masks.seq_items());
    for item in items {
        let attributes = Properties::of(item.resource());
        let Some(kind) = mask_kind(&attributes) else {
            unsupported.push(format!(
                "“{}” ({})",
                component_name(&attributes),
                mask_kind_label(&attributes)
            ));
            continue;
        };
        let Some(combine) = mask_combine(&attributes) else {
            unsupported.push(format!(
                "“{}” (unknown blend mode)",
                component_name(&attributes)
            ));
            continue;
        };
        if components.len() >= MAX_MASK_COMPONENTS {
            warnings.push(format!(
                "The mask “{name}” has more than {MAX_MASK_COMPONENTS} components; the extra components were not imported."
            ));
            break;
        }
        let mut component = MaskComponent::new(kind, combine);
        if let Some(component_name) = attributes.text("MaskName") {
            component.name = sanitize_label(component_name, kind.label(), MAX_PRESET_NAME_CHARS).0;
        }
        component.enabled = attributes.boolean("MaskActive").unwrap_or(true);
        component.invert = attributes.boolean("MaskInverted").unwrap_or(false);
        components.push(component);
    }

    if !unsupported.is_empty() {
        warnings.push(format!(
            "The mask “{name}” was not imported because it uses unsupported components: {}.",
            unsupported.join(", ")
        ));
        return None;
    }
    let Some(first) = components.first() else {
        warnings.push(format!(
            "The mask “{name}” has no components and was not imported."
        ));
        return None;
    };
    if first.combine != MaskCombineMode::Add {
        warnings.push(format!(
            "The mask “{name}” starts with a subtracted component and was not imported."
        ));
        return None;
    }

    let mut mask = LocalMask::new(first.kind, index + 1);
    mask.adjustments = local_adjustments(correction, &properties, &name, warnings);
    mask.common.name = name;
    mask.common.enabled = properties.boolean("CorrectionActive").unwrap_or(true);
    mask.opacity = properties
        .value("CorrectionAmount")
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);
    mask.components = components;
    Some(mask)
}

fn local_adjustments(
    correction: &Element,
    properties: &Properties<'_>,
    mask_name: &str,
    warnings: &mut Warnings,
) -> LocalAdjustments {
    let mut adjustments = LocalAdjustments::default();

    // Local sliders are stored as -1..1, unlike the -100..100 global sliders.
    // Exposure is already in EV, and hue scales to CalibRaw's hue rotation.
    if let Some(value) = properties.first_number(&["LocalExposure2012", "LocalExposure"], warnings)
    {
        adjustments.exposure = value.clamp(-5.0, 5.0);
    }
    if let Some(value) = properties.number("LocalHue", warnings) {
        adjustments.hue = (value * HUE_ROTATION_LIMIT_DEGREES)
            .clamp(-HUE_ROTATION_LIMIT_DEGREES, HUE_ROTATION_LIMIT_DEGREES);
    }
    for (keys, target) in [
        (
            &["LocalContrast2012", "LocalContrast"][..],
            &mut adjustments.contrast,
        ),
        (
            &["LocalHighlights2012", "LocalHighlights"],
            &mut adjustments.highlights,
        ),
        (
            &["LocalShadows2012", "LocalShadows"],
            &mut adjustments.shadows,
        ),
        (&["LocalWhites2012", "LocalWhites"], &mut adjustments.whites),
        (&["LocalBlacks2012", "LocalBlacks"], &mut adjustments.blacks),
        (&["LocalTemperature"], &mut adjustments.temperature),
        (&["LocalTint"], &mut adjustments.tint),
        (&["LocalSaturation"], &mut adjustments.saturation),
        (&["LocalTexture"], &mut adjustments.texture),
        (
            &["LocalClarity2012", "LocalClarity"],
            &mut adjustments.clarity,
        ),
        (&["LocalDehaze"], &mut adjustments.dehaze),
    ] {
        if let Some(value) = properties.first_number(keys, warnings) {
            *target = (value * 100.0).clamp(-100.0, 100.0);
        }
    }

    let targets = [
        &mut adjustments.tone_curve,
        &mut adjustments.tone_curve_red,
        &mut adjustments.tone_curve_green,
        &mut adjustments.tone_curve_blue,
    ];
    for ((key, label), target) in LOCAL_TONE_CURVES.iter().zip(targets) {
        let Some(element) = correction.child(key) else {
            continue;
        };
        let what = format!("The local {label} tone curve of the mask “{mask_name}”");
        if let Some(curve) = read_curve(element, &what, warnings) {
            *target = curve;
        }
    }

    if UNSUPPORTED_LOCAL_SETTINGS
        .iter()
        .any(|(key, neutral)| properties.differs(key, *neutral))
        || correction.child("crs:LocalPointColors").is_some()
    {
        warnings.push(format!(
            "Some local adjustments of the mask “{mask_name}” were not imported."
        ));
    }
    adjustments
}

fn mask_kind(attributes: &Properties<'_>) -> Option<MaskKind> {
    match integer(attributes.value("MaskSubType")?)? {
        0 if attributes.value("MaskSubCategoryID") == Some(22.0) => Some(MaskKind::Background),
        1 => Some(MaskKind::Subject),
        2 => Some(MaskKind::Sky),
        _ => None,
    }
}

fn mask_combine(attributes: &Properties<'_>) -> Option<MaskCombineMode> {
    if !attributes.has("MaskBlendMode") {
        return Some(MaskCombineMode::Add);
    }
    match integer(attributes.value("MaskBlendMode")?)? {
        0 => Some(MaskCombineMode::Add),
        1 => Some(MaskCombineMode::Subtract),
        2 => Some(MaskCombineMode::Intersect),
        _ => None,
    }
}

/// `value` as an enumeration index, when it is a whole number.
fn integer(value: f32) -> Option<i32> {
    ((value - value.round()).abs() <= 1e-6).then_some(value.round() as i32)
}

fn component_name(attributes: &Properties<'_>) -> String {
    let fallback = "unnamed mask";
    sanitize_label(
        attributes.text("MaskName").unwrap_or(fallback),
        fallback,
        MAX_PRESET_NAME_CHARS,
    )
    .0
}

fn mask_kind_label(attributes: &Properties<'_>) -> &'static str {
    match attributes
        .value("MaskSubType")
        .map(|value| value.round() as i32)
    {
        Some(0) => "landscape element",
        Some(1) => "subject",
        Some(2) => "sky",
        Some(3) => "person",
        Some(4) => "object",
        Some(_) => "unknown AI mask",
        None => match attributes.text("What") {
            Some("Mask/CircularGradient") => "radial gradient",
            Some("Mask/LinearGradient") => "linear gradient",
            Some("Mask/Aggregate") => "brush",
            Some("Mask/Path") => "path",
            Some("Mask/Image") => "unsupported AI mask",
            Some(_) | None => "unsupported mask",
        },
    }
}

/// Reads the curve property `element`. An unreadable curve is reported as
/// `what` and left linear.
fn read_curve(element: &Element, what: &str, warnings: &mut Warnings) -> Option<PointCurve> {
    let points: Vec<&str> = element
        .seq_items()
        .map(|item| item.text.trim())
        .filter(|point| !point.is_empty())
        .collect();
    let curve = parse_curve(&points, warnings).map(|points| point_curve_from_points(&points));
    if curve.is_none() {
        warnings.push(format!("{what} could not be read; it was left linear."));
    }
    curve
}

/// Parses "x, y" points in 0–255 into sorted 0–1 points, reduced to the
/// point limit. Needs at least two distinct points.
fn parse_curve(raw_points: &[&str], warnings: &mut Warnings) -> Option<Vec<[f32; 2]>> {
    let mut points = Vec::with_capacity(raw_points.len());
    for raw in raw_points {
        let point = raw
            .split_once(',')
            .and_then(|(x, y)| Some([parse_number(x)?, parse_number(y)?]));
        let Some([x, y]) = point else {
            warnings.push(format!("The tone curve point “{raw}” could not be read."));
            continue;
        };
        if !(0.0..=255.0).contains(&x) || !(0.0..=255.0).contains(&y) {
            warnings.push(format!(
                "The tone curve point “{raw}” is outside the 0–255 range and was skipped."
            ));
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
        warnings.push(format!(
            "The tone curve has {} points; it was reduced to {MAX_POINT_CURVE_POINTS}.",
            points.len()
        ));
        points = resample_curve(&points, MAX_POINT_CURVE_POINTS);
    }
    Some(points)
}

/// `max_points` evenly spaced samples of the piecewise-linear curve through
/// `points`, which are sorted and at least two.
fn resample_curve(points: &[[f32; 2]], max_points: usize) -> Vec<[f32; 2]> {
    let first_x = points[0][0];
    let span = points[points.len() - 1][0] - first_x;
    (0..max_points)
        .map(|index| {
            let fraction = index as f32 / (max_points - 1) as f32;
            let x = first_x + span * fraction;
            [x, interpolate_curve(points, x)]
        })
        .collect()
}

fn interpolate_curve(points: &[[f32; 2]], x: f32) -> f32 {
    for pair in points.windows(2) {
        let [x0, y0] = pair[0];
        let [x1, y1] = pair[1];
        if x <= x1 {
            let width = x1 - x0;
            if width.abs() < f32::EPSILON || x <= x0 {
                return y0;
            }
            return y0 + (y1 - y0) * (x - x0) / width;
        }
    }
    points[points.len() - 1][1]
}

fn point_curve_from_points(points: &[[f32; 2]]) -> PointCurve {
    let mut curve = PointCurve::linear();
    curve.points = [[1.0, 1.0]; MAX_POINT_CURVE_POINTS];
    let len = points.len().min(MAX_POINT_CURVE_POINTS);
    curve.points[..len].copy_from_slice(&points[..len]);
    curve.len = len as u32;
    curve.sanitize();
    curve
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

fn parse_number(raw: &str) -> Option<f32> {
    let raw = raw.trim();
    let raw = raw.strip_prefix('+').unwrap_or(raw);
    raw.parse::<f32>().ok().filter(|value| value.is_finite())
}

/// Maps the 0–100 grain size to CalibRaw's 0.5–4.0. The format's default 25
/// lands on CalibRaw's default 1.0.
fn grain_size(value: f32) -> f32 {
    let value = value.clamp(0.0, 100.0);
    if value <= 25.0 {
        0.5 + value / 25.0 * 0.5
    } else {
        1.0 + (value - 25.0) / 75.0 * 3.0
    }
}

#[cfg(test)]
mod tests;
