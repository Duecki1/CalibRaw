//! The XML layer of XMP files: a small element tree that keeps the byte range
//! of every element, the top-level descriptions of an XMP packet, and
//! byte-exact edits that leave every other byte of a file as it was.
//!
//! Shared by the XMP preset import (`presets::xmp`) and the ratings kept in
//! XMP sidecars and exports (`sidecar::xmp_review`).

use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use std::fmt;
use std::ops::Range;

const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
/// Real presets nest about a dozen elements deep; the limit keeps a hostile
/// file from building an arbitrarily deep tree.
const MAX_ELEMENT_DEPTH: usize = 64;

/// Why bytes are not XMP text CalibRaw reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextError {
    Utf16,
    NotUtf8,
}

/// A document the XML reader rejected, or one without the structure of XMP.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct XmlError(String);

impl fmt::Display for XmlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid XMP: {}", self.0)
    }
}

fn invalid_xml(message: impl Into<String>) -> XmlError {
    XmlError(message.into())
}

/// Splits `bytes` into its UTF-8 byte-order mark, if any, and the text.
pub(crate) fn split_bom(bytes: &[u8]) -> Result<(&[u8], &str), TextError> {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return Err(TextError::Utf16);
    }
    let bom_len = if bytes.starts_with(UTF8_BOM) {
        UTF8_BOM.len()
    } else {
        0
    };
    let (bom, rest) = bytes.split_at(bom_len);
    let text = std::str::from_utf8(rest).map_err(|_| TextError::NotUtf8)?;
    Ok((bom, text))
}

/// One element of an XMP document.
#[derive(Default)]
pub(crate) struct Element {
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
    /// Byte range of the end tag; `None` for `<…/>`.
    end_tag: Option<Range<usize>>,
}

