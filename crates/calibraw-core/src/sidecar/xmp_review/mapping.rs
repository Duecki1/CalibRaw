//! How a photo's review is stored in XMP, and the in-place edits that change
//! it while keeping every other byte of the file.
//!
//! - `xmp:Rating` holds the stars, 0 to 5. The XMP specification gives -1 to a
//!   rejected photo, which darktable, Bridge and FastRawViewer write; CalibRaw
//!   writes it for a rejected photo without stars, so no stars are lost.
//! - `xmpDM:pick` (-1, 0, 1) with `xmpDM:good` (`false`, absent, `true`) is
//!   Lightroom Classic's flag since 13.2.
//! - `digiKam:PickLabel` (0 none, 1 rejected, 2 pending, 3 accepted) is read,
//!   and kept current where a file already has it.
//!
//! - `Iptc4xmpExt:DigitalSourceType` marks an export AI Remove changed
//!   substantially; it is only written, into exports.
//!
//! The flag is read from `xmpDM:pick`, else `digiKam:PickLabel`, else a
//! rating of -1. Fractional ratings are rounded. Properties are found by
//! namespace, whatever prefix the file binds to it, as attributes of a
//! top-level `rdf:Description` or as its child elements.

use crate::sidecar::{PhotoFlag, PhotoReview, SidecarError};
use crate::xmp::{
    apply_edits, attribute_insertion_point, attribute_span, attribute_value_span, parse_document,
    split_bom, Element, Packet, TextError, XmlError,
};
use serde::{Deserialize, Serialize};
use std::ops::Range;

/// A file that adds properties gets this description when it has none.
const NEW_SIDECAR_HEAD: &str = concat!(
    "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"CalibRaw\">\n",
    " <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n",
    "  <rdf:Description rdf:about=\"\"",
);
const NEW_SIDECAR_TAIL: &str = "/>\n </rdf:RDF>\n</x:xmpmeta>\n";
/// Between attributes CalibRaw adds to a description.
const ATTRIBUTE_SEPARATOR: &str = "\n    ";
/// The wrapper of a packet embedded in an image file.
const PACKET_BEGIN: &str = "<?xpacket begin=\"\u{FEFF}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n";
const PACKET_END: &str = "<?xpacket end=\"w\"?>";

/// A property, by namespace and local name, with the prefix CalibRaw binds
/// to its namespace when a file has none.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Property {
    namespace: &'static str,
    prefix: &'static str,
    name: &'static str,
}

const RATING: Property = Property {
    namespace: "http://ns.adobe.com/xap/1.0/",
    prefix: "xmp",
    name: "Rating",
};
const PICK: Property = Property {
    namespace: "http://ns.adobe.com/xmp/1.0/DynamicMedia/",
    prefix: "xmpDM",
    name: "pick",
};
const GOOD: Property = Property {
    namespace: "http://ns.adobe.com/xmp/1.0/DynamicMedia/",
    prefix: "xmpDM",
    name: "good",
};
const PICK_LABEL: Property = Property {
    namespace: "http://www.digikam.org/ns/1.0/",
    prefix: "digiKam",
    name: "PickLabel",
};

/// The review an XMP file records: each field is `None` where the file has
/// no property for it. Serialized in the media library's folder caches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct XmpReview {
    rating: Option<u8>,
    flag: Option<PhotoFlag>,
}

impl XmpReview {
    /// Whether the file records neither a rating nor a flag.
    pub(super) fn is_empty(self) -> bool {
        self.rating.is_none() && self.flag.is_none()
    }

    /// `base` with each field this file records replaced by its value.
    pub fn over(self, base: PhotoReview) -> PhotoReview {
        PhotoReview {
            rating: self.rating.unwrap_or(base.rating),
            flag: self.flag.unwrap_or(base.flag),
        }
    }
}

/// The review recorded in the XMP document `bytes`.
pub(super) fn read_review(bytes: &[u8]) -> Result<XmpReview, SidecarError> {
    let (_, text) = split_bom(bytes).map_err(text_error)?;
    let root = parse_document(text).map_err(xml_error)?;
    let packet = Packet::new(&root).map_err(xml_error)?;
    let rating = first_value(&packet, RATING).and_then(stored_rating);
    let flag = first_value(&packet, PICK)
        .and_then(pick_flag)
        .or_else(|| first_value(&packet, PICK_LABEL).and_then(pick_label_flag))
        .or_else(|| (rating == Some(REJECTED_RATING)).then_some(PhotoFlag::Rejected));
    Ok(XmpReview {
        rating: rating.map(|rating| u8::try_from(rating).unwrap_or(0)),
        flag,
    })
}

