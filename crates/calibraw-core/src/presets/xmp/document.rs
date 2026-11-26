//! The preset layer of XMP presets: the preset's own descriptions within a
//! parsed document ([`crate::xmp`]), their `crs:` properties, and the
//! byte-exact label rewriting that keeps every other byte of a copied preset.

use super::super::{PresetError, MAX_PRESET_BYTES};
use crate::xmp::parse_document as parse_xml;
pub(super) use crate::xmp::Element;
use crate::xmp::{apply_edits, attribute_insertion_point, attribute_value_span, Packet, TextError};
use quick_xml::escape::escape;
use std::collections::BTreeMap;
use std::ops::Range;

const CRS_NAMESPACE: &str = "http://ns.adobe.com/camera-raw-settings/1.0/";
/// The lists that hold corrections: the current masking layout, then the
/// older lists that kept each local tool apart.
const CORRECTION_LISTS: [&str; 4] = [
    "crs:MaskGroupBasedCorrections",
    "crs:GradientBasedCorrections",
    "crs:CircularGradientBasedCorrections",
    "crs:PaintBasedCorrections",
];

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
        let at = attribute_insertion_point(text, document.descriptions[0]);
        edits.push((at..at, missing));
    }

    let output = apply_edits(bom, text, edits);
    if output.len() as u64 > MAX_PRESET_BYTES {
        return Err(PresetError::TooLarge(output.len() as u64));
    }
    Ok(output)
}

/// Splits `bytes` into its UTF-8 byte-order mark, if any, and the text.
pub(super) fn split_bom(bytes: &[u8]) -> Result<(&[u8], &str), PresetError> {
    if bytes.len() as u64 > MAX_PRESET_BYTES {
        return Err(PresetError::TooLarge(bytes.len() as u64));
    }
    crate::xmp::split_bom(bytes).map_err(|error| {
        PresetError::Unsupported(
            match error {
                TextError::Utf16 => {
                    "UTF-16 XMP presets are not supported; save the preset as UTF-8"
                }
                TextError::NotUtf8 => "the XMP preset is not valid UTF-8",
            }
            .to_owned(),
        )
    })
}

/// Parses `text` into a tree under an unnamed root element.
pub(super) fn parse_document(text: &str) -> Result<Element, PresetError> {
    parse_xml(text).map_err(|error| PresetError::Invalid(error.to_string()))
}

/// The top-level descriptions of an XMP document, which hold the preset's
/// own properties.
pub(super) struct Document<'a> {
    descriptions: Vec<&'a Element>,
    /// Whether `xmlns:crs` is in scope on the first description.
    crs_declared: bool,
}

impl<'a> Document<'a> {
    pub(super) fn new(root: &'a Element) -> Result<Self, PresetError> {
        let packet = Packet::new(root).map_err(|error| PresetError::Invalid(error.to_string()))?;
        let first = packet.first_description();
        Ok(Self {
            descriptions: packet.descriptions().to_vec(),
            crs_declared: packet.binds_prefix(first, "crs"),
        })
    }

    /// The elements that hold settings: each description and its
    /// `crs:Parameters`.
    pub(super) fn settings(&self) -> Vec<&'a Element> {
        self.descriptions
            .iter()
            .flat_map(|&description| {
                std::iter::once(description)
                    .chain(description.child("crs:Parameters").map(Element::resource))
            })
            .collect()
    }

    pub(super) fn properties(&self) -> Properties<'a> {
        let mut properties = Properties::default();
        for element in self.settings() {
            properties.extend(element);
        }
        properties
    }

    /// The first settings property written as an element, such as a curve.
    pub(super) fn property_element(&self, key: &str) -> Option<&'a Element> {
        self.settings()
            .into_iter()
            .find_map(|element| element.child(key))
    }

    /// The local adjustment groups, called corrections in XMP, from the
    /// current masking list and the older per-tool lists.
    pub(super) fn corrections(&self) -> Vec<&'a Element> {
        let settings = self.settings();
        CORRECTION_LISTS
            .iter()
            .flat_map(|list| {
                settings
                    .iter()
                    .filter_map(move |element| element.child(list))
            })
            .flat_map(|corrections| corrections.seq_items())
            .map(Element::resource)
            .collect()
    }

    /// The preset label `key` (`crs:Name` or `crs:Group`). An element wins
    /// over an attribute, as XMP writers put the localized label there.
    pub(super) fn label(&self, key: &'static str) -> Option<&'a str> {
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
                let tag = description.start_tag();
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
                match element.content() {
                    Some(content) => edits.push((content, alt)),
                    None => {
                        // `<crs:Name/>` becomes `<crs:Name>…</crs:Name>`.
                        let tag = &text[element.start_tag()];
                        let open = tag.strip_suffix("/>").unwrap_or(tag);
                        edits.push((element.start_tag(), format!("{open}>{alt}</{key}>")));
                    }
                }
            }
        }
        found
    }
}

/// The `crs:` attributes of the elements read so far, by local name. Later
/// elements override earlier ones.
#[derive(Default)]
pub(super) struct Properties<'a>(BTreeMap<&'a str, &'a str>);

impl<'a> Properties<'a> {
    pub(super) fn of(element: &'a Element) -> Self {
        let mut properties = Self::default();
        properties.extend(element);
        properties
    }

    fn extend(&mut self, element: &'a Element) {
        for (name, value) in element.attributes() {
            if let Some(key) = name.strip_prefix("crs:") {
                self.0.insert(key, value);
            }
        }
    }

    pub(super) fn has(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    pub(super) fn has_any(&self, keys: &[&str]) -> bool {
        keys.iter().any(|key| self.has(key))
    }

    pub(super) fn text(&self, key: &str) -> Option<&'a str> {
        self.0
            .get(key)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
    }

    /// The number stored as `key`, or `None` when it is missing or unreadable.
    pub(super) fn value(&self, key: &str) -> Option<f32> {
        self.0.get(key).and_then(|raw| parse_number(raw))
    }

    /// Like [`Self::value`], but reports an unreadable value.
    pub(super) fn number(&self, key: &str, warnings: &mut Warnings) -> Option<f32> {
        let raw = self.0.get(key)?;
        let value = parse_number(raw);
        if value.is_none() {
            warnings.push(format!("The value “{raw}” of {key} could not be read."));
        }
        value
    }

    /// The first readable number among `keys`, such as a current key and
    /// its legacy name.
    pub(super) fn first_number(&self, keys: &[&str], warnings: &mut Warnings) -> Option<f32> {
        keys.iter().find_map(|key| self.number(key, warnings))
    }

    /// Whether `key` holds a number other than `neutral`.
    pub(super) fn differs(&self, key: &str, neutral: f32) -> bool {
        self.value(key)
            .is_some_and(|value| (value - neutral).abs() > 1e-6)
    }

    pub(super) fn boolean(&self, key: &str) -> Option<bool> {
        match self.0.get(key)?.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        }
    }
}

/// Warnings in the order they arose, without duplicates.
#[derive(Default)]
pub(super) struct Warnings(Vec<String>);

impl Warnings {
    pub(super) fn push(&mut self, message: impl Into<String>) {
        let message = message.into();
        if !self.0.contains(&message) {
            self.0.push(message);
        }
    }

    pub(super) fn into_vec(self) -> Vec<String> {
        self.0
    }
}

/// A number as XMP writes it, with an optional leading `+`.
pub(super) fn parse_number(raw: &str) -> Option<f32> {
    let raw = raw.trim();
    let raw = raw.strip_prefix('+').unwrap_or(raw);
    raw.parse::<f32>().ok().filter(|value| value.is_finite())
}
