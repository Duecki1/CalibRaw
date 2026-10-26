//! Colour helpers shared by the display-referred raster loaders (rendered
//! TIFF, JPEG, PNG and HEIF): decoding encoded RGB into the scene-linear
//! Rec.2020 working space, and encoding it back for library thumbnails.

use super::raw_loader::RawThumbnail;
use crate::color_math::{srgb_decode, LINEAR_DISPLAY_P3_TO_REC2020, LINEAR_SRGB_TO_REC2020};
use crate::matrix::{transform, Matrix3};
use anyhow::{anyhow, Result};
use rayon::prelude::*;

/// How the encoded RGB samples of a rendered image map to colour.
#[derive(Clone, Copy, Debug)]
pub(super) enum EncodedColorSpace<'a> {
    /// An embedded ICC matrix/shaper profile.
    Icc(&'a [u8]),
    /// Coded primaries and transfer curve, as an H.273 `nclx` box describes
    /// them. Untagged images are sRGB by convention.
    Coded {
        primaries: Primaries,
        transfer: Transfer,
    },
}

impl EncodedColorSpace<'_> {
    pub(super) const SRGB: Self = Self::Coded {
        primaries: Primaries::Srgb,
        transfer: Transfer::Srgb,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Primaries {
    Srgb,
    DisplayP3,
    Rec2020,
}

impl Primaries {
    /// Maps H.273 `colour_primaries`; unknown or unspecified codes are sRGB.
    pub(super) fn from_h273(code: u16) -> Self {
        match code {
            9 => Self::Rec2020,
            12 => Self::DisplayP3,
            _ => Self::Srgb,
        }
    }

    fn to_rec2020(self) -> Option<Matrix3> {
        match self {
            Self::Srgb => Some(LINEAR_SRGB_TO_REC2020),
            Self::DisplayP3 => Some(LINEAR_DISPLAY_P3_TO_REC2020),
            Self::Rec2020 => None,
        }
    }
}

/// Transfer curves decoded to scene-linear light where SDR diffuse white is
/// 1.0. HDR curves (PQ, HLG) keep their highlights above 1.0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Transfer {
    Srgb,
    /// BT.709, BT.601 and BT.2020 SDR camera curves, inverted as browsers do.
    Bt709,
    Gamma(f32),
    Linear,
    /// SMPTE ST 2084 (PQ), with BT.2408 reference white (203 nits) at 1.0.
    Pq,
    /// ARIB STD-B67 (HLG), with BT.2408 reference white (75% signal) at 1.0.
    Hlg,
}

/// BT.2408 HDR reference white.
const PQ_REFERENCE_WHITE_NITS: f32 = 203.0;
const HLG_REFERENCE_WHITE_SIGNAL: f32 = 0.75;

impl Transfer {
    /// Maps H.273 `transfer_characteristics`; unknown or unspecified codes
    /// are sRGB, the convention for still images.
    pub(super) fn from_h273(code: u16) -> Self {
        match code {
            1 | 6 | 14 | 15 => Self::Bt709,
            4 => Self::Gamma(2.2),
            5 => Self::Gamma(2.8),
            8 => Self::Linear,
            16 => Self::Pq,
            18 => Self::Hlg,
            _ => Self::Srgb,
        }
    }

    fn decode(self, encoded: f32) -> f32 {
        match self {
            Self::Srgb => srgb_decode(encoded),
            Self::Bt709 => {
                let value = encoded.max(0.0);
                if value < 0.081 {
                    value / 4.5
                } else {
                    ((value + 0.099) / 1.099).powf(1.0 / 0.45)
                }
            }
            Self::Gamma(gamma) => encoded.max(0.0).powf(gamma),
            Self::Linear => encoded,
            Self::Pq => pq_eotf_nits(encoded) / PQ_REFERENCE_WHITE_NITS,
            Self::Hlg => hlg_inverse_oetf(encoded) / hlg_inverse_oetf(HLG_REFERENCE_WHITE_SIGNAL),
        }
    }
}

fn pq_eotf_nits(encoded: f32) -> f32 {
    const M1: f32 = 2610.0 / 16384.0;
    const M2: f32 = 2523.0 / 4096.0 * 128.0;
    const C1: f32 = 3424.0 / 4096.0;
    const C2: f32 = 2413.0 / 4096.0 * 32.0;
    const C3: f32 = 2392.0 / 4096.0 * 32.0;
    let power = encoded.clamp(0.0, 1.0).powf(1.0 / M2);
    let ratio = (power - C1).max(0.0) / (C2 - C3 * power);
    10_000.0 * ratio.powf(1.0 / M1)
}

/// Normalised scene light in [0, 1] from an HLG signal (BT.2100).
fn hlg_inverse_oetf(encoded: f32) -> f32 {
    const A: f32 = 0.178_832_77;
    const B: f32 = 1.0 - 4.0 * A;
    const C: f32 = 0.559_910_7;
    let value = encoded.clamp(0.0, 1.0);
    if value <= 0.5 {
        value * value / 3.0
    } else {
        (((value - C) / A).exp() + B) / 12.0
    }
}

/// Decodes encoded RGB triplets in place into linear Rec.2020. An error
/// means the ICC profile was rejected before any pixel changed, so callers
/// may retry with another colour space. Check the result with
/// [`ensure_finite`].
pub(super) fn encoded_rgb_to_scene_linear_rec2020(
    rgb: &mut [f32],
    space: EncodedColorSpace<'_>,
) -> Result<()> {
    match space {
        EncodedColorSpace::Icc(profile) => {
            super::color_profile::convert_embedded_icc_rgb_to_rec2020(profile, rgb)
        }
        EncodedColorSpace::Coded {
            primaries,
            transfer,
        } => {
            let matrix = primaries.to_rec2020();
            rgb.par_chunks_exact_mut(3).for_each(|pixel| {
                let linear = [pixel[0], pixel[1], pixel[2]].map(|value| transfer.decode(value));
                pixel.copy_from_slice(&matrix.map_or(linear, |matrix| transform(matrix, linear)));
            });
            Ok(())
        }
    }
}

pub(super) fn ensure_finite(rgb: &[f32]) -> Result<()> {
    if rgb.par_iter().any(|value| !value.is_finite()) {
        return Err(anyhow!("colour conversion produced NaN or infinity"));
    }
    Ok(())
}

/// Encodes scene-linear Rec.2020 pixels as an opaque sRGB thumbnail.
pub(super) fn scene_linear_thumbnail(width: u32, height: u32, rgb: &[f32]) -> RawThumbnail {
    let rgba = rgb
        .par_chunks_exact(3)
        .flat_map_iter(|pixel| {
            let encoded = super::color_profile::display_linear_rec2020_to_srgb([
                pixel[0], pixel[1], pixel[2],
            ]);
            let [r, g, b] = encoded.map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8);
            [r, g, b, 255]
        })
        .collect();
    RawThumbnail {
        width,
        height,
        rgba,
    }
}
