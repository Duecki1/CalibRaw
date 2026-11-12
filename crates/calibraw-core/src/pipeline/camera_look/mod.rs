//! Camera match: a look fitted so the canonical render of a RAW matches the
//! camera's embedded JPEG preview.
//!
//! The look has four parts, applied in this order:
//!
//! 1. a scene exposure offset in EV, added to the camera profile's default
//!    exposure before tone mapping;
//! 2. a monotone tone curve on the sRGB-encoded display values the tone
//!    mapper produces, the same curve on each channel;
//! 3. a 3×3 matrix on the curve's result in linear sRGB;
//! 4. an optional smooth residual on a 9³ lattice over the sRGB-encoded input,
//!    which corrects what the curve and matrix cannot. Lattice points the fit
//!    saw too few colours near carry no residual, so unseen colours keep the
//!    curve and matrix alone.
//!
//! Parts 2–4 are display-referred and render as a 3D LUT through the edit's
//! look-LUT stage ([`CameraLook::bake`], `look_lut.wgsl`), which takes and
//! returns sRGB-encoded values in `[0, 1]`.

mod bake;
mod delta_e;
mod fit;
mod image;
mod solve;

use bake::BakeCache;
pub use bake::{BakeCheck, BAKED_LUT_SIZE};
pub use delta_e::{ciede2000, linear_srgb_to_lab};
pub use fit::{
    fit_camera_look, CameraLookFit, CameraLookFitError, FitAlignment, FitStage, FitVerdict,
    StageScore, MIN_PREVIEW_EDGE,
};

use crate::color_math::{srgb_decode_signed, srgb_encode_signed};
use crate::matrix::{self, Matrix3};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

/// Knots of the tone curve, evenly spaced over the encoded input `[0, 1]`.
pub const LOOK_CURVE_KNOTS: usize = 17;
/// Lattice points per axis of the residual grid.
pub const RESIDUAL_GRID_SIZE: usize = 9;
const RESIDUAL_GRID_POINTS: usize = RESIDUAL_GRID_SIZE * RESIDUAL_GRID_SIZE * RESIDUAL_GRID_SIZE;
/// Largest exposure offset a look may carry, in EV.
pub const MAX_LOOK_EXPOSURE_EV: f32 = 3.0;
/// Largest residual a lattice point may carry, in sRGB-encoded units.
pub const MAX_RESIDUAL: f32 = 0.25;
/// Residuals are stored as signed 16-bit steps of this size, which spans
/// ±0.5, beyond [`MAX_RESIDUAL`].
const RESIDUAL_STEP: f32 = 1.0 / 65_536.0;

/// The camera's look as fitted for one photo, with the amount the edit
/// applies it at.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "CameraLookFile", into = "CameraLookFile")]
pub struct CameraLook {
    exposure_ev: f32,
    curve: LookCurve,
    /// Row-major, linear sRGB in and out.
    matrix: Matrix3,
    residual: Option<ResidualGrid>,
    amount: f32,
    /// The tables baked from this fit, shared by its clones (edit history,
    /// export and thumbnail jobs). Not part of the look's value.
    bakes: Arc<BakeCache>,
}

impl PartialEq for CameraLook {
    fn eq(&self, other: &Self) -> bool {
        self.exposure_ev == other.exposure_ev
            && self.curve == other.curve
            && self.matrix == other.matrix
            && self.residual == other.residual
            && self.amount == other.amount
    }
}

impl CameraLook {
    pub const DEFAULT_AMOUNT: f32 = 1.0;

    pub(crate) fn new(
        exposure_ev: f32,
        curve: LookCurve,
        matrix: Matrix3,
        residual: Option<ResidualGrid>,
    ) -> Self {
        Self {
            exposure_ev: exposure_ev.clamp(-MAX_LOOK_EXPOSURE_EV, MAX_LOOK_EXPOSURE_EV),
            curve,
            matrix,
            residual,
            amount: Self::DEFAULT_AMOUNT,
            bakes: Arc::default(),
        }
    }

    /// The scene exposure offset in EV at full amount.
    pub fn exposure_ev(&self) -> f32 {
        self.exposure_ev
    }

    /// The exposure offset the edit applies: the fitted one scaled by the
    /// amount.
    pub fn applied_exposure_ev(&self) -> f32 {
        self.exposure_ev * self.amount
    }

