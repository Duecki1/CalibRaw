//! Tone responses for edits saved by v1.4.1 and earlier.
//!
//! migration: remove in v2.0.0
//!
//! The master tone curve and Blacks, globally and in a mask, and a mask's
//! Contrast mapped the luminance and scaled RGB by its ratio, which keeps
//! chromaticity, so contrast added no saturation. New edits use
//! hue-locked RGB tone ([`ToneResponse::Rgb`]), and each stores its response
//! beside its value. A control saved without one keeps rendering as it did:
//! it becomes [`ToneResponse::Luminance`] when it changes the image. One at
//! its default renders the same either way, so it takes the new response.
//!
//! [`LegacyToneResponses`] reads which responses a serialized `edits` object
//! holds, by mask position, in the same pass as the decoded edit.
//!
//! To remove: delete this file, [`ToneResponse::Luminance`] and the shader
//! branches marked with this version (`tonemap.wgsl`,
//! `scene_adjustments.wgsl`, `view_transform.wgsl`) with the flags that
//! select them (`legacy_luminance_tone` and an adjustment's `adjust_3` in
//! calibraw-gpu), and read a stored `"luminance"` as [`ToneResponse::Rgb`]
//! (`#[serde(alias)]`), so files that hold it still open.

use crate::pipeline::{LocalAdjustments, ToneResponse};
use crate::sidecar::EditState;
use serde::de::IgnoredAny;
use serde::Deserialize;
use std::sync::Arc;

/// Which tone responses a serialized `edits` object holds. Every other field
/// is ignored, so this reads any edit, old or current.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct LegacyToneResponses {
    #[serde(default)]
    exposure: SavedExposure,
    #[serde(default)]
    masks: SavedMasks,
}

#[derive(Debug, Default, Deserialize)]
struct SavedExposure {
    #[serde(default)]
    tone_curve_response: Option<IgnoredAny>,
    #[serde(default)]
    blacks_response: Option<IgnoredAny>,
}

#[derive(Debug, Default, Deserialize)]
struct SavedMasks {
    /// In the same order as the decoded edit's masks.
    #[serde(default)]
    masks: Vec<SavedMask>,
}

#[derive(Debug, Default, Deserialize)]
struct SavedMask {
    #[serde(default)]
    adjustments: SavedAdjustments,
}

#[derive(Debug, Default, Deserialize)]
struct SavedAdjustments {
    #[serde(default)]
    tone_curve_response: Option<IgnoredAny>,
    #[serde(default)]
    contrast_response: Option<IgnoredAny>,
    #[serde(default)]
    blacks_response: Option<IgnoredAny>,
}

impl SavedAdjustments {
    /// The responses `adjustments`, decoded from the same mask, get: the
    /// luminance response for each control saved without one that changes
    /// the image. `None` when none does.
    fn legacy(&self, adjustments: &LocalAdjustments) -> Option<LocalAdjustments> {
        let defaults = LocalAdjustments::default();
        let mut migrated = *adjustments;
        let mut changed = false;
        if self.tone_curve_response.is_none() && !adjustments.tone_curve.is_identity() {
            migrated.tone_curve_response = ToneResponse::Luminance;
            changed = true;
        }
        if self.contrast_response.is_none() && adjustments.contrast != defaults.contrast {
            migrated.contrast_response = ToneResponse::Luminance;
            changed = true;
        }
        if self.blacks_response.is_none() && adjustments.blacks != defaults.blacks {
            migrated.blacks_response = ToneResponse::Luminance;
            changed = true;
        }
        changed.then_some(migrated)
    }
}

impl LegacyToneResponses {
    /// Gives every tone control of `edits` saved without a response, and
    /// changing the image, the luminance response it rendered with. `edits`
    /// must be decoded from the same `edits` object as `self`, before any
    /// mask is removed or reordered. Returns whether any control changed.
    pub(crate) fn migrate(self, edits: &mut EditState) -> bool {
        let mut migrated = false;
        let exposure = &mut edits.exposure;
        if self.exposure.tone_curve_response.is_none() && !exposure.tone_curve.is_identity() {
            exposure.tone_curve_response = ToneResponse::Luminance;
            migrated = true;
        }
        if self.exposure.blacks_response.is_none() && exposure.blacks != 0.0 {
            exposure.blacks_response = ToneResponse::Luminance;
            migrated = true;
        }

        let masks: Vec<_> = edits
            .masks
            .masks
            .iter()
            .zip(&self.masks.masks)
            .map(|(mask, saved)| saved.adjustments.legacy(&mask.adjustments))
            .collect();
        if masks.iter().any(Option::is_some) {
            let stack = Arc::make_mut(&mut edits.masks);
            for (mask, legacy) in stack.masks.iter_mut().zip(masks) {
                if let Some(adjustments) = legacy {
                    mask.adjustments = adjustments;
                }
            }
            migrated = true;
        }
        migrated
    }
}
