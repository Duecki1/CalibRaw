//! Perspective correction: a rotation of a virtual camera about the photo's centre that makes
//! converging lines parallel, and its automatic and guided estimates.

use super::bisect_largest;
use super::mapping::Homography;
use serde::{Deserialize, Serialize};
use std::ops::RangeInclusive;

mod estimate;
mod fit;
mod guided;
pub use estimate::{estimate_perspective, PerspectiveEstimate};
pub use guided::{solve_guided_perspective, PerspectiveGuide};

/// Diagonal of the 35 mm film frame, mm.
const FULL_FRAME_DIAGONAL_MM: f32 = 43.266_617;
/// How much larger, linearly, the most magnified part of the frame may be drawn than its
/// centre. Angles are reduced together until the correction stays within it, which keeps the
/// vanishing line well outside the frame.
const MAX_EDGE_MAGNIFICATION: f32 = 4.0;

/// Rotates the camera that took the photo, as if it had been pointed differently: pitching it
/// makes vertical lines that converge parallel again, yawing it does the same for horizontals.
///
/// Angles are about the source's own axes, before flips and quarter turns, with the source's
/// x to the right, y down and the view along +z; the pitch is applied first. The corrected image
/// keeps the source's centre in place, scaled so that its area there is unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct PerspectiveCorrection {
    /// Rotation about the source's horizontal axis, in degrees. Positive values widen the top
    /// of the source and narrow its bottom.
    #[serde(default)]
    pub pitch_degrees: f32,
    /// Rotation about the vertical axis after the pitch, in degrees. Positive values widen the
    /// right side of the source.
    #[serde(default)]
    pub yaw_degrees: f32,
    /// 35 mm-equivalent focal length the rotation is made with, mm. It is the photo's when the
    /// correction is first set and is kept with it, so the result never depends on metadata or
    /// lens databases read later.
    #[serde(default = "generic_focal_length")]
    pub focal_length_35mm: f32,
}

const fn generic_focal_length() -> f32 {
    PerspectiveCorrection::GENERIC_FOCAL_LENGTH_35MM
}

impl Default for PerspectiveCorrection {
    fn default() -> Self {
        Self {
            pitch_degrees: 0.0,
            yaw_degrees: 0.0,
            focal_length_35mm: Self::GENERIC_FOCAL_LENGTH_35MM,
        }
    }
}

impl PerspectiveCorrection {
    /// Largest pitch and yaw. Wide lenses allow less (see `MAX_EDGE_MAGNIFICATION`).
    pub const MAX_DEGREES: f32 = 45.0;
    /// Focal length for photos that do not state theirs or their crop factor: a moderate wide
    /// angle, as darktable assumes.
    pub const GENERIC_FOCAL_LENGTH_35MM: f32 = 28.0;
    pub const FOCAL_LENGTH_RANGE: RangeInclusive<f32> = 4.0..=2000.0;

    /// The 35 mm-equivalent focal length of a photo taken at `focal_length_mm` on a sensor with
    /// `crop_factor`, or the generic one when either is unknown.
    pub fn focal_length_35mm_for(focal_length_mm: f32, crop_factor: Option<f32>) -> f32 {
        crop_factor
            .map(|crop_factor| focal_length_mm * crop_factor)
            .filter(|focal| focal.is_finite() && Self::FOCAL_LENGTH_RANGE.contains(focal))
            .unwrap_or(Self::GENERIC_FOCAL_LENGTH_35MM)
    }

    pub fn is_identity(&self) -> bool {
        self.pitch_degrees.abs() < 1e-4 && self.yaw_degrees.abs() < 1e-4
    }