impl Element {
    fn open(start: &BytesStart<'_>, start_tag: Range<usize>) -> Result<Self, XmlError> {
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
            .collect::<Result<_, XmlError>>()?;
        Ok(Self {
            name: utf8(start.name().as_ref())?.to_owned(),
            attributes,
            start_tag,
            ..Self::default()
        })
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// Attributes as `(qualified name, value)`, in document order.
    pub(crate) fn attributes(&self) -> impl Iterator<Item = (&str, &str)> + '_ {
        self.attributes
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    pub(crate) fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value)
    }

    pub(crate) fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|child| child.name == name)
    }

    pub(crate) fn children(&self) -> impl Iterator<Item = &Element> + '_ {
        self.children.iter()
    }

    pub(crate) fn children_named<'a>(
        &'a self,
        name: &'a str,
    ) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |child| child.name == name)
    }

    /// The element's own character data, with entities resolved.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// Byte range of the start tag, `<…>` or `<…/>`.
    pub(crate) fn start_tag(&self) -> Range<usize> {
        self.start_tag.clone()
    }

    /// Byte range between the start and end tags; `None` for `<…/>`.
    pub(crate) fn content(&self) -> Option<Range<usize>> {
        self.content.clone()
    }

    /// Byte range of the whole element, from its start tag to its end tag.
    pub(crate) fn span(&self) -> Range<usize> {
        let end = self
            .end_tag
            .as_ref()
            .map_or(self.start_tag.end, |end_tag| end_tag.end);
        self.start_tag.start..end
    }

    /// The fields of a struct value: the `rdf:Description` inside a property
    /// or list item, or the element itself when the fields are written on it.
    pub(crate) fn resource(&self) -> &Element {
        self.child("rdf:Description").unwrap_or(self)
    }

    /// The items of an `rdf:Seq` property.
    pub(crate) fn seq_items(&self) -> impl Iterator<Item = &Element> + '_ {
        self.child("rdf:Seq")
            .into_iter()
            .flat_map(|seq| seq.children_named("rdf:li"))
    }

    /// The non-empty text items of an `rdf:Seq` property, trimmed.
    pub(crate) fn seq_texts(&self) -> Vec<&str> {
        self.seq_items()
            .map(|item| item.text.trim())
            .filter(|text| !text.is_empty())
            .collect()
    }

    /// The text of a language-alternative property: its default-language
    /// item, else its first item. A plain text property is accepted too.
    pub(crate) fn alt_text(&self) -> Option<&str> {
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

fn utf8(bytes: &[u8]) -> Result<&str, XmlError> {
    std::str::from_utf8(bytes).map_err(|_| invalid_xml("a name is not valid UTF-8"))
}

/// Parses `text` into a tree under an unnamed root element.
pub(crate) fn parse_document(text: &str) -> Result<Element, XmlError> {
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
                element.end_tag = Some(before..after);
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

fn decoded_reference(reference: &BytesRef<'_>) -> Result<String, XmlError> {
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

/// The top-level `rdf:Description` elements of an XMP packet, which hold its
/// properties, and the optional `x:xmpmeta` and `rdf:RDF` wrapping them.
pub(crate) struct Packet<'a> {
    /// `x:xmpmeta` and `rdf:RDF`, outermost first, where present.
    wrappers: Vec<&'a Element>,
    descriptions: Vec<&'a Element>,
}

impl<'a> Packet<'a> {
    /// Fails when the document holds no description.
    pub(crate) fn new(root: &'a Element) -> Result<Self, XmlError> {
        let mut parent = root;
        let mut wrappers = Vec::new();
        for wrapper in ["x:xmpmeta", "rdf:RDF"] {
            if let Some(inner) = parent.child(wrapper) {
                parent = inner;
                wrappers.push(inner);
            }
        }
        let descriptions: Vec<&Element> = parent.children_named("rdf:Description").collect();
        if descriptions.is_empty() {
            return Err(invalid_xml("no rdf:Description was found"));
        }
        Ok(Self {
            wrappers,
            descriptions,
        })
    }

    /// Every description, in document order; never empty.
    pub(crate) fn descriptions(&self) -> &[&'a Element] {
        &self.descriptions
    }

    /// The description new properties are added to.
    pub(crate) fn first_description(&self) -> &'a Element {
        self.descriptions[0]
    }

    /// The prefix bound to `namespace` where `description`'s properties are
    /// read: on the description itself or on a wrapper.
    pub(crate) fn prefix_in(&self, description: &'a Element, namespace: &str) -> Option<&'a str> {
        std::iter::once(description)
            .chain(self.wrappers.iter().rev().copied())
            .flat_map(Element::attributes)
            .find(|(_, value)| *value == namespace)
            .and_then(|(key, _)| key.strip_prefix("xmlns:"))
    }

    /// Whether `prefix` is bound, to any namespace, where `description`'s
    /// properties are read.
    pub(crate) fn binds_prefix(&self, description: &Element, prefix: &str) -> bool {
        let declaration = format!("xmlns:{prefix}");
        std::iter::once(description)
            .chain(self.wrappers.iter().copied())
            .any(|element| element.attribute(&declaration).is_some())
    }
}

/// Where text is inserted to add attributes to `element`'s start tag: before
/// its closing `>` or `/>`.
pub(crate) fn attribute_insertion_point(text: &str, element: &Element) -> usize {
    let tag = element.start_tag();
    let closing = if text[tag.clone()].ends_with("/>") {
        2
    } else {
        1
    };
    tag.end - closing
}

/// The byte range of the value of attribute `key` within `tag`, the source
/// of a start tag the XML reader has accepted.
pub(crate) fn attribute_value_span(tag: &str, key: &str) -> Option<Range<usize>> {
    attribute_spans(tag, key).map(|(_, value)| value)
}

/// The byte range of attribute `key` within `tag`, from the whitespace that
/// precedes its name to its closing quote, so removing it leaves a valid tag.
pub(crate) fn attribute_span(tag: &str, key: &str) -> Option<Range<usize>> {
    attribute_spans(tag, key).map(|(whole, _)| whole)
}

/// The whole attribute `key` with its leading whitespace, and its value.
fn attribute_spans(tag: &str, key: &str) -> Option<(Range<usize>, Range<usize>)> {
    let bytes = tag.as_bytes();
    // Past `<` and the element name.
    let mut cursor = bytes.iter().position(u8::is_ascii_whitespace)?;
    loop {
        let whitespace_start = cursor;
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
            return Some((whitespace_start..value_end + 1, value_start..value_end));
        }
        cursor = value_end + 1;
    }
}

/// `bom` and `text` with each `(range, replacement)` of `edits` applied. The
/// ranges index `text` and must not overlap.
pub(crate) fn apply_edits(
    bom: &[u8],
    text: &str,
    mut edits: Vec<(Range<usize>, String)>,
) -> Vec<u8> {
    edits.sort_by_key(|(range, _)| range.start);
    let source = text.as_bytes();
    let mut output = Vec::with_capacity(bom.len() + source.len() + 64);
    output.extend_from_slice(bom);
    let mut cursor = 0;
    for (range, replacement) in edits {
        output.extend_from_slice(&source[cursor..range.start]);
        output.extend_from_slice(replacement.as_bytes());
        cursor = range.end;
    }
    output.extend_from_slice(&source[cursor..]);
    output
}
