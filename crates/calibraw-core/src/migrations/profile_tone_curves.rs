//! The camera profile's tone curve for edits saved by v1.5.1 and earlier.
//!
//! migration: remove in v2.0.0
//!
//! The sigmoid rendered every photo and a DCP's ProfileToneCurve went unused.
//! New edits render with the curve ([`ProfileToneCurve::Applied`]) and store
//! the choice in `exposure`. A sidecar saved without it keeps the rendering
//! it was edited with, [`ProfileToneCurve::Ignored`], while its photo differs
//! from its start; a photo still at its start takes the curve, as one opened
//! for the first time does. A preset saved without it ignores the curve when
//! it carries the RAW processing settings, which hold the choice; any other
//! preset does not set it.
//!
//! [`LegacyProfileToneCurve`] reads whether a serialized `edits` object holds
//! the choice, in the same pass as the decoded edit.
//!
//! To remove: delete this file and its calls in `sidecar.rs` and
//! `presets.rs`. A file without the field then reads as
//! [`ProfileToneCurve::Applied`].

use crate::pipeline::{ExposureParams, ProfileToneCurve};
use serde::de::IgnoredAny;
use serde::Deserialize;

/// Whether a serialized `edits` object holds the profile tone curve choice.
/// Every other field is ignored, so this reads any edit, old or current.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub(crate) struct LegacyProfileToneCurve {
    #[serde(default)]
    exposure: SavedExposure,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
struct SavedExposure {
    #[serde(default)]
    profile_tone_curve: Option<IgnoredAny>,
}

impl LegacyProfileToneCurve {
    /// Gives `exposure`, decoded from the same `edits` object as `self`, the
    /// choice it rendered with when the file holds none: the curve ignored
    /// when `keeps_rendering`, else left at its default. Returns whether the
    /// file holds none, so that it is saved again in the current layout.
    pub(crate) fn migrate(self, exposure: &mut ExposureParams, keeps_rendering: bool) -> bool {
        if self.exposure.profile_tone_curve.is_some() {
            return false;
        }
        if keeps_rendering {
            exposure.profile_tone_curve = ProfileToneCurve::Ignored;
        }
        true
    }
}
