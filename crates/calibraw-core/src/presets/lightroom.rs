//! Import of Adobe Lightroom and Camera Raw presets (`.xmp`).
//!
//! A preset stores develop settings as `crs:` (Camera Raw Settings) properties
//! of an XMP packet: simple values as attributes of `rdf:Description`, curves
//! and names as `rdf:Seq` / `rdf:Alt` children. Each setting is converted to
//! the CalibRaw control with the same meaning and range. Same-named sliders
//! run different algorithms in the two programs, so an imported preset is a
//! close starting point, not a pixel match. A profile's colour lookup table
//! becomes a CalibRaw colour look (see [`rgb_table`]); Adobe also applies it
//! earlier in its own pipeline, so its strength may need adjusting.
//!
//! Only the settings present in the file are read, and a CalibRaw preset
//! carries whole sidebar cards: applying an imported preset resets the other
//! sliders of every card it touches. Whatever has no CalibRaw counterpart
//! (white balance, built-in Adobe profiles, crops, masks, ...) is listed in
//! [`LightroomPresetImport::skipped`] instead of being dropped silently.

mod rgb_table;

use super::{Preset, PresetError, MAX_PRESET_NAME_CHARS};
use crate::pipeline::{
    AdjustmentGroup, AdjustmentGroupSet, ColorLut, ColorLutEdit, ExposureParams, PointCurve,
    MAX_POINT_CURVE_POINTS,
};
use crate::sidecar::{default_edit_state, EditSelection};
use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::ops::RangeInclusive;
use std::path::Path;

pub const LIGHTROOM_PRESET_SUFFIX: &str = ".xmp";

/// Presets with an embedded colour lookup table can be several megabytes
/// before they are found to be unsupported.
const MAX_LIGHTROOM_PRESET_BYTES: u64 = 32 * 1024 * 1024;
const MAX_XML_DEPTH: usize = 64;
const DEFAULT_IMPORT_GROUP: &str = "Lightroom";
const FALLBACK_NAME: &str = "Lightroom preset";
/// The hue, saturation and luminance bands of the colour mixer, in the order
/// of `ExposureParams::hsl_*` and of Lightroom's `...Adjustment<Colour>` keys.
const MIXER_COLOURS: [&str; 8] = [
    "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
];

/// A Lightroom preset converted to a CalibRaw [`Preset`].
#[derive(Clone, Debug, PartialEq)]
pub struct LightroomPresetImport {
    pub preset: Preset,
    /// How many Lightroom settings were converted.
    pub imported_settings: usize,
    /// What the file contains that CalibRaw cannot reproduce, in plain words.
    pub skipped: Vec<String>,
}

pub fn is_lightroom_preset_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.len() > LIGHTROOM_PRESET_SUFFIX.len()
                && name
                    .get(name.len() - LIGHTROOM_PRESET_SUFFIX.len()..)
                    .is_some_and(|suffix| suffix.eq_ignore_ascii_case(LIGHTROOM_PRESET_SUFFIX))
                && !name.starts_with('.')
        })
}

/// Reads and converts the Lightroom preset at `path`. A preset without a name
/// of its own is named after the file.
pub fn read_lightroom_preset_file(path: &Path) -> Result<LightroomPresetImport, PresetError> {
    let file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    if length > MAX_LIGHTROOM_PRESET_BYTES {
        return Err(PresetError::TooLarge(length));
    }
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(MAX_LIGHTROOM_PRESET_BYTES + 1)
        .read_to_end(&mut bytes)?;
    let fallback = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    LightroomPresetImport::from_xmp(&bytes, &fallback)
}

