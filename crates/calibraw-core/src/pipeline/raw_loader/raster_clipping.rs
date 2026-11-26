//! `RasterClipping`: where a camera-linear raster's channels clipped.

use super::{BinnedClipping, SensorSaturation};
use rayon::prelude::*;

/// Where the channels of a camera-linear raster (a LinearRaw DNG) clipped, so
/// highlight reconstruction rebuilds them as it rebuilds a mosaic's clipped
/// photosites.
///
/// A converter that demosaics a RAW into a LinearRaw DNG (DxO PureRAW,
/// Lightroom's Enhance, the Adobe DNG Converter's linear mode) keeps a clipped
/// photosite's recorded level, so a clipped channel sits at the white level in
/// camera RGB. White balance then raises the channels that did not clip above
/// it, which tints blown areas: magenta where green clipped.
///
/// The raster's values are normalized so the white level is 1, and the photo
/// saturates at its white level ([`SensorSaturation::AT_WHITE`]).
///
/// [`SensorSaturation::AT_WHITE`]: super::SensorSaturation::AT_WHITE
#[derive(Clone, Debug, Default)]
pub enum RasterClipping {
    /// Nothing to rebuild: a rendered photo, a linear raster with no samples
    /// at its white level, or a floating-point one with values beyond it (an
    /// HDR merge), whose white level is not where light clipped.
    #[default]
    None,
    /// A channel at or above [`SensorSaturation::CLIP_DETECTION`] of the clip
    /// level clipped, as each pixel's own values show: true of the decoded
    /// raster, its crops and tiles, and of a reduced proxy none of whose
    /// source pixels clipped.
    ///
    /// [`SensorSaturation::CLIP_DETECTION`]: super::SensorSaturation::CLIP_DETECTION
    PerPixel,
    /// A reduced proxy whose box-averaged pixels mix clipped and unclipped
    /// source pixels, which pull a clipped channel below the clip level: where
    /// its source pixels clipped, in the units of [`BinnedClipping`] with each
    /// pixel's colours as its photosites.
    Averaged(BinnedClipping),
}

/// How a LinearRaw DNG stores its samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinearSampleFormat {
    /// Unsigned integers, which cannot record light beyond the white level.
    Integer,
    /// Floating point, which can: an HDR merge stores scene values above it.
    Float,
}

/// Samples of one channel at its highest level that mark it as clipped there.
/// Clipped areas pile up many at one code; a raster whose brightest pixels
/// merely reach their highest level at a few places is taken as unclipped.
const CLIPPED_SAMPLES: usize = 16;

/// How far a normalized sample may lie below its channel's highest level and
/// still count as at it: the normalization's rounding, well below the 16-bit
/// step, so only samples at the very same code pile up.
const LEVEL_TOLERANCE: f32 = 1.0 / 65536.0;

impl RasterClipping {
    /// The clipping of a decoded LinearRaw raster, `rgb` interleaved and
    /// normalized so its white level is 1.
    ///
    /// A channel clipped where [`CLIPPED_SAMPLES`] of it pile up at its
    /// highest level. A converter may clip a few codes below the white level
    /// it writes (DxO PureRAW clips a Canon file at 65526 of 65535), so the
    /// level only has to lie where reconstruction tests for clipping, from
    /// [`SensorSaturation::CLIP_DETECTION`] of the white level up. A
    /// floating-point raster clipped only when it also holds nothing above the
    /// white level: an HDR merge's values reach beyond it, and its brightest
    /// ones were recorded, not clipped.
    ///
    /// [`SensorSaturation::CLIP_DETECTION`]: super::SensorSaturation::CLIP_DETECTION
    pub fn of_linear_raw(rgb: &[f32], format: LinearSampleFormat) -> Self {
        let highest = rgb
            .par_chunks_exact(3)
            .fold(
                || [f32::NEG_INFINITY; 3],
                |highest, pixel| std::array::from_fn(|c| highest[c].max(pixel[c])),
            )
            .reduce(
                || [f32::NEG_INFINITY; 3],
                |a, b| std::array::from_fn(|c| a[c].max(b[c])),
            );
        let at_highest = rgb
            .par_chunks_exact(3)
            .fold(
                || [0usize; 3],
                |mut counts, pixel| {
                    for (c, count) in counts.iter_mut().enumerate() {
                        *count += usize::from(pixel[c] >= highest[c] - LEVEL_TOLERANCE);
                    }
                    counts
                },
            )
            .reduce(|| [0usize; 3], |a, b| std::array::from_fn(|c| a[c] + b[c]));
        let clipped = (0..3).any(|c| {
            highest[c] >= SensorSaturation::CLIP_DETECTION && at_highest[c] >= CLIPPED_SAMPLES
        });
        let within_white = match format {
            LinearSampleFormat::Integer => true,
            LinearSampleFormat::Float => {
                highest.iter().all(|level| *level <= 1.0 + LEVEL_TOLERANCE)
            }
        };
        if clipped && within_white {
            Self::PerPixel
        } else {
            Self::None
        }
    }

    /// Whether the raster holds clipped channels to rebuild.
    pub fn is_recorded(&self) -> bool {
        !matches!(self, Self::None)
    }

    /// The clipping record of a reduced proxy ([`Self::Averaged`]).
    pub fn averaged(&self) -> Option<&BinnedClipping> {
        match self {
            Self::Averaged(clipping) => Some(clipping),
            Self::None | Self::PerPixel => None,
        }
    }

    /// This clipping for a crop or padded tile of its raster: an
    /// [`Self::Averaged`] record passed through `plane`, which crops or pads
    /// interleaved RGB rows as it does the raster's pixels. The other kinds
    /// hold per pixel and stay as they are.
    pub(crate) fn map_planes(&self, mut plane: impl FnMut(&[f32]) -> Vec<f32>) -> Self {
        match self {
            Self::Averaged(clipping) => Self::Averaged(
                clipping
                    .try_map_planes(|values| Some(plane(values)))
                    .expect("mapping a plane never gives up"),
            ),
            Self::None => Self::None,
            Self::PerPixel => Self::PerPixel,
        }
    }
}
