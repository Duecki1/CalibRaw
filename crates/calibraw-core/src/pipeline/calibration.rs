//! Lightroom's Calibration sliders: the hue and saturation of the red, green
//! and blue primaries.
//!
//! Each primary of the working space (linear Rec.2020 with its D65 white) is
//! rotated about white in CIE 1931 xy by its Hue and moved towards or away
//! from white by its Saturation. [`Calibration::working_matrix`] reads working
//! RGB as if its primaries were the adjusted ones while white stays where it
//! was. The render folds it into the camera-to-working matrix. Adobe documents
//! the Calibration sliders as the same controls as the Color Matrices tab of
//! its DNG Profile Editor, which edits the camera profile's matrix, so like
//! Lightroom's they act before the profile's hue/saturation and look tables
//! and before exposure. Lightroom applies them to rendered photos too.
//!
//! The parametrization and slider ranges are those of ART's channel mixer in
//! its primaries mode (`get_mixer_matrix` in `src/engine/ipchmixer.cc`, ART
//! 1.26.10-77-g9429cd370), written by Alberto Griggio as an analogue of
//! Lightroom's Calibration after the four-color matrix method of Ohno and
//! Hardis (1997). It differs where ART does not keep white or a valid matrix:
//! - ART multiplies the adjusted matrix by the inverse of the original
//!   (`N * M^-1`), which tints white. Here working RGB goes to XYZ through the
//!   adjusted primaries and back through the original ones (`M^-1 * N`), as
//!   darktable's rgb primaries module does (`src/iop/primaries.c`, darktable
//!   5.6.0).
//! - ART takes the primaries from its D50-adapted ICC working matrix but
//!   rotates about, and balances to, D65. Here the primaries and white are the
//!   working space's own.
//! - ART lets neighbouring primaries rotate apart until white leaves their
//!   triangle and the matrix turns singular. Here no gap between neighbours
//!   opens further than one slider alone opens it ([`limited_rotations`]).

use crate::matrix::{self, Matrix3};

/// Limit of every Calibration slider, in Lightroom's units.
pub const CALIBRATION_LIMIT: f32 = 100.0;

/// ITU-R BT.2020 primaries (red, green, blue) and its D65 white in CIE 1931
/// xy: the working space.
const WORKING_PRIMARIES: [[f32; 2]; 3] = [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]];
const WORKING_WHITE: [f32; 2] = [0.3127, 0.3290];

/// Rotation of each primary about white at a Hue of 100, in degrees: ART's
/// 7.5 %, 10 % and 7.5 % of a turn.
const FULL_HUE_ROTATION_DEGREES: [f32; 3] = [27.0, 36.0, 27.0];
/// Relative change of each primary's distance from white at a Saturation of
/// ±100: ART's 30 %, 50 % and 50 %.
const FULL_SATURATION_CHANGE: [f32; 3] = [0.3, 0.5, 0.5];

/// One primary's Calibration sliders, each in
/// `[-CALIBRATION_LIMIT, CALIBRATION_LIMIT]` and neutral at 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PrimaryCalibration {
    /// Rotation about white: positive turns red towards orange, green
    /// towards cyan and blue towards purple, as in Lightroom and ART.
    pub hue: f32,
    /// Distance from white: positive is more saturated.
    pub saturation: f32,
}

impl PrimaryCalibration {
    fn is_neutral(self) -> bool {
        self.hue == 0.0 && self.saturation == 0.0
    }
}

/// Lightroom's Calibration panel, apart from its Shadows Tint.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Calibration {
    pub red: PrimaryCalibration,
    pub green: PrimaryCalibration,
    pub blue: PrimaryCalibration,
}

impl Calibration {
    pub fn is_neutral(&self) -> bool {
        self.primaries()
            .into_iter()
            .all(PrimaryCalibration::is_neutral)
    }

    fn primaries(&self) -> [PrimaryCalibration; 3] {
        [self.red, self.green, self.blue]
    }

