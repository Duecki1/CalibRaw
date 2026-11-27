//! Sidecars saved without edit fingerprints, by v1.4 and older.
//!
//! migration: remove in v2.0.0
//!
//! Such a sidecar records no start, so its photo starts at the defaults, as
//! for every photo whose start is not known. Saving it again records both
//! fingerprints. Once removed, a sidecar without them reads as unedited.

use crate::pipeline::SharpenMethod;
use crate::sidecar::{default_edit_state, EditFingerprint, EditState, SidecarError};
use serde::Deserialize;
use serde_json::Value;

/// Whether `edits`, decoded from a sidecar without fingerprints, differ from
/// the defaults of the versions that saved such sidecars.
pub(crate) fn edited_without_fingerprints(edits: &EditState) -> Result<bool, SidecarError> {
    Ok(EditFingerprint::of(edits)? != EditFingerprint::of(&saved_defaults())?)
}

/// The defaults as those versions had them: capture sharpening by the legacy
/// method at Amount 40 and Radius 1, which their edits decode to
/// (`capture_sharpening`).
fn saved_defaults() -> EditState {
    let mut defaults = default_edit_state();
    defaults.exposure.sharpen_method = SharpenMethod::Legacy;
    defaults.exposure.sharpen_amount = SharpenMethod::LEGACY_DEFAULT_AMOUNT;
    defaults.exposure.sharpen_radius = SharpenMethod::LEGACY_DEFAULT_RADIUS;
    defaults
}

/// The same for the sidecar `bytes`, without decoding its edits: compares the
/// `edits` object as saved with [`saved_defaults`] as those versions wrote
/// them.
pub(crate) fn edited_without_fingerprints_in(bytes: &[u8]) -> Result<bool, SidecarError> {
    #[derive(Deserialize)]
    struct Document {
        edits: Value,
    }
    let document: Document =
        serde_json::from_slice(bytes).map_err(|error| SidecarError::Invalid(error.to_string()))?;
    Ok(EditFingerprint::of_json(document.edits)?
        != EditFingerprint::of_json(saved_defaults_as_written()?)?)
}

/// [`saved_defaults`] serialized as those versions wrote it: without a
/// sharpening method, which they did not have. They always wrote Amount and
/// Radius.
///
/// Read back from its bytes, as a sidecar's edits are: a `Value` made from
/// the edit directly holds each `f32` widened (0.0152 as 0.015200000256…),
/// which compares unequal with the same field read from a file.
fn saved_defaults_as_written() -> Result<Value, SidecarError> {
    let invalid = |error: serde_json::Error| {
        SidecarError::Invalid(format!("could not serialize defaults: {error}"))
    };
    let bytes = serde_json::to_vec(&saved_defaults()).map_err(invalid)?;
    let mut defaults: Value = serde_json::from_slice(&bytes).map_err(invalid)?;
    if let Some(exposure) = defaults.get_mut("exposure").and_then(Value::as_object_mut) {
        exposure.remove("sharpen_method");
    }
    Ok(defaults)
}
