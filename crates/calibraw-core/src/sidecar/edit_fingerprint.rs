//! Fingerprints that tell whether a photo's edits differ from how it started.
//!
//! A photo does not start at [`default_edit_state`]: its first open adds
//! camera- and ISO-dependent detail defaults, the new-photo preset and the
//! processing settings carried over from the previous photo. A sidecar
//! therefore records the fingerprint of that start beside the fingerprint of
//! its edits, and the photo counts as edited while the two differ. A photo
//! whose start is not known starts at the defaults.
//!
//! Sidecars keep these fingerprints across releases, so they hash what makes
//! the edits differ from the defaults, not the serialized edits themselves:
//! a field added at its default, or a retired one written at rest, leaves
//! every fingerprint as it was. The hash is FNV-1a, whose output, unlike
//! `std`'s `DefaultHasher`, is fixed.

use super::*;
use serde::de::Error as _;
use serde::{Deserializer, Serializer};
use serde_json::Value;

/// Bookkeeping in the serialized edits that is not an edit.
const BOOKKEEPING_FIELDS: [&str; 1] = ["ai_masks_need_update"];

/// A fingerprint of an edit: equal for edits that differ from the defaults in
/// the same values. Serialized as 16 hexadecimal digits, which JSON tools
/// that read numbers as doubles keep intact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EditFingerprint(u64);

impl EditFingerprint {
    pub fn of(edits: &EditState) -> Result<Self, SidecarError> {
        Self::of_json(edits_value(edits)?)
    }

    /// The start of a photo whose start is not known.
    pub fn of_defaults() -> Result<Self, SidecarError> {
        Self::of(&default_edit_state())
    }

    /// The fingerprint of `edits` serialized as a sidecar's `edits` object.
    pub(crate) fn of_json(mut edits: Value) -> Result<Self, SidecarError> {
        let mut defaults = edits_value(&default_edit_state())?;
        for value in [&mut edits, &mut defaults] {
            if let Some(fields) = value.as_object_mut() {
                for field in BOOKKEEPING_FIELDS {
                    fields.remove(field);
                }
            }
        }
        let mut hasher = Fnv1a::new();
        hash_differences(&mut hasher, &mut String::new(), &edits, &defaults)?;
        Ok(Self(hasher.finish()))
    }
}

impl Serialize for EditFingerprint {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{:016x}", self.0))
    }
}

impl<'de> Deserialize<'de> for EditFingerprint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let digits = String::deserialize(deserializer)?;
        u64::from_str_radix(&digits, 16)
            .map(Self)
            .map_err(|_| D::Error::custom("an edit fingerprint is 16 hexadecimal digits"))
    }
}

/// Which start a sidecar write records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidecarStart {
    /// The start the sidecar already has, or the defaults when it has none.
    Keep,
    Set(EditFingerprint),
}

impl SidecarStart {
    /// The start to write over a sidecar that records `recorded`.
    pub fn fingerprint(
        self,
        recorded: Option<EditFingerprint>,
    ) -> Result<EditFingerprint, SidecarError> {
        match (self, recorded) {
            (Self::Set(start), _) | (Self::Keep, Some(start)) => Ok(start),
            (Self::Keep, None) => EditFingerprint::of_defaults(),
        }
    }
}

/// `edits` as decoding leaves it, serialized: tone curves sanitized and the
/// subject refinement in both of its places, so edits hash the same before
/// they are saved and after they are read back.
fn edits_value(edits: &EditState) -> Result<Value, SidecarError> {
    let mut edits = edits.clone();
    synchronize_subject_refinement(&mut edits);
    edits.exposure.sanitize_tone_curves();
    for mask in &mut Arc::make_mut(&mut edits.masks).masks {
        mask.adjustments.sanitize_tone_curves();
    }
    serde_json::to_value(&edits)
        .map_err(|error| SidecarError::Invalid(format!("could not fingerprint edit: {error}")))
}

/// Hashes every value of `value` that differs from `default` at the same
/// path, with its path. Numbers compare by value, so `-0.0`, which slider
/// rounding can leave, equals `0.0`. A field missing from `value` is hashed
/// as `null`: serialization leaves out fields at a skip value that differs
/// from the default the defaults write.
fn hash_differences(
    hasher: &mut Fnv1a,
    path: &mut String,
    value: &Value,
    default: &Value,
) -> Result<(), SidecarError> {
    if value == default {
        return Ok(());
    }
    match (value, default) {
        (Value::Object(fields), Value::Object(defaults)) => {
            for (key, field) in fields {
                let default = defaults.get(key).unwrap_or(&Value::Null);
                with_key(path, key, |path| {
                    hash_differences(hasher, path, field, default)
                })?;
            }
            for (key, default) in defaults {
                if !fields.contains_key(key) {
                    with_key(path, key, |path| {
                        hash_differences(hasher, path, &Value::Null, default)
                    })?;
                }
            }
            Ok(())
        }
        _ => {
            hasher.write_all(path.as_bytes())?;
            hasher.write_all(b"=")?;
            let mut serializer = serde_json::Serializer::with_formatter(&mut *hasher, SignlessZero);
            value.serialize(&mut serializer).map_err(|error| {
                SidecarError::Invalid(format!("could not fingerprint edit: {error}"))
            })?;
            hasher.write_all(b"\n")?;
            Ok(())
        }
    }
}

fn with_key<T>(path: &mut String, key: &str, visit: impl FnOnce(&mut String) -> T) -> T {
    let length = path.len();
    path.push('/');
    path.push_str(key);
    let result = visit(path);
    path.truncate(length);
    result
}

/// Compact JSON that writes `-0.0` as `0.0`, for differing values that hold
/// a negative zero, such as a changed tone curve.
struct SignlessZero;

impl serde_json::ser::Formatter for SignlessZero {
    fn write_f32<W: ?Sized + Write>(&mut self, writer: &mut W, value: f32) -> std::io::Result<()> {
        serde_json::ser::CompactFormatter.write_f32(writer, if value == 0.0 { 0.0 } else { value })
    }

    fn write_f64<W: ?Sized + Write>(&mut self, writer: &mut W, value: f64) -> std::io::Result<()> {
        serde_json::ser::CompactFormatter.write_f64(writer, if value == 0.0 { 0.0 } else { value })
    }
}

/// 64-bit FNV-1a, whose output stays the same across builds and releases.
pub(super) struct Fnv1a(u64);

impl Fnv1a {
    pub(super) const fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    pub(super) const fn finish(&self) -> u64 {
        self.0
    }
}

impl Write for Fnv1a {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
