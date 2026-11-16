//! What turns a decoded RAW into the source an edit develops.

use super::{apply_lens_correction, remove_hot_pixels, LensCorrection, LoadedRaw};
use anyhow::Result;
use std::sync::Arc;

/// The corrections a decoded RAW takes before it is developed. Hot pixels
/// are repaired in the mosaic as decoded, then the lens correction resamples
/// it, which would spread each hot pixel over its neighbours. Every renderer
/// prepares its source through [`Self::apply`]: the editor, exports, batch
/// exports, edited thumbnails, the command-line export and render parity.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceCorrections {
    /// Repair the hot pixels ([`remove_hot_pixels`]): the edit's
    /// `hot_pixel_removal` as it renders (`ExposureParams::removes_hot_pixels`).
    pub hot_pixels: bool,
    /// The lens correction; `None` keeps the original geometry.
    pub lens: Option<LensCorrection>,
}

impl SourceCorrections {
    /// Whether the corrections leave every RAW as decoded.
    pub fn is_none(&self) -> bool {
        !self.hot_pixels && self.lens.is_none()
    }

    /// `raw` corrected, or `None` when the corrections change nothing: none
    /// is asked for, or hot pixel removal alone on a RAW without any. Fails
    /// when the lens correction fails; hot pixel removal cannot.
    pub fn apply(&self, raw: &LoadedRaw) -> Result<Option<LoadedRaw>> {
        let repaired = self.hot_pixels.then(|| remove_hot_pixels(raw)).flatten();
        match &self.lens {
            Some(lens) => apply_lens_correction(repaired.as_ref().unwrap_or(raw), lens).map(Some),
            None => Ok(repaired),
        }
    }

    /// [`Self::apply`] to a shared decode, which is returned itself when the
    /// corrections change nothing.
    pub fn apply_shared(&self, raw: &Arc<LoadedRaw>) -> Result<Arc<LoadedRaw>> {
        Ok(self.apply(raw)?.map_or_else(|| Arc::clone(raw), Arc::new))
    }
}
