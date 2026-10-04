//! `#[serde(with = "crate::base64_arc_bytes")]` for `Arc<[u8]>` fields stored as standard,
//! padded base64 strings in sidecars, mask images and retouch patches.

use base64::Engine as _;
use serde::{Deserialize, Deserializer, Serializer};
use std::sync::Arc;

pub(crate) fn serialize<S>(bytes: &Arc<[u8]>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.collect_str(&base64::display::Base64Display::new(
        bytes.as_ref(),
        &base64::engine::general_purpose::STANDARD,
    ))
}

pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Arc<[u8]>, D::Error>
where
    D: Deserializer<'de>,
{
    let encoded = String::deserialize(deserializer)?;
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map(Arc::from)
        .map_err(serde::de::Error::custom)
}