    /// How much of the look is applied, 0–1.
    pub fn amount(&self) -> f32 {
        self.amount
    }

    pub fn set_amount(&mut self, amount: f32) {
        self.amount = if amount.is_finite() {
            amount.clamp(0.0, 1.0)
        } else {
            Self::DEFAULT_AMOUNT
        };
    }

    /// Whether the residual grid survived the fit's guard.
    pub fn has_residual(&self) -> bool {
        self.residual.is_some()
    }

    /// Maps an sRGB-encoded display colour, as the tone mapper produced it
    /// with this look's exposure offset, through the curve, matrix and
    /// residual at full amount. The result is clamped to `[0, 1]`.
    pub fn map_display(&self, rgb: [f32; 3]) -> [f32; 3] {
        self.map_display_unclamped(rgb)
            .map(|value| value.clamp(0.0, 1.0))
    }

    /// [`Self::map_display`] before the clamp to `[0, 1]`.
    fn map_display_unclamped(&self, rgb: [f32; 3]) -> [f32; 3] {
        let curved = rgb.map(|value| self.curve.evaluate(value));
        let linear = curved.map(srgb_decode_signed);
        let mixed = matrix::transform(self.matrix, linear);
        let mut encoded = mixed.map(srgb_encode_signed);
        if let Some(residual) = &self.residual {
            let offset = residual.sample(rgb);
            for channel in 0..3 {
                encoded[channel] += offset[channel];
            }
        }
        encoded
    }
}

/// A monotone non-decreasing curve on `[0, 1]`, linear between
/// [`LOOK_CURVE_KNOTS`] evenly spaced knots.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LookCurve {
    knots: [f32; LOOK_CURVE_KNOTS],
}

impl LookCurve {
    pub(crate) fn identity() -> Self {
        Self {
            knots: std::array::from_fn(|index| index as f32 / (LOOK_CURVE_KNOTS - 1) as f32),
        }
    }

    /// `knots` made non-decreasing; `None` if any is not finite.
    pub(crate) fn from_knots(knots: [f32; LOOK_CURVE_KNOTS]) -> Option<Self> {
        if knots.iter().any(|knot| !knot.is_finite()) {
            return None;
        }
        let mut knots = knots;
        for index in 1..LOOK_CURVE_KNOTS {
            knots[index] = knots[index].max(knots[index - 1]);
        }
        Some(Self { knots })
    }

    pub(crate) fn knots(&self) -> &[f32; LOOK_CURVE_KNOTS] {
        &self.knots
    }

    /// The curve at `input`; inputs outside `[0, 1]` continue the end
    /// segments' slopes.
    pub(crate) fn evaluate(&self, input: f32) -> f32 {
        let last = (LOOK_CURVE_KNOTS - 1) as f32;
        let position = input * last;
        let cell = position.floor().clamp(0.0, last - 1.0);
        let index = cell as usize;
        let fraction = position - cell;
        self.knots[index] + fraction * (self.knots[index + 1] - self.knots[index])
    }
}

/// Residuals in sRGB-encoded units on a [`RESIDUAL_GRID_SIZE`]³ lattice over
/// the encoded input cube, red index fastest, read with trilinear
/// interpolation.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResidualGrid {
    values: Box<[[f32; 3]]>,
}

impl ResidualGrid {
    /// `values` clamped to [`MAX_RESIDUAL`]; `None` unless it holds
    /// [`RESIDUAL_GRID_SIZE`]³ finite entries.
    pub(crate) fn new(values: Vec<[f32; 3]>) -> Option<Self> {
        if values.len() != RESIDUAL_GRID_POINTS || values.iter().flatten().any(|v| !v.is_finite()) {
            return None;
        }
        let values = values
            .into_iter()
            .map(|entry| entry.map(|value| value.clamp(-MAX_RESIDUAL, MAX_RESIDUAL)))
            .collect();
        Some(Self { values })
    }

    pub(crate) fn sample(&self, rgb: [f32; 3]) -> [f32; 3] {
        let (corners, weights) = trilinear_corners(rgb);
        let mut sum = [0.0_f32; 3];
        for (corner, weight) in corners.iter().zip(weights) {
            let value = self.values[*corner];
            for channel in 0..3 {
                sum[channel] += weight * value[channel];
            }
        }
        sum
    }

