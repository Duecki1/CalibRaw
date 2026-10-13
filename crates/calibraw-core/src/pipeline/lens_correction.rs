//! Lens correction requests: a Lensfun profile, or hand-set corrections for
//! lenses without one.
//!
//! Both produce the same result: a [`LoadedRaw`] whose mosaic (or raster) is
//! shading-corrected and whose `lens_geometry` maps corrected to source
//! positions, so preview, masks and export treat them alike.

use super::lensfun::{apply_lensfun_correction, LensfunLens};
use super::{LensGeometryMap, LoadedRaw};
use anyhow::{anyhow, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// What a photo is corrected with.
#[derive(Clone, Debug, PartialEq)]
pub enum LensCorrection {
    Profile(LensfunLens),
    Manual(ManualLensCorrection),
}

impl LensCorrection {
    pub fn label(&self) -> String {
        match self {
            Self::Profile(lens) => lens.label(),
            Self::Manual(_) => "manual lens correction".to_owned(),
        }
    }
}

pub fn apply_lens_correction(raw: &LoadedRaw, correction: &LensCorrection) -> Result<LoadedRaw> {
    match correction {
        LensCorrection::Profile(lens) => apply_lensfun_correction(raw, lens),
        LensCorrection::Manual(manual) => apply_manual_lens_correction(raw, *manual),
    }
}

/// Hand-set radial corrections, centred on the image. Both amounts run from
/// -100 to 100 and 0 leaves the photo unchanged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ManualLensCorrection {
    /// Positive straightens barrel distortion, negative pincushion.
    pub distortion: f32,
    /// Positive brightens the corners, negative darkens them.
    pub vignetting: f32,
}

impl ManualLensCorrection {
    pub const RANGE: std::ops::RangeInclusive<f32> = -100.0..=100.0;

    pub fn is_identity(self) -> bool {
        self.distortion == 0.0 && self.vignetting == 0.0
    }
}

/// Radial coefficient at full distortion strength: the source radius changes
/// by this fraction at the corners (radius 1 is the half diagonal).
const MAX_DISTORTION_COEFFICIENT: f32 = 0.15;
/// Corner exposure change, in stops, at full vignetting strength.
const MAX_VIGNETTING_STOPS: f32 = 2.0;
/// Corrected-image pixels between distortion map samples, as for Lensfun maps.
const GEOMETRY_GRID_STEP: u32 = 32;
/// Points sampled along each image edge when fitting the auto-scale.
const EDGE_SAMPLES: u32 = 64;

fn apply_manual_lens_correction(
    raw: &LoadedRaw,
    manual: ManualLensCorrection,
) -> Result<LoadedRaw> {
    if !manual.distortion.is_finite() || !manual.vignetting.is_finite() {
        return Err(anyhow!("manual lens correction amounts must be finite"));
    }
    let frame = RadialFrame::of(raw.width, raw.height);
    let distortion = manual.distortion.clamp(-100.0, 100.0) / 100.0;
    let vignetting = manual.vignetting.clamp(-100.0, 100.0) / 100.0;
    let lens_geometry = if distortion == 0.0 {
        None
    } else {
        Some(Arc::new(distortion_map(frame, raw, distortion)?))
    };
    let mut corrected = if vignetting == 0.0 {
        raw.clone()
    } else {
        apply_vignetting(raw, frame, vignetting * MAX_VIGNETTING_STOPS)?
    };
    corrected.lens_geometry = lens_geometry;
    Ok(corrected)
}

/// Pixel coordinates normalized so the image centre is 0 and its corners are
/// at radius 1.
#[derive(Clone, Copy)]
struct RadialFrame {
    center: [f32; 2],
    half_diagonal: f32,
}

impl RadialFrame {
    fn of(width: u32, height: u32) -> Self {
        let center = [
            width.saturating_sub(1) as f32 * 0.5,
            height.saturating_sub(1) as f32 * 0.5,
        ];
        Self {
            center,
            half_diagonal: center[0].hypot(center[1]).max(1.0),
        }
    }

    fn normalize(self, x: f32, y: f32) -> [f32; 2] {
        [
            (x - self.center[0]) / self.half_diagonal,
            (y - self.center[1]) / self.half_diagonal,
        ]
    }

    fn pixel(self, [u, v]: [f32; 2]) -> [f32; 2] {
        [
            self.center[0] + u * self.half_diagonal,
            self.center[1] + v * self.half_diagonal,
        ]
    }
}

/// The source position of normalized corrected position `point`: scaled by
/// `zoom`, then moved radially by `coefficient * r²`.
fn distort(point: [f32; 2], coefficient: f32, zoom: f32) -> [f32; 2] {
    let scaled = [point[0] * zoom, point[1] * zoom];
    let factor = 1.0 + coefficient * (scaled[0] * scaled[0] + scaled[1] * scaled[1]);
    [scaled[0] * factor, scaled[1] * factor]
}

