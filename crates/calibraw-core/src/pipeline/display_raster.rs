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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_white_and_black_stay_neutral_in_every_primary_set() {
        for primaries in [Primaries::Srgb, Primaries::DisplayP3, Primaries::Rec2020] {
            let mut rgb = vec![1.0, 1.0, 1.0, 0.0, 0.0, 0.0];
            encoded_rgb_to_scene_linear_rec2020(
                &mut rgb,
                EncodedColorSpace::Coded {
                    primaries,
                    transfer: Transfer::Srgb,
                },
            )
            .unwrap();
            for value in &rgb[..3] {
                assert!((value - 1.0).abs() < 1e-4, "{primaries:?}: {rgb:?}");
            }
            assert!(rgb[3..].iter().all(|value| value.abs() < 1e-6));
        }
    }

    #[test]
    fn display_p3_red_is_more_saturated_than_srgb_red() {
        let mut srgb = vec![1.0, 0.0, 0.0];
        let mut p3 = srgb.clone();
        encoded_rgb_to_scene_linear_rec2020(&mut srgb, EncodedColorSpace::SRGB).unwrap();
        encoded_rgb_to_scene_linear_rec2020(
            &mut p3,
            EncodedColorSpace::Coded {
                primaries: Primaries::DisplayP3,
                transfer: Transfer::Srgb,
            },
        )
        .unwrap();
        assert!(p3[0] > srgb[0] && p3[1] < srgb[1]);
    }

    #[test]
    fn transfer_curves_follow_their_standards() {
        let close = |actual: f32, expected: f32| (actual - expected).abs() < 2e-3;
        for transfer in [
            Transfer::Srgb,
            Transfer::Bt709,
            Transfer::Gamma(2.2),
            Transfer::Linear,
        ] {
            assert!(close(transfer.decode(0.0), 0.0), "{transfer:?}");
            assert!(close(transfer.decode(1.0), 1.0), "{transfer:?}");
        }
        assert!(close(Transfer::Bt709.decode(0.081), 0.018));
        assert!(close(Transfer::Bt709.decode(0.5), 0.2596));
        // HDR reference white maps to SDR diffuse white; peaks stay above it.
        assert!(close(Transfer::Pq.decode(0.580_69), 1.0));
        assert!(close(pq_eotf_nits(1.0), 10_000.0));
        assert!(close(Transfer::Hlg.decode(0.75), 1.0));
        assert!(Transfer::Hlg.decode(1.0) > 3.5);
    }

    #[test]
    fn h273_codes_select_curves_and_primaries() {
        assert_eq!(Transfer::from_h273(13), Transfer::Srgb);
        assert_eq!(Transfer::from_h273(2), Transfer::Srgb);
        assert_eq!(Transfer::from_h273(1), Transfer::Bt709);
        assert_eq!(Transfer::from_h273(16), Transfer::Pq);
        assert_eq!(Transfer::from_h273(18), Transfer::Hlg);
        assert_eq!(Transfer::from_h273(8), Transfer::Linear);
        assert_eq!(Primaries::from_h273(1), Primaries::Srgb);
        assert_eq!(Primaries::from_h273(9), Primaries::Rec2020);
        assert_eq!(Primaries::from_h273(12), Primaries::DisplayP3);
    }

    #[test]
    fn rejected_icc_profile_leaves_pixels_untouched() {
        let mut rgb = vec![0.5, 0.25, 0.75];
        assert!(
            encoded_rgb_to_scene_linear_rec2020(&mut rgb, EncodedColorSpace::Icc(b"bad")).is_err()
        );
        assert_eq!(rgb, vec![0.5, 0.25, 0.75]);
    }

    #[test]
    fn thumbnail_encodes_scene_linear_white_and_black() {
        let thumbnail = scene_linear_thumbnail(2, 1, &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
        assert_eq!(thumbnail.rgba, vec![0, 0, 0, 255, 255, 255, 255, 255]);
    }
}
