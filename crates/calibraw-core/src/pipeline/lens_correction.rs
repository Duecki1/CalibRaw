//! Lens correction requests: a Lensfun profile, or hand-set corrections for
//! lenses without one.
//!
//! Both produce the same result: a [`LoadedRaw`] whose mosaic (or raster) is
//! shading-corrected and whose `lens_geometry` maps corrected to source
//! positions, so preview, masks and export treat them alike.

use super::lensfun::{apply_lensfun_correction, LensfunLens};
use super::{LensGeometryMap, LensShadingMap, LoadedRaw};
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
    let model = ManualLensModel::new(manual, raw.width, raw.height);
    let lens_geometry = if model.coefficient == 0.0 {
        None
    } else {
        Some(Arc::new(distortion_map(&model, raw)?))
    };
    let mut corrected = if model.vignetting_stops == 0.0 {
        raw.clone()
    } else {
        apply_vignetting(raw, &model)?
    };
    corrected.lens_geometry = lens_geometry;
    Ok(corrected)
}

/// A [`ManualLensCorrection`] evaluated for one image size, in that image's
/// pixel coordinates. Applying the correction and previewing it share it, so
/// the preview shows what is applied.
#[derive(Clone, Copy, Debug)]
pub struct ManualLensModel {
    /// The image centre in pixels.
    center: [f32; 2],
    /// The centre-to-corner distance in pixels: radius 1 of the model.
    half_diagonal: f32,
    /// Radial distortion coefficient `k`: corrected radius `r` samples the
    /// source at `zoom·r·(1 + k·(zoom·r)²)`.
    coefficient: f32,
    /// Scale that keeps the corrected frame free of empty borders.
    zoom: f32,
    /// Exposure change at the corners; it falls off with `r²`.
    vignetting_stops: f32,
}

impl ManualLensModel {
    pub fn new(correction: ManualLensCorrection, width: u32, height: u32) -> Self {
        let center = [
            width.saturating_sub(1) as f32 * 0.5,
            height.saturating_sub(1) as f32 * 0.5,
        ];
        let amount = |value: f32| {
            if value.is_finite() {
                value.clamp(-100.0, 100.0) / 100.0
            } else {
                0.0
            }
        };
        // Straightening barrel distortion samples the source closer to the centre.
        let coefficient = -amount(correction.distortion) * MAX_DISTORTION_COEFFICIENT;
        let mut model = Self {
            center,
            half_diagonal: center[0].hypot(center[1]).max(1.0),
            coefficient,
            zoom: 1.0,
            vignetting_stops: amount(correction.vignetting) * MAX_VIGNETTING_STOPS,
        };
        if coefficient != 0.0 {
            model.zoom = model.auto_scale();
        }
        model
    }

    /// The source position that corrected pixel position `point` shows.
    pub fn source_position(&self, point: [f32; 2]) -> [f32; 2] {
        let [u, v] = self.normalize(point);
        let [u, v] = self.distort([u, v], self.zoom);
        [
            self.center[0] + u * self.half_diagonal,
            self.center[1] + v * self.half_diagonal,
        ]
    }

    /// The exposure change, in stops, applied to the source pixel at `point`.
    pub fn vignetting_stops(&self, point: [f32; 2]) -> f32 {
        let [u, v] = self.normalize(point);
        self.vignetting_stops * (u * u + v * v)
    }

    fn normalize(&self, [x, y]: [f32; 2]) -> [f32; 2] {
        [
            (x - self.center[0]) / self.half_diagonal,
            (y - self.center[1]) / self.half_diagonal,
        ]
    }

    /// The normalized source position of normalized corrected position
    /// `point` at `zoom`.
    fn distort(&self, point: [f32; 2], zoom: f32) -> [f32; 2] {
        let scaled = [point[0] * zoom, point[1] * zoom];
        let factor = 1.0 + self.coefficient * (scaled[0] * scaled[0] + scaled[1] * scaled[1]);
        [scaled[0] * factor, scaled[1] * factor]
    }

    /// The largest zoom whose source positions stay inside the image along
    /// every edge, so the corrected frame has no empty borders. The mapping is
    /// radially monotonic for the coefficients and zooms used (`1 + 3kr² > 0`
    /// up to `r = 1.4`), so the source of the frame lies inside the source of
    /// its edges and a bisection on edge samples finds the bound.
    fn auto_scale(&self) -> f32 {
        let limits = [
            self.center[0] / self.half_diagonal,
            self.center[1] / self.half_diagonal,
        ];
        let fits = |zoom: f32| {
            (0..=EDGE_SAMPLES).all(|index| {
                let t = index as f32 / EDGE_SAMPLES as f32 * 2.0 - 1.0;
                [[t * limits[0], limits[1]], [limits[0], t * limits[1]]]
                    .into_iter()
                    .all(|point| {
                        let [u, v] = self.distort(point, zoom);
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
}

fn distortion_map(model: &ManualLensModel, raw: &LoadedRaw) -> Result<LensGeometryMap> {
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
                model.source_position([x, y])
            })
        })
        .collect();
    LensGeometryMap::new(raw.width, raw.height, grid_width, grid_height, coordinates)
        .ok_or_else(|| anyhow!("manual distortion produced an invalid map"))
}

/// Applies the model's vignetting gain: as a [`LensShadingMap`] the GPU
/// applies after highlight reconstruction for sensor mosaics, or to the
/// scene-linear raster of rendered photos.
fn apply_vignetting(raw: &LoadedRaw, model: &ManualLensModel) -> Result<LoadedRaw> {
    let gain = |[x, y]: [f32; 2]| model.vignetting_stops([x, y]).exp2();

    if raw.is_pre_demosaiced_raster() {
        let width = (raw.width as usize).max(1);
        let source = raw
            .scene_linear_raster()
            .ok_or_else(|| anyhow!("rendered photo is missing its scene-linear raster"))?;
        let mut shaded = source.to_vec();
        shaded
            .par_chunks_mut(width * 3)
            .enumerate()
            .for_each(|(y, row)| {
                for (x, pixel) in row.chunks_exact_mut(3).enumerate() {
                    let gain = gain([x as f32, y as f32]);
                    pixel.iter_mut().for_each(|value| *value *= gain);
                }
            });
        let mut corrected = raw.clone();
        corrected.scene_linear_raster = Some(Arc::from(shaded));
        corrected.ai_denoised = Default::default();
        corrected.opposed_chroma_cache = Default::default();
        return Ok(corrected);
    }

    let extent = [
        raw.width.saturating_sub(1) as f32,
        raw.height.saturating_sub(1) as f32,
    ];
    let shading = LensShadingMap::from_fn(|[u, v]| [gain([u * extent[0], v * extent[1]]); 4])
        .ok_or_else(|| anyhow!("manual vignetting produced invalid gains"))?;
    Ok(LoadedRaw {
        lens_shading: Some(shading.after(raw.lens_shading.as_ref())),
        ..raw.clone()
    })
}
