//! Creative 3D LUTs: `.cube` parsing, the LUT an edit applies, and the
//! application's LUT library.
//!
//! An edit applies at most one LUT, after tone mapping and before grain and
//! vignette, to display-referred sRGB-encoded values (see
//! `calibraw-gpu/src/shaders/look_lut.wgsl`). Edits name the LUT by
//! [`LutId`], a fingerprint of the file's bytes, so the same file imported on
//! another device resolves to the same LUT.

mod cube;
mod library;

pub use cube::{
    parse_cube, CubeLut, CubeLutError, MAX_CUBE_FILE_BYTES, MAX_LUT_SIZE, MIN_LUT_SIZE,
};
pub use library::{
    lut_folder_for_settings, LutEntry, LutImport, LutLibrary, LutLibraryError, LutStore,
    MAX_LUT_NAME_CHARS,
};

use crate::fnv;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

/// Identifies a LUT by the FNV-1a 64 fingerprint of its `.cube` file bytes.
/// Serialized as 16 lowercase hexadecimal digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LutId(u64);

impl LutId {
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self(
            bytes
                .iter()
                .fold(fnv::OFFSET_BASIS, |hash, byte| fnv::step(hash, *byte)),
        )
    }

    pub const fn fingerprint(self) -> u64 {
        self.0
    }
}

impl fmt::Display for LutId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:016x}", self.0)
    }
}

impl FromStr for LutId {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.len() != 16 {
            return Err(format!("LUT id {text:?} is not 16 hexadecimal digits"));
        }
        u64::from_str_radix(text, 16)
            .map(Self)
            .map_err(|_| format!("LUT id {text:?} is not 16 hexadecimal digits"))
    }
}

impl Serialize for LutId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for LutId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// The LUT an edit applies.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct LookLut {
    pub id: LutId,
    /// The library name when the LUT was chosen, kept so a LUT missing from
    /// this device's library can still be named.
    pub name: String,
    /// How much of the LUT's change is applied, 0–1.
    #[serde(default = "full_amount")]
    pub amount: f32,
}

impl LookLut {
    pub const DEFAULT_AMOUNT: f32 = 1.0;

    pub fn new(entry: &LutEntry) -> Self {
        Self {
            id: entry.id,
            name: entry.name.clone(),
            amount: Self::DEFAULT_AMOUNT,
        }
    }
}

const fn full_amount() -> f32 {
    LookLut::DEFAULT_AMOUNT
}

/// A LUT loaded for rendering, with the amount an edit applies it at.
#[derive(Clone, Debug)]
pub struct ResolvedLook {
    pub lut: Arc<CubeLut>,
    /// 0–1, as [`LookLut::amount`].
    pub amount: f32,
}

#[cfg(test)]
mod tests;