/// `bytes`, an XMP document, storing `review`; `None` when it already does.
/// Fails without changing anything on a document CalibRaw cannot read.
pub(super) fn write_review(
    bytes: &[u8],
    review: PhotoReview,
) -> Result<Option<Vec<u8>>, SidecarError> {
    let (bom, text) = split_bom(bytes).map_err(text_error)?;
    let root = parse_document(text).map_err(xml_error)?;
    let packet = Packet::new(&root).map_err(xml_error)?;
    // New properties join those CalibRaw finds, as a file that keeps each
    // namespace in a description of its own keeps them together.
    let holder = [RATING, PICK, GOOD, PICK_LABEL]
        .into_iter()
        .find_map(|property| {
            occurrences(&packet, property)
                .first()
                .map(Occurrence::description)
        })
        .unwrap_or_else(|| packet.first_description());
    let mut edits = Vec::new();
    let mut added = Vec::new();
    for change in changes(review) {
        let occurrences = occurrences(&packet, change.property());
        if occurrences.is_empty() {
            if let Change::Set(value) = change {
                if value.added_where_missing() {
                    added.push(value);
                }
            }
            continue;
        }
        for occurrence in occurrences {
            if let Some(edit) = occurrence.edit(text, change) {
                edits.push(edit);
            }
        }
    }
    if !added.is_empty() {
        for value in &added {
            let property = value.property();
            if packet.prefix_in(holder, property.namespace).is_none()
                && packet.binds_prefix(holder, property.prefix)
            {
                return Err(SidecarError::Unsupported(format!(
                    "the XMP file binds the prefix {} to another namespace",
                    property.prefix
                )));
            }
        }
        let at = attribute_insertion_point(text, holder);
        let attributes = added_attributes(&added, |namespace| packet.prefix_in(holder, namespace));
        edits.push((at..at, attributes));
    }
    Ok((!edits.is_empty()).then(|| apply_edits(bom, text, edits)))
}

/// A new XMP sidecar holding only `review`.
pub(super) fn new_sidecar(review: PhotoReview) -> String {
    format!(
        "{NEW_SIDECAR_HEAD}{}{NEW_SIDECAR_TAIL}",
        added_attributes(&new_values(review), |_| None)
    )
}

/// The values a file without review properties gets for `review`.
fn new_values(review: PhotoReview) -> Vec<Stored> {
    changes(review)
        .into_iter()
        .filter_map(|change| match change {
            Change::Set(value) if value.added_where_missing() => Some(value),
            Change::Set(_) | Change::Remove(_) => None,
        })
        .collect()
}

/// The attributes that add `values` to a description: a declaration of each
/// namespace `bound` gives no prefix, with CalibRaw's prefix, then the
/// properties.
fn added_attributes<'a>(values: &[Stored], bound: impl Fn(&str) -> Option<&'a str>) -> String {
    let mut declarations = String::new();
    let mut properties = String::new();
    let mut declared: Vec<&str> = Vec::new();
    for value in values {
        let property = value.property();
        let prefix = match bound(property.namespace) {
            Some(prefix) => prefix,
            None => {
                if !declared.contains(&property.namespace) {
                    declared.push(property.namespace);
                    declarations.push_str(&format!(
                        "{ATTRIBUTE_SEPARATOR}xmlns:{}=\"{}\"",
                        property.prefix, property.namespace
                    ));
                }
                property.prefix
            }
        };
        properties.push_str(&format!(
            "{ATTRIBUTE_SEPARATOR}{prefix}:{}=\"{}\"",
            property.name,
            value.text()
        ));
    }
    declarations + &properties
}

/// IPTC's property for how an image was made.
const DIGITAL_SOURCE_TYPE: Property = Property {
    namespace: "http://iptc.org/std/Iptc4xmpExt/2008-02-29/",
    prefix: "Iptc4xmpExt",
    name: "DigitalSourceType",
};
/// The IPTC digital source type of a capture combined with content a trained
/// model generated, the mark an AI Remove fill gets.
const COMPOSITE_WITH_TRAINED_ALGORITHMIC_MEDIA: &str =
    "http://cv.iptc.org/newscodes/digitalsourcetype/compositeWithTrainedAlgorithmicMedia";

