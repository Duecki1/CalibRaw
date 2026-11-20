//! The colour spaces 8- and 16-bit exports encode, and the transform that
//! encodes the pipeline's display-linear Rec.2020 (D65) into one of them.
//!
//! Every space is a matrix/TRC space defined by its primaries, white and curve.
//! The encoder derives its matrix from the same rounded colorants the embedded
//! ICC profile records ([`OutputColorSpace::pcs_colorants`]), so a
//! colour-managed reader decodes exactly what was encoded, up to the 16-bit
//! sample rounding.

use super::color_profile::{
    display_linear_rec2020_to_srgb, perceptual_gamut_compress_unit, rgb_is_unit,
};
use crate::color_math::{rec2020_to_linear_srgb, srgb_encode};
use crate::matrix::{transform, Matrix3};

/// The ICC profile connection space white (ICC.1, the PCS illuminant field),
/// D50 rounded to four decimals as every ICC profile records it.
pub const ICC_PCS_D50_XYZ: [f32; 3] = [0.9642, 1.0, 0.8249];

/// An output colour space of 8- and 16-bit exports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutputColorSpace {
    /// IEC 61966-2-1.
    #[default]
    Srgb,
    /// P3 primaries (SMPTE EG 432-1) with a D65 white and the sRGB curve.
    DisplayP3,
    /// Adobe RGB (1998): D65, pure gamma 563/256.
    AdobeRgb,
    /// ROMM RGB (ISO 22028-2) primaries and D50 white with a pure 1.8 gamma,
    /// as Adobe's ProPhoto RGB profile encodes it, without ROMM's linear toe
    /// below 1/512 (which falls between 8-bit codes 0 and 1).
    ProPhotoRgb,
    /// ITU-R BT.2020 primaries, D65, pure gamma 2.4 (the BT.1886 reference
    /// display without its black offset, as CSS Color 4 defines `rec2020`).
    Rec2020,
}

/// How linear output values are encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputTransfer {
    /// The IEC 61966-2-1 piecewise curve.
    Srgb,
    /// A pure power law, `encoded = linear^(1/γ)` with `γ = u8_fixed8 / 256`:
    /// the ICC u8Fixed8Number a single-entry `curv` tag stores, so the
    /// encoder and the profile use the same exponent exactly.
    Gamma { u8_fixed8: u16 },
}

impl OutputTransfer {
    /// Encodes a linear value, clamped to `[0, 1]`.
    fn encode(self, linear: f32) -> f32 {
        match self {
            Self::Srgb => srgb_encode(linear),
            Self::Gamma { u8_fixed8 } => linear.clamp(0.0, 1.0).powf(256.0 / f32::from(u8_fixed8)),
        }
    }
}

/// CIE 1931 xy chromaticities of a space's primaries and white.
struct Chromaticities {
    red: [f64; 2],
    green: [f64; 2],
    blue: [f64; 2],
    white: [f64; 2],
}

const D65_XY: [f64; 2] = [0.3127, 0.3290];

impl OutputColorSpace {
    pub const ALL: [Self; 5] = [
        Self::Srgb,
        Self::DisplayP3,
        Self::AdobeRgb,
        Self::ProPhotoRgb,
        Self::Rec2020,
    ];