    /// Linear working RGB to linear working RGB with the adjusted primaries,
    /// mapping white to itself. `None` when every slider is neutral, so edits
    /// without calibration render exactly as before.
    pub fn working_matrix(&self) -> Option<Matrix3> {
        if self.is_neutral() {
            return None;
        }
        let primaries = self.primaries();
        let rotations = limited_rotations(primaries.map(|primary| slider_fraction(primary.hue)));
        let adjusted = std::array::from_fn(|index| {
            let distance_scale =
                1.0 + slider_fraction(primaries[index].saturation) * FULL_SATURATION_CHANGE[index];
            adjusted_primary(WORKING_PRIMARIES[index], rotations[index], distance_scale)
        });
        // Invariant: with the rotations limited and every distance scale at
        // least 0.5, white stays inside the adjusted primaries' triangle, so
        // both matrices are invertible for finite slider values.
        let working = rgb_to_xyz(WORKING_PRIMARIES)?;
        let adjusted = rgb_to_xyz(adjusted)?;
        Some(matrix::multiply(matrix::invert(working)?, adjusted))
    }
}

/// A slider value as a fraction of its range, in `[-1, 1]`; 0 for a
/// non-finite value.
fn slider_fraction(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(-CALIBRATION_LIMIT, CALIBRATION_LIMIT) / CALIBRATION_LIMIT
    } else {
        0.0
    }
}

/// Each primary's rotation in degrees for Hue sliders given as fractions of
/// their range.
///
/// Positive rotations turn counter-clockwise in xy, the order in which the
/// primaries follow each other about white, so the gap from one primary to
/// the next opens by the next one's rotation minus its own. White leaves the
/// triangle, and the matrix turns singular, once a gap reaches 180°; it
/// approaches that near the corners of ART's ranges. Every gap may open as far
/// as one slider alone opens it at full range. When the rotations together
/// would open one further, all of them are scaled back by the same factor, so
/// each slider alone keeps ART's exact rotation.
fn limited_rotations(hue_fractions: [f32; 3]) -> [f32; 3] {
    let rotations: [f32; 3] =
        std::array::from_fn(|index| hue_fractions[index] * FULL_HUE_ROTATION_DEGREES[index]);
    let mut scale = 1.0_f32;
    for index in 0..3 {
        let next = (index + 1) % 3;
        let opening = rotations[next] - rotations[index];
        let limit = FULL_HUE_ROTATION_DEGREES[index].max(FULL_HUE_ROTATION_DEGREES[next]);
        if opening > limit {
            scale = scale.min(limit / opening);
        }
    }
    rotations.map(|rotation| rotation * scale)
}

/// `primary` rotated about white by `rotation_degrees` and with its distance
/// from white multiplied by `distance_scale` (ART's `tweak`).
fn adjusted_primary(primary: [f32; 2], rotation_degrees: f32, distance_scale: f32) -> [f32; 2] {
    let offset = [primary[0] - WORKING_WHITE[0], primary[1] - WORKING_WHITE[1]];
    let angle = offset[1].atan2(offset[0]) + rotation_degrees.to_radians();
    let distance = offset[0].hypot(offset[1]) * distance_scale;
    [
        WORKING_WHITE[0] + distance * angle.cos(),
        WORKING_WHITE[1] + distance * angle.sin(),
    ]
}

/// RGB to XYZ of the RGB space with `primaries` and the working white, scaled
/// so that white's X + Y + Z is 1 (ART's `get_matrix`; the scale cancels in
/// [`Calibration::working_matrix`]). The columns are the chromaticities
/// themselves rather than divided by y, so a primary moved below y = 0 stays
/// valid.
fn rgb_to_xyz(primaries: [[f32; 2]; 3]) -> Option<Matrix3> {
    let xyz = |[x, y]: [f32; 2]| [x, y, 1.0 - x - y];
    let columns = primaries.map(xyz);
    let chromaticities: Matrix3 =
        std::array::from_fn(|row| std::array::from_fn(|column| columns[column][row]));
    let weights = matrix::transform(matrix::invert(chromaticities)?, xyz(WORKING_WHITE));
    Some(std::array::from_fn(|row| {
        std::array::from_fn(|column| chromaticities[row][column] * weights[column])
    }))
}
