//! The AI denoise an export needs, installed before it renders. A photo
//! exported from the Library is not open, and an open one may have its
//! result missing, so the export restores the saved result or runs RawNIND
//! itself instead of failing ([`crate::services::ai_denoise`]).

use super::*;

/// Whether `raw` must be denoised before `exposure` can render. Rendered
/// photos have no sensor data for RawNIND and export without AI denoise, as
/// they show in Develop.
pub(super) fn export_needs_ai_denoise(raw: &LoadedRaw, exposure: &ExposureParams) -> bool {
    exposure.ai_denoise_enabled
        && !raw.is_pre_demosaiced_raster()
        && raw.ai_denoised_image().is_none()
}