    /// A stable name for settings files and the command line.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Srgb => "srgb",
            Self::DisplayP3 => "display-p3",
            Self::AdobeRgb => "adobe-rgb",
            Self::ProPhotoRgb => "prophoto-rgb",
            Self::Rec2020 => "rec2020",
        }
    }

    /// The space whose [`Self::id`] is `id`.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|space| space.id() == id)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Srgb => "sRGB",
            Self::DisplayP3 => "Display P3",
            Self::AdobeRgb => "Adobe RGB (1998)",
            Self::ProPhotoRgb => "ProPhoto RGB",
            Self::Rec2020 => "Rec. 2020",
        }
    }

    pub const fn transfer(self) -> OutputTransfer {
        match self {
            Self::Srgb | Self::DisplayP3 => OutputTransfer::Srgb,
            Self::AdobeRgb => OutputTransfer::Gamma { u8_fixed8: 563 },
            Self::ProPhotoRgb => OutputTransfer::Gamma { u8_fixed8: 461 },
            Self::Rec2020 => OutputTransfer::Gamma { u8_fixed8: 614 },
        }
    }

    const fn chromaticities(self) -> Chromaticities {
        match self {
            Self::Srgb => Chromaticities {
                red: [0.64, 0.33],
                green: [0.30, 0.60],
                blue: [0.15, 0.06],
                white: D65_XY,
            },
            Self::DisplayP3 => Chromaticities {
                red: [0.680, 0.320],
                green: [0.265, 0.690],
                blue: [0.150, 0.060],
                white: D65_XY,
            },
            Self::AdobeRgb => Chromaticities {
                red: [0.64, 0.33],
                green: [0.21, 0.71],
                blue: [0.15, 0.06],
                white: D65_XY,
            },
            Self::ProPhotoRgb => Chromaticities {
                red: [0.7347, 0.2653],
                green: [0.1596, 0.8404],
                blue: [0.0366, 0.0001],
                white: [0.3457, 0.3585],
            },
            Self::Rec2020 => Chromaticities {
                red: [0.708, 0.292],
                green: [0.170, 0.797],
                blue: [0.131, 0.046],
                white: D65_XY,
            },
        }
    }

    /// The RGB-to-PCS matrix an ICC matrix/TRC profile records in its
    /// `rXYZ`, `gXYZ` and `bXYZ` tags (the columns): the primaries' XYZ,
    /// scaled so RGB white is the space's white at Y = 1, Bradford-adapted to
    /// [`ICC_PCS_D50_XYZ`] and rounded to s15Fixed16Number. Rounding is
    /// corrected on each row's largest entry so the columns sum exactly to the
    /// rounded PCS white the profile records: RGB white then decodes to the
    /// PCS white, neutral to every reader.
    pub fn pcs_colorants(self) -> Matrix3 {
        let space = self.chromaticities();
        let adapted = multiply64(self.adaptation64(), native_rgb_to_xyz(&space));
        let mut colorants = adapted.map(|row| row.map(|value| (value * 65_536.0).round() as i64));
        for (row, white) in colorants.iter_mut().zip(ICC_PCS_D50_XYZ) {
            let target = (f64::from(white) * 65_536.0).round() as i64;
            let residual = target - row.iter().sum::<i64>();
            if let Some(largest) = row.iter_mut().max_by_key(|value| value.abs()) {
                *largest += residual;
            }
        }
        colorants.map(|row| row.map(|value| value as f32 / 65_536.0))
    }

    /// The Bradford adaptation from the space's white to [`ICC_PCS_D50_XYZ`],
    /// row-major and rounded to s15Fixed16Number, for an ICC `chad` tag.
    pub fn pcs_adaptation(self) -> Matrix3 {
        self.adaptation64().map(|row| row.map(s15_fixed16))
    }

    fn adaptation64(self) -> [[f64; 3]; 3] {
        let white = xy_to_xyz(self.chromaticities().white);
        bradford64(white, ICC_PCS_D50_XYZ.map(f64::from))
    }
}

/// Encodes display-linear Rec.2020 (D65) for an 8- or 16-bit export in an
/// [`OutputColorSpace`].
///
/// Colours outside the space keep their OKLab lightness and hue while their
/// chroma rolls off to its boundary, as the preview does for sRGB; colours
/// inside it are converted exactly. sRGB output is exactly the preview's
/// encoding.
#[derive(Clone, Copy, Debug)]
pub struct OutputTransform {
    space: OutputColorSpace,
    /// Linear sRGB to linear output RGB, through the PCS of both profiles.
    linear_srgb_to_output: Matrix3,
    transfer: OutputTransfer,
}

impl OutputTransform {
    pub fn new(space: OutputColorSpace) -> Self {
        let srgb_to_pcs = OutputColorSpace::Srgb
            .pcs_colorants()
            .map(|row| row.map(f64::from));
        let output_to_pcs = space.pcs_colorants().map(|row| row.map(f64::from));
        // Colorants are invertible for every listed space: their primaries
        // are linearly independent chromaticities.
        let pcs_to_output = invert64(output_to_pcs).unwrap_or(IDENTITY64);
        let linear_srgb_to_output =
            multiply64(pcs_to_output, srgb_to_pcs).map(|row| row.map(|value| value as f32));
        Self {
            space,
            linear_srgb_to_output,
            transfer: space.transfer(),
        }
    }

