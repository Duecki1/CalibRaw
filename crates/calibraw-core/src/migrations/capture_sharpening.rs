//! Capture sharpening for edits saved by v1.5.1 and earlier.
//!
//! migration: remove in v2.0.0
//!
//! The Detail card's Amount, Radius, Detail and Masking boosted local
//! contrast in the scene tone pass, kept within each pixel's neighbourhood
//! (`detail_capture.wgsl`). New edits deconvolve in the RAW stage
//! ([`SharpenMethod::Deconvolution`]), and each stores its method beside the
//! controls. An edit saved without one that sharpens keeps rendering as it
//! did: it becomes [`SharpenMethod::Legacy`], with Amount 40 and Radius 1
//! where it saved none, the defaults those versions read for them (now
//! Amount 30, and Radius 0 for the photo's own). One that does not sharpen
//! takes the new method with the photo's own radius, so raising its Amount
//! deconvolves. A preset keeps only Amount, Detail and Masking: building it
//! (`transfer_edits`) moves the legacy method to deconvolution, as pasting
//! an old edit's settings does.
//!
//! [`LegacyCaptureSharpening`] reads what a serialized `edits` object holds
//! in the same pass as the decoded edit. Checks that compare a serialized
//! `edits` object with the defaults without decoding it compare it with
//! [`defaults_written_beside`] it.
//!
//! To remove: delete this file, [`SharpenMethod::Legacy`] and the global
//! capture sharpening of the scene tone pass it selects
//! (`apply_capture_sharpening_at` in `scene_adjustments.wgsl`, packed by
//! `pack_effect_params` in calibraw-gpu), and read a stored `"legacy"` as
//! [`SharpenMethod::Deconvolution`] (`#[serde(alias)]`), so files that hold
//! it still open. The Sharpen effect keeps `detail_capture.wgsl`.

use crate::pipeline::SharpenMethod;
use crate::sidecar::{default_edit_state, encode, EditState, SidecarError};
use serde::de::IgnoredAny;
use serde::Deserialize;
use serde_json::Value;

/// The defaults of the versions that saved no method, as their edits decode:
/// the legacy method at Amount 40 and Radius 1.
pub(crate) fn defaults_before_methods() -> EditState {
    let mut defaults = default_edit_state();
    defaults.exposure.sharpen_method = SharpenMethod::Legacy;
    defaults.exposure.sharpen_amount = SharpenMethod::LEGACY_DEFAULT_AMOUNT;
    defaults.exposure.sharpen_radius = SharpenMethod::LEGACY_DEFAULT_RADIUS;
    defaults
}

/// The `edits` object a sidecar at the defaults holds, as the version that
/// saved the serialized `edits` wrote it: [`defaults_before_methods`] without
/// a method when `edits` has none (those versions always wrote Amount and
/// Radius), the current defaults otherwise.
///
/// Read back from the sidecar's bytes, as a saved `edits` object is: a
/// `Value` made from the edit directly holds each `f32` widened (0.0152 as
/// 0.015200000256…), which compares unequal with the same field read from a
/// file.
pub(crate) fn defaults_written_beside(edits: &Value) -> Result<Value, SidecarError> {
    let without_method = edits
        .get("exposure")
        .is_some_and(|exposure| exposure.get("sharpen_method").is_none());
    let defaults = if without_method {
        defaults_before_methods()
    } else {
        default_edit_state()
    };
    let mut document: Value = serde_json::from_slice(&encode(defaults)?)
        .map_err(|error| SidecarError::Invalid(error.to_string()))?;
    let mut defaults = document
        .get_mut("edits")
        .map(Value::take)
        .ok_or_else(|| SidecarError::Invalid("encoded sidecar has no edits".to_owned()))?;
    if without_method {
        if let Some(exposure) = defaults.get_mut("exposure").and_then(Value::as_object_mut) {
            exposure.remove("sharpen_method");
        }
    }
    Ok(defaults)
}

/// Which capture sharpening fields a serialized `edits` object holds. Every
/// other field is ignored, so this reads any edit, old or current.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct LegacyCaptureSharpening {
    #[serde(default)]
    exposure: SavedExposure,
}

#[derive(Debug, Default, Deserialize)]
struct SavedExposure {
    #[serde(default)]
    sharpen_method: Option<IgnoredAny>,
    #[serde(default)]
    sharpen_amount: Option<IgnoredAny>,
    #[serde(default)]
    sharpen_radius: Option<IgnoredAny>,
}

impl LegacyCaptureSharpening {
    /// Gives `edits`, decoded from the same `edits` object as `self` and
    /// saved without a method, the legacy method when it sharpens and the
    /// photo's own radius when it does not. Returns whether it changed.
    pub(crate) fn migrate(self, edits: &mut EditState) -> bool {
        let exposure = &mut edits.exposure;
        if self.exposure.sharpen_method.is_some() {
            return false;
        }
        if self.exposure.sharpen_amount.is_none() {
            exposure.sharpen_amount = SharpenMethod::LEGACY_DEFAULT_AMOUNT;
        }
        if exposure.sharpen_amount <= 0.0 {
            // Its Radius is in the legacy method's units: the photo's own
            // radius takes its place, which renders alike at Amount 0.
            let migrated = exposure.sharpen_radius != 0.0;
            exposure.sharpen_radius = 0.0;
            return migrated;
        }
        exposure.sharpen_method = SharpenMethod::Legacy;
        if self.exposure.sharpen_radius.is_none() {
            exposure.sharpen_radius = SharpenMethod::LEGACY_DEFAULT_RADIUS;
        }
        true
    }
}