/// The largest zoom whose source positions stay inside the image along every
/// edge, so the corrected frame has no empty borders. The mapping is radially
/// monotonic for the coefficients and zooms used (`1 + 3kr² > 0` up to
/// `r = 1.4`), so the source of the frame lies inside the source of its edges
/// and a bisection on edge samples finds the bound.
fn auto_scale(frame: RadialFrame, coefficient: f32) -> f32 {
    let limits = frame.normalize(0.0, 0.0).map(f32::abs);
    let fits = |zoom: f32| {
        (0..=EDGE_SAMPLES).all(|index| {
            let t = index as f32 / EDGE_SAMPLES as f32 * 2.0 - 1.0;
            [[t * limits[0], limits[1]], [limits[0], t * limits[1]]]
                .into_iter()
                .all(|point| {
                    let [u, v] = distort(point, coefficient, zoom);
                    u.abs() <= limits[0] * (1.0 + 1e-5) && v.abs() <= limits[1] * (1.0 + 1e-5)
                })
        })
    };
    let (mut low, mut high) = (0.5f32, 1.4f32);
    for _ in 0..24 {
        let middle = 0.5 * (low + high);
        if fits(middle) {
            low = middle;
        } else {
            high = middle;
        }
    }
    low
}

fn distortion_map(frame: RadialFrame, raw: &LoadedRaw, distortion: f32) -> Result<LensGeometryMap> {
    // Straightening barrel distortion samples the source closer to the centre.
    let coefficient = -distortion * MAX_DISTORTION_COEFFICIENT;
    let zoom = auto_scale(frame, coefficient);
    let grid_width = (raw.width.saturating_sub(1).div_ceil(GEOMETRY_GRID_STEP) + 1).max(2);
    let grid_height = (raw.height.saturating_sub(1).div_ceil(GEOMETRY_GRID_STEP) + 1).max(2);
    let grid_coordinate = |index: u32, cells: u32, extent: u32| {
        index as f32 * extent.saturating_sub(1) as f32 / (cells - 1) as f32
    };
    let coordinates = (0..grid_height)
        .flat_map(|grid_y| {
            let y = grid_coordinate(grid_y, grid_height, raw.height);
            (0..grid_width).map(move |grid_x| {
                let x = grid_coordinate(grid_x, grid_width, raw.width);
                frame.pixel(distort(frame.normalize(x, y), coefficient, zoom))
            })
        })
        .collect();
    LensGeometryMap::new(raw.width, raw.height, grid_width, grid_height, coordinates)
        .ok_or_else(|| anyhow!("manual distortion produced an invalid map"))
}