impl LightroomPresetImport {
    /// Converts the XMP packet in `bytes`. `fallback_name` names a preset
    /// whose file does not.
    pub fn from_xmp(bytes: &[u8], fallback_name: &str) -> Result<Self, PresetError> {
        if bytes.len() as u64 > MAX_LIGHTROOM_PRESET_BYTES {
            return Err(PresetError::TooLarge(bytes.len() as u64));
        }
        let text = std::str::from_utf8(bytes)
            .map_err(|_| PresetError::Invalid("the preset is not UTF-8 XML".to_owned()))?
            .trim_start_matches('\u{feff}');
        let settings = CameraRawSettings::parse(text)?;
        if settings.values.is_empty() && settings.present.is_empty() {
            return Err(PresetError::Invalid(
                "not a Lightroom preset: it has no Camera Raw settings".to_owned(),
            ));
        }

        let mut converter = Converter::new(&settings);
        converter.convert();
        let Converter {
            exposure,
            groups,
            imported,
            skipped,
            color_lut,
            ..
        } = converter;
        if imported == 0 {
            return Err(PresetError::Unsupported(if skipped.is_empty() {
                "it has no Lightroom develop settings that CalibRaw can import".to_owned()
            } else {
                format!(
                    "it only has settings that CalibRaw cannot import ({})",
                    skipped.join(", ")
                )
            }));
        }

        let name = label(
            settings.first_item("Name"),
            &label(Some(fallback_name), FALLBACK_NAME),
        );
        let group = label(settings.first_item("Group"), DEFAULT_IMPORT_GROUP);
        let mut edits = default_edit_state();
        edits.exposure = exposure;
        edits.exposure.sanitize_tone_curves();
        let has_lut = color_lut.is_some();
        edits.color_lut = color_lut.map(|lut| ColorLutEdit::new(&name, lut));
        let preset = Preset::new(
            &name,
            &group,
            EditSelection {
                adjustment_groups: groups,
                color_lut: has_lut,
                ..EditSelection::default()
            },
            &edits,
        )?;
        Ok(Self {
            preset,
            imported_settings: imported,
            skipped,
        })
    }
}

/// A name or group from the file, cleaned up to what a preset accepts.
fn label(text: Option<&str>, fallback: &str) -> String {
    let cleaned: String = text
        .unwrap_or_default()
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    let cleaned: String = cleaned.trim().chars().take(MAX_PRESET_NAME_CHARS).collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        fallback.to_owned()
    } else {
        cleaned.to_owned()
    }
}

/// The `crs:` properties of a preset, keyed without their prefix.
#[derive(Default)]
struct CameraRawSettings {
    /// Properties written as attributes or as simple elements.
    values: HashMap<String, String>,
    /// The text of each `rdf:li` of a sequence or alternative.
    lists: HashMap<String, Vec<String>>,
    /// Every property present, however it is structured.
    present: HashSet<String>,
}