    fn encode(&self) -> String {
        let mut bytes = Vec::with_capacity(self.values.len() * 6);
        for value in self.values.iter().flatten() {
            let step = (value / RESIDUAL_STEP).round().clamp(-32_767.0, 32_767.0) as i16;
            bytes.extend_from_slice(&step.to_le_bytes());
        }
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn decode(text: &str) -> Result<Self, CameraLookFileError> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(text)
            .map_err(|_| CameraLookFileError::Residual)?;
        if bytes.len() != RESIDUAL_GRID_POINTS * 6 {
            return Err(CameraLookFileError::Residual);
        }
        let values = bytes
            .chunks_exact(6)
            .map(|entry| {
                std::array::from_fn(|channel| {
                    f32::from(i16::from_le_bytes([
                        entry[channel * 2],
                        entry[channel * 2 + 1],
                    ])) * RESIDUAL_STEP
                })
            })
            .collect();
        Self::new(values).ok_or(CameraLookFileError::Residual)
    }
}

/// The eight lattice points around `rgb` (clamped to `[0, 1]`) on the
/// residual grid and their trilinear weights.
pub(crate) fn trilinear_corners(rgb: [f32; 3]) -> ([usize; 8], [f32; 8]) {
    let last = (RESIDUAL_GRID_SIZE - 1) as f32;
    let mut base = [0_usize; 3];
    let mut fraction = [0.0_f32; 3];
    for channel in 0..3 {
        let position = rgb[channel].clamp(0.0, 1.0) * last;
        let cell = position.floor().min(last - 1.0);
        base[channel] = cell as usize;
        fraction[channel] = position - cell;
    }
    let mut corners = [0_usize; 8];
    let mut weights = [0.0_f32; 8];
    for corner in 0..8 {
        let offset = [corner & 1, (corner >> 1) & 1, (corner >> 2) & 1];
        let mut weight = 1.0;
        for channel in 0..3 {
            weight *= if offset[channel] == 1 {
                fraction[channel]
            } else {
                1.0 - fraction[channel]
            };
        }
        corners[corner] = (base[0] + offset[0])
            + RESIDUAL_GRID_SIZE
                * ((base[1] + offset[1]) + RESIDUAL_GRID_SIZE * (base[2] + offset[2]));
        weights[corner] = weight;
    }
    (corners, weights)
}

/// The sidecar layout of [`CameraLook`].
#[derive(Serialize, Deserialize)]
struct CameraLookFile {
    exposure_ev: f32,
    curve: Vec<f32>,
    matrix: Matrix3,
    /// Base64 of little-endian `i16` steps of `RESIDUAL_STEP`, three per
    /// lattice point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    residual: Option<String>,
    amount: f32,
}

#[derive(Debug)]
enum CameraLookFileError {
    Curve,
    Matrix,
    Residual,
    Value,
}

impl fmt::Display for CameraLookFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Curve => "the camera look's curve is not valid",
            Self::Matrix => "the camera look's matrix is not valid",
            Self::Residual => "the camera look's residual grid is not valid",
            Self::Value => "the camera look holds a non-finite value",
        })
    }
}

impl TryFrom<CameraLookFile> for CameraLook {
    type Error = CameraLookFileError;

    fn try_from(file: CameraLookFile) -> Result<Self, Self::Error> {
        if !file.exposure_ev.is_finite() || !file.amount.is_finite() {
            return Err(CameraLookFileError::Value);
        }
        let knots: [f32; LOOK_CURVE_KNOTS] = file
            .curve
            .try_into()
            .map_err(|_| CameraLookFileError::Curve)?;
        let curve = LookCurve::from_knots(knots).ok_or(CameraLookFileError::Curve)?;
        if file.matrix.iter().flatten().any(|value| !value.is_finite())
            || matrix::invert(file.matrix).is_none()
        {
            return Err(CameraLookFileError::Matrix);
        }
        let residual = file
            .residual
            .as_deref()
            .map(ResidualGrid::decode)
            .transpose()?;
        let mut look = Self::new(file.exposure_ev, curve, file.matrix, residual);
        look.set_amount(file.amount);
        Ok(look)
    }
}

impl From<CameraLook> for CameraLookFile {
    fn from(look: CameraLook) -> Self {
        Self {
            exposure_ev: look.exposure_ev,
            curve: look.curve.knots().to_vec(),
            matrix: look.matrix,
            residual: look.residual.as_ref().map(ResidualGrid::encode),
            amount: look.amount,
        }
    }
}