/// The XMP packet an exported file embeds: `review` when given and the photo
/// is rated or flagged, and the AI-edit mark when `ai_edited`
/// (`pipeline::is_ai_edited`). `None` when it would hold neither.
pub fn export_packet(review: Option<PhotoReview>, ai_edited: bool) -> Option<String> {
    let mut attributes = review
        .filter(|review| *review != PhotoReview::default())
        .map(|review| added_attributes(&new_values(review), |_| None))
        .unwrap_or_default();
    if ai_edited {
        let Property {
            namespace,
            prefix,
            name,
        } = DIGITAL_SOURCE_TYPE;
        attributes.push_str(&format!(
            "{ATTRIBUTE_SEPARATOR}xmlns:{prefix}=\"{namespace}\"\
             {ATTRIBUTE_SEPARATOR}{prefix}:{name}=\"{COMPOSITE_WITH_TRAINED_ALGORITHMIC_MEDIA}\""
        ));
    }
    (!attributes.is_empty()).then(|| {
        format!("{PACKET_BEGIN}{NEW_SIDECAR_HEAD}{attributes}{NEW_SIDECAR_TAIL}{PACKET_END}")
    })
}

/// The stored rating of a rejected photo.
const REJECTED_RATING: i8 = -1;

/// A stored `xmp:Rating`: -1 for rejected, else the stars.
fn stored_rating(raw: &str) -> Option<i8> {
    let value = parse_number(raw)?;
    Some(if value <= -0.5 {
        REJECTED_RATING
    } else {
        // In 0..=5 after the clamp, so the cast is exact.
        value.round().clamp(0.0, 5.0) as i8
    })
}

fn pick_flag(raw: &str) -> Option<PhotoFlag> {
    match parse_number(raw)?.round() as i32 {
        -1 => Some(PhotoFlag::Rejected),
        0 => Some(PhotoFlag::Unflagged),
        1 => Some(PhotoFlag::Picked),
        _ => None,
    }
}

/// digiKam's pick label; pending is not a flag CalibRaw has.
fn pick_label_flag(raw: &str) -> Option<PhotoFlag> {
    match raw.trim() {
        "0" | "2" => Some(PhotoFlag::Unflagged),
        "1" => Some(PhotoFlag::Rejected),
        "3" => Some(PhotoFlag::Picked),
        _ => None,
    }
}

fn parse_boolean(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// A number as XMP writes it, with an optional leading `+`.
fn parse_number(raw: &str) -> Option<f32> {
    let raw = raw.trim();
    let raw = raw.strip_prefix('+').unwrap_or(raw);
    raw.parse::<f32>().ok().filter(|value| value.is_finite())
}

/// A property value CalibRaw stores.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stored {
    Rating(i8),
    Pick(PhotoFlag),
    Good(bool),
    PickLabel(PhotoFlag),
}

impl Stored {
    fn property(self) -> Property {
        match self {
            Self::Rating(_) => RATING,
            Self::Pick(_) => PICK,
            Self::Good(_) => GOOD,
            Self::PickLabel(_) => PICK_LABEL,
        }
    }

    fn text(self) -> String {
        match self {
            Self::Rating(rating) => rating.to_string(),
            Self::Pick(flag) => match flag {
                PhotoFlag::Rejected => "-1",
                PhotoFlag::Unflagged => "0",
                PhotoFlag::Picked => "1",
            }
            .to_owned(),
            Self::Good(good) => good.to_string(),
            Self::PickLabel(flag) => match flag {
                PhotoFlag::Rejected => "1",
                PhotoFlag::Unflagged => "0",
                PhotoFlag::Picked => "3",
            }
            .to_owned(),
        }
    }

    /// Whether `raw`, the value a file holds, already reads as this value,
    /// so it is left as written.
    fn matches(self, raw: &str) -> bool {
        match self {
            Self::Rating(rating) => stored_rating(raw) == Some(rating),
            Self::Pick(flag) => pick_flag(raw) == Some(flag),
            Self::Good(good) => parse_boolean(raw) == Some(good),
            Self::PickLabel(flag) => pick_label_flag(raw) == Some(flag),
        }
    }

    /// Whether a file without the property gets it. digiKam's label is only
    /// kept current where digiKam wrote it.
    fn added_where_missing(self) -> bool {
        !matches!(self, Self::PickLabel(_))
    }
}

#[derive(Clone, Copy)]
enum Change {
    Set(Stored),
    Remove(Property),
}

