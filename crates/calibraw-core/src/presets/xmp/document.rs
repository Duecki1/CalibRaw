//! The XML layer of XMP presets: a small element tree, the preset's own
//! descriptions within it, their `crs:` properties, and the byte-exact label
//! rewriting that keeps every other byte of a copied preset.

use super::super::{PresetError, MAX_PRESET_BYTES};
use quick_xml::escape::{escape, resolve_predefined_entity};
use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use std::collections::BTreeMap;
use std::ops::Range;

const CRS_NAMESPACE: &str = "http://ns.adobe.com/camera-raw-settings/1.0/";
const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
/// Real presets nest about a dozen elements deep; the limit keeps a hostile
/// file from building an arbitrarily deep tree.
const MAX_ELEMENT_DEPTH: usize = 64;
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
pub(super) fn split_bom(bytes: &[u8]) -> Result<(&[u8], &str), PresetError> {
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

/// One element of an XMP document.
#[derive(Default)]
pub(super) struct Element {
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

    pub(super) fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub(super) fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|child| child.name == name)
    }

    fn children_named(&self, name: &'static str) -> impl Iterator<Item = &Element> + '_ {
        self.children.iter().filter(move |child| child.name == name)
    }

    /// The fields of a struct value: the `rdf:Description` inside a property
    /// or list item, or the element itself when the fields are written on it.
    pub(super) fn resource(&self) -> &Element {
        self.child("rdf:Description").unwrap_or(self)
    }

    /// The items of an `rdf:Seq` property.
    pub(super) fn seq_items(&self) -> impl Iterator<Item = &Element> + '_ {
        self.child("rdf:Seq")
            .into_iter()
            .flat_map(|seq| seq.children_named("rdf:li"))
    }

    /// The non-empty text items of an `rdf:Seq` property, trimmed.
    pub(super) fn seq_texts(&self) -> Vec<&str> {
        self.seq_items()
            .map(|item| item.text.trim())
            .filter(|text| !text.is_empty())
            .collect()
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
pub(super) fn parse_document(text: &str) -> Result<Element, PresetError> {
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
pub(super) struct Document<'a> {
    descriptions: Vec<&'a Element>,
    /// Whether `xmlns:crs` is in scope on the first description.
    crs_declared: bool,
}

impl<'a> Document<'a> {
    pub(super) fn new(root: &'a Element) -> Result<Self, PresetError> {
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
pub(super) struct Properties<'a>(BTreeMap<&'a str, &'a str>);

impl<'a> Properties<'a> {
    pub(super) fn of(element: &'a Element) -> Self {
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