    pub fn color_space(&self) -> OutputColorSpace {
        self.space
    }

    /// Encoded output RGB in `[0, 1]` for display-linear Rec.2020 `rgb`.
    pub fn transform_rgb(&self, rgb: [f32; 3]) -> [f32; 3] {
        if self.space == OutputColorSpace::Srgb {
            return display_linear_rec2020_to_srgb(rgb);
        }
        let to_output = self.linear_srgb_to_output;
        let compressed = perceptual_gamut_compress_unit(rec2020_to_linear_srgb(rgb), |srgb| {
            rgb_is_unit(transform(to_output, srgb))
        });
        transform(to_output, compressed).map(|value| self.transfer.encode(value))
    }
}

const IDENTITY64: [[f64; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

fn s15_fixed16(value: f64) -> f32 {
    ((value * 65_536.0).round() / 65_536.0) as f32
}

fn xy_to_xyz([x, y]: [f64; 2]) -> [f64; 3] {
    [x / y, 1.0, (1.0 - x - y) / y]
}

/// RGB to XYZ relative to the space's own white (Y of white = 1).
fn native_rgb_to_xyz(space: &Chromaticities) -> [[f64; 3]; 3] {
    let [red, green, blue] = [space.red, space.green, space.blue].map(xy_to_xyz);
    let primaries = [
        [red[0], green[0], blue[0]],
        [red[1], green[1], blue[1]],
        [red[2], green[2], blue[2]],
    ];
    let white = xy_to_xyz(space.white);
    let scale = transform64(invert64(primaries).unwrap_or(IDENTITY64), white);
    primaries.map(|row| std::array::from_fn(|column| row[column] * scale[column]))
}

/// Linear Bradford adaptation from `source` to `target` white, in `f64`.
fn bradford64(source: [f64; 3], target: [f64; 3]) -> [[f64; 3]; 3] {
    const BRADFORD: [[f64; 3]; 3] = [
        [0.8951, 0.2664, -0.1614],
        [-0.7502, 1.7135, 0.0367],
        [0.0389, -0.0685, 1.0296],
    ];
    let inverse = invert64(BRADFORD).unwrap_or(IDENTITY64);
    let source_lms = transform64(BRADFORD, source);
    let target_lms = transform64(BRADFORD, target);
    let diagonal: [[f64; 3]; 3] = std::array::from_fn(|row| {
        std::array::from_fn(|column| {
            if row == column {
                target_lms[row] / source_lms[row]
            } else {
                0.0
            }
        })
    });
    multiply64(inverse, multiply64(diagonal, BRADFORD))
}

fn multiply64(left: [[f64; 3]; 3], right: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    std::array::from_fn(|row| {
        std::array::from_fn(|column| (0..3).map(|k| left[row][k] * right[k][column]).sum())
    })
}

fn transform64(matrix: [[f64; 3]; 3], vector: [f64; 3]) -> [f64; 3] {
    matrix.map(|row| (0..3).map(|k| row[k] * vector[k]).sum())
}

/// Inverse by cofactors; `None` for a singular matrix.
fn invert64(m: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let cofactor =
        |r0: usize, r1: usize, c0: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    let adjugate = [
        [
            cofactor(1, 2, 1, 2),
            -cofactor(0, 2, 1, 2),
            cofactor(0, 1, 1, 2),
        ],
        [
            -cofactor(1, 2, 0, 2),
            cofactor(0, 2, 0, 2),
            -cofactor(0, 1, 0, 2),
        ],
        [
            cofactor(1, 2, 0, 1),
            -cofactor(0, 2, 0, 1),
            cofactor(0, 1, 0, 1),
        ],
    ];
    let determinant =
        m[0][0] * adjugate[0][0] + m[0][1] * adjugate[1][0] + m[0][2] * adjugate[2][0];
    (determinant.abs() > 1e-12).then(|| adjugate.map(|row| row.map(|value| value / determinant)))
}
