//! Tone curves saved with eight point slots, before v1.1.1.
//!
//! migration: remove in v2.0.0
//!
//! Curves now store `MAX_POINT_CURVE_POINTS` slots. Eight-slot curves are
//! padded with unused points. Once removed, `PointCurve` deserializes its
//! points without this function.

use crate::pipeline::MAX_POINT_CURVE_POINTS;
use serde::{Deserialize, Deserializer};

const OLD_SLOT_COUNT: usize = 8;

/// Reads point slots in the current or the eight-slot layout.
pub(crate) fn deserialize<'de, D>(
    deserializer: D,
) -> Result<[[f32; 2]; MAX_POINT_CURVE_POINTS], D::Error>
where
    D: Deserializer<'de>,
{
    let stored = Vec::<[f32; 2]>::deserialize(deserializer)?;
    if stored.len() != OLD_SLOT_COUNT && stored.len() != MAX_POINT_CURVE_POINTS {
        return Err(serde::de::Error::custom(
            "expected 8 or 16 tone curve point slots",
        ));
    }
    let mut points = [[1.0, 1.0]; MAX_POINT_CURVE_POINTS];
    points[..stored.len()].copy_from_slice(&stored);
    Ok(points)
}