/// Multiplies the signal above black by `2^(stops * r²)`.
fn apply_vignetting(raw: &LoadedRaw, frame: RadialFrame, stops: f32) -> Result<LoadedRaw> {
    let width = (raw.width as usize).max(1);
    let gain = |x: usize, y: usize| {
        let [u, v] = frame.normalize(x as f32, y as f32);
        (stops * (u * u + v * v)).exp2()
    };

    if raw.is_pre_demosaiced_raster() {
        let source = raw
            .scene_linear_raster()
            .ok_or_else(|| anyhow!("rendered photo is missing its scene-linear raster"))?;
        let mut shaded = source.to_vec();
        shaded
            .par_chunks_mut(width * 3)
            .enumerate()
            .for_each(|(y, row)| {
                for (x, pixel) in row.chunks_exact_mut(3).enumerate() {
                    let gain = gain(x, y);
                    pixel.iter_mut().for_each(|value| *value *= gain);
                }
            });
        let mut corrected = raw.clone();
        corrected.scene_linear_raster = Some(Arc::from(shaded));
        corrected.ai_denoised = Default::default();
        corrected.opposed_chroma_cache = Default::default();
        return Ok(corrected);
    }

    let mut raw_pixels = vec![0u16; raw.raw_pixels.len()];
    raw_pixels
        .par_chunks_mut(width)
        .zip(raw.raw_pixels.par_chunks(width))
        .enumerate()
        .for_each(|(y, (output, input))| {
            for (x, (output, &sample)) in output.iter_mut().zip(input).enumerate() {
                let black = raw.black_levels_per_pixel[y * width + x];
                let sample = f32::from(sample);
                let corrected = black + (sample - black).max(0.0) * gain(x, y);
                *output = corrected.round().clamp(0.0, f32::from(u16::MAX)) as u16;
            }
        });
    // The corrected mosaic replaces the source as the full-sensor reference,
    // so it starts its own opposed-chroma cache and source identity.
    Ok(LoadedRaw {
        opposed_chroma_cache: Default::default(),
        opposed_chroma_source_identity: Default::default(),
        opposed_chroma_reference_source: true,
        ..raw.derive_with(
            raw.width,
            raw.height,
            raw_pixels,
            raw.color_indices.clone(),
            raw.black_levels_per_pixel.clone(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{CfaKind, CompactPixelMap};

    fn mosaic(width: u32, height: u32, value: u16) -> LoadedRaw {
        let raster = LoadedRaw::from_scene_linear_rec2020(
            width,
            height,
            vec![0.5; (width * height * 3) as usize],
        )
        .unwrap();
        LoadedRaw {
            cfa_kind: CfaKind::Bayer,
            ..raster.derive_with(
                width,
                height,
                vec![value; (width * height) as usize],
                CompactPixelMap::repeating(width, height, 2, 2, vec![0, 1, 1, 2]),
                CompactPixelMap::repeating(width, height, 1, 1, vec![100.0]),
            )
        }
    }

    fn manual(distortion: f32, vignetting: f32) -> LensCorrection {
        LensCorrection::Manual(ManualLensCorrection {
            distortion,
            vignetting,
        })
    }

    #[test]
    fn vignetting_brightens_corners_above_black_and_keeps_the_centre() {
        let raw = mosaic(65, 33, 1100);
        let corrected = apply_lens_correction(&raw, &manual(0.0, 50.0)).unwrap();
        assert!(corrected.lens_geometry.is_none());
        let at = |x: usize, y: usize| corrected.raw_pixels[y * 65 + x];
        assert_eq!(at(32, 16), 1100);
        // One stop at the corner doubles the 1000 above black.
        assert_eq!(at(0, 0), 2100);
        assert!(at(0, 16) > 1100 && at(0, 16) < 2100);

        let darkened = apply_lens_correction(&raw, &manual(0.0, -50.0)).unwrap();
        assert_eq!(darkened.raw_pixels[0], 600);
    }

    #[test]
    fn vignetting_scales_rendered_rasters() {
        let raster = LoadedRaw::from_scene_linear_rec2020(9, 9, vec![0.25; 9 * 9 * 3]).unwrap();
        let corrected = apply_lens_correction(&raster, &manual(0.0, 50.0)).unwrap();
        let rgb = corrected.scene_linear_raster().unwrap();
        assert!(rgb[..3].iter().all(|value| (value - 0.5).abs() < 1e-5));
        assert_eq!(&rgb[(4 * 9 + 4) * 3..][..3], &[0.25; 3]);
    }

    #[test]
    fn distortion_fills_the_frame_and_keeps_the_centre() {
        let raw = mosaic(301, 201, 1000);
        for distortion in [-100.0, -30.0, 30.0, 100.0] {
            let corrected = apply_lens_correction(&raw, &manual(distortion, 0.0)).unwrap();
            assert_eq!(corrected.raw_pixels, raw.raw_pixels);
            let map = corrected.lens_geometry.as_deref().unwrap();
            let source = |x: f32, y: f32| map.source_position_for_raster(x, y, 301, 201);
            let center = source(150.0, 100.0);
            assert!((center[0] - 150.0).abs() < 1e-3 && (center[1] - 100.0).abs() < 1e-3);
            let edges = (0..=20).flat_map(|index| {
                let t = index as f32 / 20.0;
                [
                    (300.0 * t, 0.0),
                    (300.0 * t, 200.0),
                    (0.0, 200.0 * t),
                    (300.0, 200.0 * t),
                ]
            });
            let mut touches_edge = false;
            for (x, y) in edges {
                let [sx, sy] = source(x, y);
                assert!(
                    (-0.01..=300.01).contains(&sx) && (-0.01..=200.01).contains(&sy),
                    "{distortion}: ({x}, {y}) samples ({sx}, {sy})"
                );
                touches_edge |= !(0.5..=299.5).contains(&sx) || !(0.5..=199.5).contains(&sy);
            }
            assert!(
                touches_edge,
                "{distortion}: the auto-scale wastes the frame"
            );
        }
    }

    #[test]
    fn distortion_direction_follows_the_amount() {
        let raw = mosaic(301, 201, 1000);
        let corner_source = |distortion| {
            let corrected = apply_lens_correction(&raw, &manual(distortion, 0.0)).unwrap();
            let map = corrected.lens_geometry.unwrap();
            let [x, y] = map.source_position_for_raster(0.0, 0.0, 301, 201);
            (x - 150.0).hypot(y - 100.0)
        };
        let edge_source = |distortion| {
            let corrected = apply_lens_correction(&raw, &manual(distortion, 0.0)).unwrap();
            let map = corrected.lens_geometry.unwrap();
            150.0 - map.source_position_for_raster(0.0, 100.0, 301, 201)[0]
        };
        // Straightening barrel distortion pulls the corners in relative to the
        // edges; pincushion pushes them out.
        let ratio = |distortion| corner_source(distortion) / edge_source(distortion);
        let neutral = 150.0f32.hypot(100.0) / 150.0;
        assert!(ratio(60.0) < neutral && ratio(-60.0) > neutral);
    }

    #[test]
    fn non_finite_amounts_are_refused() {
        let raw = mosaic(8, 8, 1000);
        assert!(apply_lens_correction(&raw, &manual(f32::NAN, 0.0)).is_err());
        assert!(ManualLensCorrection::default().is_identity());
    }
}