impl CameraRawSettings {
    fn parse(text: &str) -> Result<Self, PresetError> {
        let invalid = |error: &dyn std::fmt::Display| {
            PresetError::Invalid(format!("invalid preset XML: {error}"))
        };
        let mut reader = Reader::from_str(text);
        let mut settings = Self::default();
        // Open element names, to tell a top-level property from the nested
        // structures of masks and similar.
        let mut stack: Vec<String> = Vec::new();
        let mut property: Option<Property> = None;
        loop {
            let event = reader.read_event().map_err(|error| invalid(&error))?;
            match event {
                Event::Empty(start) => {
                    settings.open(&start, &stack, &mut property, false, &invalid)?;
                }
                Event::Start(start) => {
                    settings.open(&start, &stack, &mut property, true, &invalid)?;
                    stack.push(qualified_name(&start));
                    if stack.len() > MAX_XML_DEPTH {
                        return Err(invalid(&"elements are nested too deeply"));
                    }
                    if let Some(property) = property.as_mut() {
                        property.track_open(&stack);
                    }
                }
                Event::End(_) => {
                    let closed = stack.pop().unwrap_or_default();
                    let property_closed = property
                        .as_mut()
                        .is_some_and(|open| open.track_close(&closed, stack.len()));
                    if property_closed {
                        if let Some(finished) = property.take() {
                            settings.finish(finished);
                        }
                    }
                }
                Event::Text(content) => {
                    if let Some(open) = property.as_mut() {
                        let decoded = content
                            .xml_content(XmlVersion::Implicit1_0)
                            .map_err(|error| invalid(&error))?;
                        open.push_text(&decoded);
                    }
                }
                Event::GeneralRef(reference) => {
                    if let Some(open) = property.as_mut() {
                        if let Some(character) = resolve_reference(&reference) {
                            open.push_text(character.encode_utf8(&mut [0; 4]));
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        Ok(settings)
    }

    /// Handles the start of an element: the properties on `rdf:Description`,
    /// and the start of a `crs:` property element below it.
    fn open(
        &mut self,
        start: &BytesStart<'_>,
        stack: &[String],
        property: &mut Option<Property>,
        has_children: bool,
        invalid: &dyn Fn(&dyn std::fmt::Display) -> PresetError,
    ) -> Result<(), PresetError> {
        let name = qualified_name(start);
        // Masks and other structured properties hold nested descriptions with
        // their own `crs:` attributes; only the top-level ones are settings.
        if let Some(open) = property.as_ref() {
            if name == "rdf:Description" {
                self.read_nested_look(start, &open.key, invalid)?;
            }
            return Ok(());
        }
        if name == "rdf:Description" {
            for attribute in start.attributes() {
                let attribute = attribute.map_err(|error| invalid(&error))?;
                let key = String::from_utf8_lossy(attribute.key.as_ref()).into_owned();
                let Some(key) = key.strip_prefix("crs:") else {
                    continue;
                };
                let value = attribute
                    .normalized_value(XmlVersion::Implicit1_0)
                    .map_err(|error| invalid(&error))?;
                self.present.insert(key.to_owned());
                self.values.insert(key.to_owned(), value.into_owned());
            }
        } else if stack
            .last()
            .is_some_and(|parent| parent == "rdf:Description")
        {
            if let Some(key) = name.strip_prefix("crs:") {
                self.present.insert(key.to_owned());
                if has_children {
                    *property = Some(Property::new(key, stack.len() + 1));
                }
            }
        }
        Ok(())
    }

    /// A `crs:Look` names an Adobe profile, and a profile that carries its own
    /// lookup table keeps it in a nested description. Everything else nested
    /// below a property belongs to that property, not to the preset.
    fn read_nested_look(
        &mut self,
        start: &BytesStart<'_>,
        property: &str,
        invalid: &dyn Fn(&dyn std::fmt::Display) -> PresetError,
    ) -> Result<(), PresetError> {
        if property != "Look" {
            return Ok(());
        }
        for attribute in start.attributes() {
            let attribute = attribute.map_err(|error| invalid(&error))?;
            let key = String::from_utf8_lossy(attribute.key.as_ref()).into_owned();
            let Some(key) = key.strip_prefix("crs:") else {
                continue;
            };
            let target = match key {
                "Name" => "LookName",
                "RGBTable" => "RGBTable",
                key if key.starts_with("Table_") => key,
                _ => continue,
            };
            let value = attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .map_err(|error| invalid(&error))?;
            // A top-level table wins over one nested in a look.
            self.values
                .entry(target.to_owned())
                .or_insert_with(|| value.into_owned());
        }
        Ok(())
    }

    fn finish(&mut self, property: Property) {
        let Property {
            key, items, text, ..
        } = property;
        if !items.is_empty() {
            self.lists.insert(key, items);
        } else if !text.trim().is_empty() {
            self.values.insert(key, text.trim().to_owned());
        }
    }

    fn first_item(&self, key: &str) -> Option<&str> {
        self.lists.get(key)?.first().map(String::as_str)
    }

    fn number(&self, key: &str) -> Option<f32> {
        parse_number(self.values.get(key)?)
    }

    fn is_nonzero(&self, key: &str) -> bool {
        self.number(key).is_some_and(|value| value != 0.0)
    }

    fn is_true(&self, key: &str) -> bool {
        self.values
            .get(key)
            .is_some_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "true" | "1"))
    }

    fn has_any(&self, keys: &[&str]) -> bool {
        keys.iter().any(|key| self.present.contains(*key))
    }

    fn any_nonzero(&self, keys: &[&str]) -> bool {
        keys.iter().any(|key| self.is_nonzero(key))
    }
}

/// A `crs:` property element being read.
struct Property {
    key: String,
    /// Stack depth right after the property element was opened.
    depth: usize,
    items: Vec<String>,
    /// Text directly inside the property, for simple elements.
    text: String,
    /// Text of the `rdf:li` being read, when it belongs to this property.
    item: Option<String>,
}

impl Property {
    fn new(key: &str, depth: usize) -> Self {
        Self {
            key: key.to_owned(),
            depth,
            items: Vec::new(),
            text: String::new(),
            item: None,
        }
    }

    /// Called after an element is pushed. A list item is an `rdf:li` directly
    /// inside the `rdf:Seq` or `rdf:Alt` of this property; deeper ones belong
    /// to nested structures such as masks.
    fn track_open(&mut self, stack: &[String]) {
        if stack.len() == self.depth + 2 && stack.last().is_some_and(|name| name == "rdf:li") {
            self.item = Some(String::new());
        }
    }

    /// Called after an element is popped. Returns whether the property's own
    /// element has closed.
    fn track_close(&mut self, closed: &str, remaining: usize) -> bool {
        if closed == "rdf:li" && remaining == self.depth + 1 {
            if let Some(item) = self.item.take() {
                let item = item.trim();
                if !item.is_empty() {
                    self.items.push(item.to_owned());
                }
            }
        }
        remaining + 1 == self.depth
    }

    fn push_text(&mut self, text: &str) {
        match self.item.as_mut() {
            Some(item) => item.push_str(text),
            None => self.text.push_str(text),
        }
    }
}

fn qualified_name(start: &BytesStart<'_>) -> String {
    String::from_utf8_lossy(start.name().as_ref()).into_owned()
}

fn resolve_reference(reference: &BytesRef<'_>) -> Option<char> {
    if let Ok(Some(character)) = reference.resolve_char_ref() {
        return Some(character);
    }
    match reference.decode().ok()?.as_ref() {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => None,
    }
}

/// Lightroom writes signed numbers such as `+0.50`.
fn parse_number(text: &str) -> Option<f32> {
    text.trim()
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
}

struct Converter<'a> {
    settings: &'a CameraRawSettings,
    exposure: ExposureParams,
    groups: AdjustmentGroupSet,
    imported: usize,
    skipped: Vec<String>,
    color_lut: Option<ColorLut>,
}

type Field = fn(&mut ExposureParams) -> &mut f32;

impl<'a> Converter<'a> {
    fn new(settings: &'a CameraRawSettings) -> Self {
        Self {
            settings,
            exposure: ExposureParams::default(),
            groups: AdjustmentGroupSet::EMPTY,
            imported: 0,
            skipped: Vec::new(),
            color_lut: None,
        }
    }

    fn convert(&mut self) {
        self.light();
        self.tone_curves();
        self.color();
        self.color_grading();
        self.detail();
        self.effects();
        self.color_mixer();
        self.color_lut();
        self.note_unsupported();
    }

    /// Reads `key` into `field`, limited to `range`. Any present setting
    /// includes its card in the preset, even when it equals the default.
    fn set(&mut self, key: &str, group: AdjustmentGroup, range: RangeInclusive<f32>, field: Field) {
        let Some(value) = self.settings.number(key) else {
            return;
        };
        *field(&mut self.exposure) = value.clamp(*range.start(), *range.end());
        self.groups.set(group, true);
        self.imported += 1;
    }

    fn light(&mut self) {
        use AdjustmentGroup::Light;
        self.set("Exposure2012", Light, -5.0..=5.0, |e| &mut e.exposure);
        self.set("Contrast2012", Light, -100.0..=100.0, |e| &mut e.contrast);
        self.set("Highlights2012", Light, -100.0..=100.0, |e| {
            &mut e.highlights
        });
        self.set("Shadows2012", Light, -100.0..=100.0, |e| &mut e.shadows);
        self.set("Whites2012", Light, -100.0..=100.0, |e| &mut e.whites);
        self.set("Blacks2012", Light, -100.0..=100.0, |e| &mut e.blacks);
    }

    fn tone_curves(&mut self) {
        self.curve("ToneCurvePV2012", |e| &mut e.tone_curve);
        self.curve("ToneCurvePV2012Red", |e| &mut e.tone_curve_red);
        self.curve("ToneCurvePV2012Green", |e| &mut e.tone_curve_green);
        self.curve("ToneCurvePV2012Blue", |e| &mut e.tone_curve_blue);
    }

    /// Lightroom lists curve points as `"x, y"` on a 0–255 scale.
    fn curve(&mut self, key: &str, field: fn(&mut ExposureParams) -> &mut PointCurve) {
        let Some(items) = self.settings.lists.get(key) else {
            return;
        };
        let mut points: Vec<[f32; 2]> = items
            .iter()
            .filter_map(|item| {
                let (x, y) = item.split_once(',')?;
                Some([
                    (parse_number(x)? / 255.0).clamp(0.0, 1.0),
                    (parse_number(y)? / 255.0).clamp(0.0, 1.0),
                ])
            })
            .collect();
        if points.len() < 2 {
            return;
        }
        points.sort_by(|left, right| left[0].total_cmp(&right[0]));
        if points[0][0] > 0.0 {
            points.insert(0, [0.0, points[0][1]]);
        }
        if points[points.len() - 1][0] < 1.0 {
            points.push([1.0, points[points.len() - 1][1]]);
        }
        if points.len() > MAX_POINT_CURVE_POINTS {
            let last = points.len() - 1;
            points = (0..MAX_POINT_CURVE_POINTS)
                .map(|index| points[index * last / (MAX_POINT_CURVE_POINTS - 1)])
                .collect();
            self.skipped.push(format!(
                "tone curve points beyond {MAX_POINT_CURVE_POINTS} (the curve was simplified)"
            ));
        }
        let mut curve = PointCurve::linear();
        curve.points = [[1.0, 1.0]; MAX_POINT_CURVE_POINTS];
        curve.points[..points.len()].copy_from_slice(&points);
        curve.len = points.len() as u32;
        *field(&mut self.exposure) = curve;
        self.groups.set(AdjustmentGroup::ToneCurve, true);
        self.imported += 1;
    }

    fn color(&mut self) {
        use AdjustmentGroup::Color;
        self.set("Vibrance", Color, -100.0..=100.0, |e| &mut e.vibrance);
        self.set("Saturation", Color, -100.0..=100.0, |e| &mut e.saturation);
    }

    fn color_grading(&mut self) {
        use AdjustmentGroup::ColorGrading as Grading;
        // Shadow and highlight wheels keep their historical split-toning keys.
        self.set("SplitToningShadowHue", Grading, 0.0..=360.0, |e| {
            &mut e.color_grading.shadows.hue
        });
        self.set("SplitToningShadowSaturation", Grading, 0.0..=100.0, |e| {
            &mut e.color_grading.shadows.saturation
        });
        self.set("SplitToningHighlightHue", Grading, 0.0..=360.0, |e| {
            &mut e.color_grading.highlights.hue
        });
        self.set(
            "SplitToningHighlightSaturation",
            Grading,
            0.0..=100.0,
            |e| &mut e.color_grading.highlights.saturation,
        );
        self.set("SplitToningBalance", Grading, -100.0..=100.0, |e| {
            &mut e.color_grading.balance
        });
        self.set("ColorGradeMidtoneHue", Grading, 0.0..=360.0, |e| {
            &mut e.color_grading.midtones.hue
        });
        self.set("ColorGradeMidtoneSat", Grading, 0.0..=100.0, |e| {
            &mut e.color_grading.midtones.saturation
        });
        self.set("ColorGradeGlobalHue", Grading, 0.0..=360.0, |e| {
            &mut e.color_grading.global.hue
        });
        self.set("ColorGradeGlobalSat", Grading, 0.0..=100.0, |e| {
            &mut e.color_grading.global.saturation
        });
        self.set("ColorGradeShadowLum", Grading, -100.0..=100.0, |e| {
            &mut e.color_grading.shadows.luminance
        });
        self.set("ColorGradeMidtoneLum", Grading, -100.0..=100.0, |e| {
            &mut e.color_grading.midtones.luminance
        });
        self.set("ColorGradeHighlightLum", Grading, -100.0..=100.0, |e| {
            &mut e.color_grading.highlights.luminance
        });
        self.set("ColorGradeGlobalLum", Grading, -100.0..=100.0, |e| {
            &mut e.color_grading.global.luminance
        });
        self.set("ColorGradeBlending", Grading, 0.0..=100.0, |e| {
            &mut e.color_grading.blending
        });
    }

    fn detail(&mut self) {
        use AdjustmentGroup::Detail;
        self.set("Sharpness", Detail, 0.0..=150.0, |e| &mut e.sharpen_amount);
        self.set("SharpenRadius", Detail, 0.5..=3.0, |e| {
            &mut e.sharpen_radius
        });
        self.set("SharpenDetail", Detail, 0.0..=100.0, |e| {
            &mut e.sharpen_detail
        });
        self.set("SharpenEdgeMasking", Detail, 0.0..=100.0, |e| {
            &mut e.sharpen_masking
        });
        self.set("LuminanceSmoothing", Detail, 0.0..=100.0, |e| {
            &mut e.luminance_denoise
        });
        self.set("LuminanceNoiseReductionDetail", Detail, 0.0..=100.0, |e| {
            &mut e.denoise_detail
        });
        // CalibRaw stores colour noise reduction as a fraction.
        self.set("ColorNoiseReduction", Detail, 0.0..=100.0, |e| {
            &mut e.chroma_denoise
        });
        if self.settings.number("ColorNoiseReduction").is_some() {
            self.exposure.chroma_denoise /= 100.0;
        }
    }

    fn effects(&mut self) {
        use AdjustmentGroup::Effects;
        self.set("Texture", Effects, -100.0..=100.0, |e| &mut e.texture);
        self.set("Clarity2012", Effects, -100.0..=100.0, |e| &mut e.clarity);
        self.set("Dehaze", Effects, -100.0..=100.0, |e| &mut e.dehaze);
        self.set("GrainAmount", Effects, 0.0..=100.0, |e| &mut e.grain_amount);
        self.set("PostCropVignetteAmount", Effects, -100.0..=100.0, |e| {
            &mut e.vignette_amount
        });
        self.set("PostCropVignetteMidpoint", Effects, 0.0..=100.0, |e| {
            &mut e.vignette_midpoint
        });
        self.set("PostCropVignetteRoundness", Effects, -100.0..=100.0, |e| {
            &mut e.vignette_roundness
        });
        self.set("PostCropVignetteFeather", Effects, 0.0..=100.0, |e| {
            &mut e.vignette_feather
        });
        self.set(
            "PostCropVignetteHighlightContrast",
            Effects,
            0.0..=100.0,
            |e| &mut e.vignette_highlights,
        );
    }

    fn color_mixer(&mut self) {
        use AdjustmentGroup::ColorMixer;
        const HUE: [Field; 8] = [
            |e| &mut e.hsl_hue[0],
            |e| &mut e.hsl_hue[1],
            |e| &mut e.hsl_hue[2],
            |e| &mut e.hsl_hue[3],
            |e| &mut e.hsl_hue[4],
            |e| &mut e.hsl_hue[5],
            |e| &mut e.hsl_hue[6],
            |e| &mut e.hsl_hue[7],
        ];
        const SATURATION: [Field; 8] = [
            |e| &mut e.hsl_saturation[0],
            |e| &mut e.hsl_saturation[1],
            |e| &mut e.hsl_saturation[2],
            |e| &mut e.hsl_saturation[3],
            |e| &mut e.hsl_saturation[4],
            |e| &mut e.hsl_saturation[5],
            |e| &mut e.hsl_saturation[6],
            |e| &mut e.hsl_saturation[7],
        ];
        const LUMINANCE: [Field; 8] = [
            |e| &mut e.hsl_luminance[0],
            |e| &mut e.hsl_luminance[1],
            |e| &mut e.hsl_luminance[2],
            |e| &mut e.hsl_luminance[3],
            |e| &mut e.hsl_luminance[4],
            |e| &mut e.hsl_luminance[5],
            |e| &mut e.hsl_luminance[6],
            |e| &mut e.hsl_luminance[7],
        ];
        for (index, colour) in MIXER_COLOURS.into_iter().enumerate() {
            self.set(
                &format!("HueAdjustment{colour}"),
                ColorMixer,
                -100.0..=100.0,
                HUE[index],
            );
            self.set(
                &format!("SaturationAdjustment{colour}"),
                ColorMixer,
                -100.0..=100.0,
                SATURATION[index],
            );
            self.set(
                &format!("LuminanceAdjustment{colour}"),
                ColorMixer,
                -100.0..=100.0,
                LUMINANCE[index],
            );
        }
    }

    /// Converts the profile's lookup table, when the file carries one.
    fn color_lut(&mut self) {
        let Some(fingerprint) = self.settings.values.get("RGBTable") else {
            return;
        };
        let Some(text) = self
            .settings
            .values
            .get(&format!("Table_{}", fingerprint.trim()))
        else {
            self.skipped
                .push("colour lookup table (its data is missing)".to_owned());
            return;
        };
        match rgb_table::decode_srgb_table(text) {
            Ok(lut) => {
                self.color_lut = Some(lut);
                self.imported += 1;
            }
            Err(error) => self.skipped.push(format!("colour lookup table ({error})")),
        }
    }

    /// Lists what the file asks for that no CalibRaw preset can carry.
    fn note_unsupported(&mut self) {
        let settings = self.settings;
        let mut note = |condition: bool, what: &str| {
            if condition {
                self.skipped.push(what.to_owned());
            }
        };
        note(
            settings.has_any(&["Temperature", "Tint"])
                || settings
                    .values
                    .get("WhiteBalance")
                    .is_some_and(|value| !value.trim().eq_ignore_ascii_case("As Shot")),
            "white balance",
        );
        note(
            settings.values.get("CameraProfile").is_some_and(|profile| {
                let profile = profile.trim();
                !profile.is_empty() && !profile.eq_ignore_ascii_case("Adobe Standard")
            }),
            "camera profile",
        );
        // Adobe Color is the profile every photo starts with.
        let adobe_profile = settings.values.get("LookName").map(|name| name.trim());
        note(
            settings.present.contains("Look")
                && self.color_lut.is_none()
                && !adobe_profile
                    .is_some_and(|name| ["Adobe Color", "Adobe Standard"].contains(&name)),
            &match adobe_profile.filter(|name| !name.is_empty()) {
                Some(name) => format!("Adobe profile “{name}”"),
                None => "Adobe profile".to_owned(),
            },
        );
        note(
            settings.present.contains("LookTable"),
            "profile hue and saturation table",
        );
        note(
            settings.is_true("ConvertToGrayscale"),
            "black and white conversion",
        );
        note(
            settings.any_nonzero(&[
                "ParametricShadows",
                "ParametricDarks",
                "ParametricLights",
                "ParametricHighlights",
            ]),
            "parametric tone curve",
        );
        note(
            settings.any_nonzero(&[
                "RedHue",
                "RedSaturation",
                "GreenHue",
                "GreenSaturation",
                "BlueHue",
                "BlueSaturation",
                "ShadowTint",
            ]),
            "camera calibration",
        );
        note(
            settings.is_true("LensProfileEnable")
                || settings.is_true("AutoLateralCA")
                || settings.any_nonzero(&["LensManualDistortionAmount", "VignetteAmount"]),
            "lens corrections",
        );
        note(
            settings.any_nonzero(&["DefringePurpleAmount", "DefringeGreenAmount"]),
            "defringe",
        );
        note(
            settings.is_true("HasCrop")
                || settings.any_nonzero(&[
                    "CropAngle",
                    "PerspectiveVertical",
                    "PerspectiveHorizontal",
                    "PerspectiveRotate",
                    "PerspectiveScale",
                ]),
            "crop and geometry",
        );
        note(
            settings.has_any(&[
                "MaskGroupBasedCorrections",
                "GradientBasedCorrections",
                "PaintBasedCorrections",
                "CircularGradientBasedCorrections",
            ]),
            "local adjustments and masks",
        );
        note(settings.has_any(&["RetouchAreas"]), "spot removal");
    }
}

#[cfg(test)]
mod tests;