impl Change {
    fn property(self) -> Property {
        match self {
            Self::Set(value) => value.property(),
            Self::Remove(property) => property,
        }
    }
}

/// What storing `review` sets and removes.
fn changes(review: PhotoReview) -> [Change; 4] {
    let rating = if review.flag == PhotoFlag::Rejected && review.rating == 0 {
        REJECTED_RATING
    } else {
        // At most 5: reviews are validated before they are stored.
        i8::try_from(review.rating.min(5)).unwrap_or(5)
    };
    let good = match review.flag {
        PhotoFlag::Rejected => Change::Set(Stored::Good(false)),
        PhotoFlag::Unflagged => Change::Remove(GOOD),
        PhotoFlag::Picked => Change::Set(Stored::Good(true)),
    };
    [
        Change::Set(Stored::Rating(rating)),
        Change::Set(Stored::Pick(review.flag)),
        good,
        Change::Set(Stored::PickLabel(review.flag)),
    ]
}

/// Where a file holds a property.
enum Occurrence<'a> {
    /// An attribute of a description's start tag, by qualified name.
    Attribute {
        description: &'a Element,
        key: String,
        value: &'a str,
    },
    /// A child element of a description.
    Element {
        description: &'a Element,
        element: &'a Element,
    },
}

impl<'a> Occurrence<'a> {
    /// The description holding the property.
    fn description(&self) -> &'a Element {
        match self {
            Self::Attribute { description, .. } | Self::Element { description, .. } => description,
        }
    }

    fn value(&self) -> &'a str {
        match self {
            Self::Attribute { value, .. } => value,
            Self::Element { element, .. } => element.text().trim(),
        }
    }

    /// The edit that makes this occurrence hold `change`; `None` when it
    /// already does.
    fn edit(&self, text: &str, change: Change) -> Option<(Range<usize>, String)> {
        match (self, change) {
            (_, Change::Set(value)) if value.matches(self.value()) => None,
            (
                Self::Attribute {
                    description, key, ..
                },
                Change::Set(value),
            ) => {
                let tag = description.start_tag();
                let span = attribute_value_span(&text[tag.clone()], key)?;
                Some((tag.start + span.start..tag.start + span.end, value.text()))
            }
            (Self::Element { element, .. }, Change::Set(value)) => Some(match element.content() {
                Some(content) => (content, value.text()),
                // `<xmp:Rating/>` becomes `<xmp:Rating>…</xmp:Rating>`.
                None => {
                    let tag = &text[element.start_tag()];
                    let open = tag.strip_suffix("/>").unwrap_or(tag).trim_end();
                    let name = element.name();
                    (
                        element.start_tag(),
                        format!("{open}>{}</{name}>", value.text()),
                    )
                }
            }),
            (
                Self::Attribute {
                    description, key, ..
                },
                Change::Remove(_),
            ) => {
                let tag = description.start_tag();
                let span = attribute_span(&text[tag.clone()], key)?;
                Some((tag.start + span.start..tag.start + span.end, String::new()))
            }
            (Self::Element { element, .. }, Change::Remove(_)) => {
                Some((element.span(), String::new()))
            }
        }
    }
}

/// Every place the packet holds `property`, in document order.
fn occurrences<'a>(packet: &Packet<'a>, property: Property) -> Vec<Occurrence<'a>> {
    let mut found = Vec::new();
    for &description in packet.descriptions() {
        let Some(prefix) = packet.prefix_in(description, property.namespace) else {
            continue;
        };
        let key = format!("{prefix}:{}", property.name);
        if let Some(value) = description.attribute(&key) {
            found.push(Occurrence::Attribute {
                description,
                key: key.clone(),
                value,
            });
        }
        found.extend(
            description
                .children()
                .filter(|child| child.name() == key)
                .map(|element| Occurrence::Element {
                    description,
                    element,
                }),
        );
    }
    found
}

fn first_value<'a>(packet: &Packet<'a>, property: Property) -> Option<&'a str> {
    occurrences(packet, property).first().map(Occurrence::value)
}

fn text_error(error: TextError) -> SidecarError {
    SidecarError::Unsupported(
        match error {
            TextError::Utf16 => "UTF-16 XMP files are not supported",
            TextError::NotUtf8 => "the XMP file is not valid UTF-8",
        }
        .to_owned(),
    )
}

fn xml_error(error: XmlError) -> SidecarError {
    SidecarError::Invalid(error.to_string())
}
