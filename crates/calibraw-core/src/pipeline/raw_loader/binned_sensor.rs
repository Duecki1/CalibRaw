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
    clipping: Option<BinnedClipping>,
}

/// Where the photosites behind a binned view clipped, so highlight
/// reconstruction can rebuild the clipped part of a pixel that also holds
/// unclipped photosites: averaged into [`BinnedSensor::rgb`], clipped
/// photosites pull a colour below the clip level and would hide it.
///
/// A photosite is clipped at [`SensorSaturation::CLIP_DETECTION`] of the
/// photo's saturation in the normalized units of [`BinnedSensor::rgb`], the
/// level `binned_guided_rgb` (highlights.wgsl) tests at the default black
/// point and highlight clip. Both planes are interleaved R, G, B rows like
/// `rgb`, and both are means over the same filter weights, so averaging
/// pixels keeps them exact:
///
/// - `shares`: the share of a colour's filter weight on clipped photosites;
/// - `clipped_values`: the weighted sum of the clipped photosites' values
///   over the colour's whole filter weight, so their mean is
///   `clipped_values / shares`.
///
/// They are measured on the recorded mosaic. An AI-denoised mosaic keeps the
/// recorded values of clipped photosites, so they hold for `ai_rgb` too.
///
/// [`SensorSaturation::CLIP_DETECTION`]: crate::pipeline::SensorSaturation::CLIP_DETECTION
#[derive(Clone, Debug)]
pub struct BinnedClipping {
    shares: Arc<[f32]>,
    clipped_values: Arc<[f32]>,
}

impl BinnedClipping {
    pub(crate) fn new(shares: Vec<f32>, clipped_values: Vec<f32>) -> Self {
        debug_assert_eq!(shares.len(), clipped_values.len());
        Self {
            shares: shares.into(),
            clipped_values: clipped_values.into(),
        }
    }

    pub fn shares(&self) -> &[f32] {
        &self.shares
    }

    pub fn clipped_values(&self) -> &[f32] {
        &self.clipped_values
    }
}

impl BinnedSensor {
    /// `rgb` holds three finite values per pixel of the view; `ai_rgb`, the
    /// AI-denoised Bayer mosaic binned the same way, and both planes of
    /// `clipping` match it in length. The GPU pipeline checks them when it
    /// uploads them.
    pub(crate) fn new(
        rgb: Vec<f32>,
        ai_rgb: Option<Vec<f32>>,
        clipping: Option<BinnedClipping>,
    ) -> Self {
        debug_assert!(ai_rgb.as_ref().is_none_or(|ai| ai.len() == rgb.len()));
        debug_assert!(clipping
            .as_ref()
            .is_none_or(|clipping| clipping.shares.len() == rgb.len()));
        Self {
            rgb: rgb.into(),
            ai_rgb: ai_rgb.map(Into::into),
            clipping,
        }
    }

    /// Every plane of the view passed through `plane`, which crops, pads or
    /// averages interleaved RGB rows; `None` when it returns `None`, as a
    /// cancelled average does.
    pub(crate) fn try_map_planes(
        &self,
        mut plane: impl FnMut(&[f32]) -> Option<Vec<f32>>,
    ) -> Option<Self> {
        let rgb = plane(&self.rgb)?;
        let ai_rgb = match &self.ai_rgb {
            Some(ai) => Some(plane(ai)?),
            None => None,
        };
        let clipping = match &self.clipping {
            Some(clipping) => Some(BinnedClipping::new(
                plane(&clipping.shares)?,
                plane(&clipping.clipped_values)?,
            )),
            None => None,
        };
        Some(Self::new(rgb, ai_rgb, clipping))
    }

    pub fn rgb(&self) -> &[f32] {
        &self.rgb
    }

    /// The AI-denoised mosaic binned like [`Self::rgb`], which the RAW stage
    /// blends in by the AI denoise amount, as it does a mosaic's.
    pub fn ai_rgb(&self) -> Option<&[f32]> {
        self.ai_rgb.as_deref()
    }

    /// Where the view's photosites clipped; `None` when none did.
    pub fn clipping(&self) -> Option<&BinnedClipping> {
        self.clipping.as_ref()
    }
}
