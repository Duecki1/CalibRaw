//! Sensor views reduced to co-sited RGB before demosaicing.

use std::sync::Arc;

/// A reduced view of a sensor mosaic in which every pixel holds all three
/// colours, each filtered from the photosites of that colour around the
/// pixel's centre (`build_region_proxy`). A smaller mosaic would sample every
/// colour more coarsely than its pixels, so detail near the reduced
/// resolution would demosaic into false colour; binned pixels skip
/// demosaicing instead.
///
/// Pixels are interleaved R, G, B rows of the view's width, in normalized
/// sensor units before white balance: each photosite contributes
/// `(sample - black) / (white - black)` for its own black level and its
/// channel's white level (`raw_sensor_at` in raw_sampling.wgsl at black point
/// 0), so black is 0 and the channel's white 1. Values are not clamped:
/// noise below black stays negative. The black point, white balance,
/// highlight reconstruction and lens shading still apply on the GPU, as for
/// a mosaic.
#[derive(Clone, Debug)]
pub struct BinnedSensor {
    rgb: Arc<[f32]>,
    ai_rgb: Option<Arc<[f32]>>,
}

impl BinnedSensor {
    /// `rgb` holds three finite values per pixel of the view; `ai_rgb`, the
    /// AI-denoised Bayer mosaic binned the same way, matches it in length.
    /// The GPU pipeline checks both when it uploads them.
    pub(crate) fn new(rgb: Vec<f32>, ai_rgb: Option<Vec<f32>>) -> Self {
        debug_assert!(ai_rgb.as_ref().is_none_or(|ai| ai.len() == rgb.len()));
        Self {
            rgb: rgb.into(),
            ai_rgb: ai_rgb.map(Into::into),
        }
    }

    pub fn rgb(&self) -> &[f32] {
        &self.rgb
    }

    /// The AI-denoised mosaic binned like [`Self::rgb`], which the RAW stage
    /// blends in by the AI denoise amount, as it does a mosaic's.
    pub fn ai_rgb(&self) -> Option<&[f32]> {
        self.ai_rgb.as_deref()
    }
}
