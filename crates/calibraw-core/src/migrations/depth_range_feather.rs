//! Depth-range masks saved with one feather for both ends, before v1.2.0.
//!
//! migration: remove in v2.0.0
//!
//! The near feather keeps the serialized `feather` key, and `far_feather`
//! was added beside it. Without `far_feather`, `feather` softens both ends.
//! Once removed, `DepthRangeSettings` deserializes directly.

use crate::pipeline::DepthRangeSettings;
use serde::Deserialize;

/// `DepthRangeSettings` as stored by any version. The near feather is always
/// stored as `feather`.
#[derive(Deserialize)]
pub(crate) struct StoredDepthRange {
    near: f32,
    far: f32,
    #[serde(default)]
    feather: Option<f32>,
    #[serde(default)]
    far_feather: Option<f32>,
}

impl From<StoredDepthRange> for DepthRangeSettings {
    fn from(stored: StoredDepthRange) -> Self {
        let near_feather = stored
            .feather
            .unwrap_or(DepthRangeSettings::default().near_feather);
        Self {
            near: stored.near,
            far: stored.far,
            near_feather,
            far_feather: stored.far_feather.unwrap_or(near_feather),
        }
    }
}