    /// Finite angles within [`Self::MAX_DEGREES`], reduced together where the lens is too wide
    /// for them, and a focal length within [`Self::FOCAL_LENGTH_RANGE`]. Without a rotation the
    /// focal length does not matter and is reset, so equal corrections compare equal.
    pub fn sanitized(self) -> Self {
        let clamp = |degrees: f32| {
            if degrees.is_finite() {
                degrees.clamp(-Self::MAX_DEGREES, Self::MAX_DEGREES)
            } else {
                0.0
            }
        };
        let mut value = Self {
            pitch_degrees: clamp(self.pitch_degrees),
            yaw_degrees: clamp(self.yaw_degrees),
            focal_length_35mm: if self.focal_length_35mm.is_finite() {
                self.focal_length_35mm.clamp(
                    *Self::FOCAL_LENGTH_RANGE.start(),
                    *Self::FOCAL_LENGTH_RANGE.end(),
                )
            } else {
                Self::GENERIC_FOCAL_LENGTH_35MM
            },
        };
        if value.is_identity() {
            return Self::default();
        }
        if value.magnification_slack(1.0) < 0.0 {
            // Exact in single precision: 24 halvings stay within its mantissa.
            let low =
                bisect_largest(24, |scale| value.magnification_slack(scale as f32) >= 0.0) as f32;
            value.pitch_degrees *= low;
            value.yaw_degrees *= low;
        }
        value
    }

    /// How far the angles scaled by `scale` stay within `MAX_EDGE_MAGNIFICATION` anywhere within
    /// half the frame's diagonal of the centre; negative beyond it. It decreases as `scale`
    /// grows.
    ///
    /// A ray `(x, y, f)` of the source has depth `R31·x + R32·y + R33·f` after the rotation `R`,
    /// and its magnification is the centre's depth over its own.
    fn magnification_slack(self, scale: f32) -> f32 {
        let [_, _, [r31, r32, r33]] =
            rotation(self.pitch_degrees * scale, self.yaw_degrees * scale);
        let radius_over_focal = 0.5 * FULL_FRAME_DIAGONAL_MM / self.focal_length_35mm;
        let shallowest = r33 - radius_over_focal * r31.hypot(r32);
        shallowest - r33 / MAX_EDGE_MAGNIFICATION
    }

    /// The correction as a map of pixel positions of a `size` source (continuous pixels, top-left
    /// at 0). The identity without a rotation.
    pub(super) fn homography(self, size: [f64; 2]) -> Homography {
        if self.is_identity() {
            return Homography::IDENTITY;
        }
        let center = [size[0] * 0.5, size[1] * 0.5];
        let focal =
            f64::from(self.focal_length_35mm / FULL_FRAME_DIAGONAL_MM) * size[0].hypot(size[1]);
        // K · R · K⁻¹ with K = diag(f, f, 1), on positions relative to the centre.
        let r = rotation(self.pitch_degrees, self.yaw_degrees).map(|row| row.map(f64::from));
        let projective = Homography::from_rows([
            [r[0][0], r[0][1], r[0][2] * focal],
            [r[1][0], r[1][1], r[1][2] * focal],
            [r[2][0] / focal, r[2][1] / focal, r[2][2]],
        ]);
        // Where the centre goes, and the area scale there, from the derivative of
        // (m₁·p / m₃·p, m₂·p / m₃·p) at p = (0, 0, 1).
        let m = projective.rows_f64();
        let moved = [m[0][2] / m[2][2], m[1][2] / m[2][2]];
        let derivative = |row: usize, column: usize| {
            (m[row][column] * m[2][2] - m[row][2] * m[2][column]) / (m[2][2] * m[2][2])
        };
        let area = derivative(0, 0) * derivative(1, 1) - derivative(0, 1) * derivative(1, 0);
        let scale = area.abs().sqrt().recip();
        Homography::translation([-center[0], -center[1]])
            .then(projective)
            .then(Homography::affine(
                [[scale, 0.0], [0.0, scale]],
                [center[0] - scale * moved[0], center[1] - scale * moved[1]],
            ))
    }
}

/// `R_y(yaw) · R_x(pitch)` for y-down image coordinates looking along +z.
fn rotation(pitch_degrees: f32, yaw_degrees: f32) -> [[f32; 3]; 3] {
    let (sp, cp) = pitch_degrees.to_radians().sin_cos();
    let (sy, cy) = yaw_degrees.to_radians().sin_cos();
    // R_x = [[1, 0, 0], [0, cp, -sp], [0, sp, cp]], R_y = [[cy, 0, sy], [0, 1, 0], [-sy, 0, cy]].
    [
        [cy, sy * sp, sy * cp],
        [0.0, cp, -sp],
        [-sy, cy * sp, cy * cp],
    ]
}
