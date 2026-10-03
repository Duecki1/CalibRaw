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
    /// The sRGB transfer curve with the given primaries. Untagged images are
    /// sRGB by convention.
    SrgbCurve(Primaries),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Primaries {
    Srgb,
    DisplayP3,
    Rec2020,
}

impl Primaries {
    fn to_rec2020(self) -> Option<Matrix3> {
        match self {
            Self::Srgb => Some(LINEAR_SRGB_TO_REC2020),
            Self::DisplayP3 => Some(LINEAR_DISPLAY_P3_TO_REC2020),
            Self::Rec2020 => None,
        }
    }
}

impl EncodedColorSpace<'_> {
    pub(super) const SRGB: Self = Self::SrgbCurve(Primaries::Srgb);
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
        EncodedColorSpace::SrgbCurve(primaries) => {
            let matrix = primaries.to_rec2020();
            rgb.par_chunks_exact_mut(3).for_each(|pixel| {
                let linear = [pixel[0], pixel[1], pixel[2]].map(srgb_decode);
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
            encoded_rgb_to_scene_linear_rec2020(&mut rgb, EncodedColorSpace::SrgbCurve(primaries))
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
            EncodedColorSpace::SrgbCurve(Primaries::DisplayP3),
        )
        .unwrap();
        assert!(p3[0] > srgb[0] && p3[1] < srgb[1]);
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
