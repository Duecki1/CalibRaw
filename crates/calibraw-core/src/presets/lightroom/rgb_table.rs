//! Adobe `RGBTable` lookup tables: the colour look of a Lightroom or Camera
//! Raw profile, stored in the `crs:Table_<fingerprint>` property of an XMP
//! packet.
//!
//! The property is a base-85 text (a Z85 variant with an alphabet that is safe
//! inside XMP) of a zlib stream that is prefixed with its decompressed size.
//! The stream holds the table: its divisions, per-point offsets from the
//! identity table, and the primaries and transfer curve the table is defined
//! in. The layout follows Adobe's published DNG SDK (`dng_big_table`).
//!
//! Adobe applies these tables inside its own rendering. CalibRaw applies a look
//! to the finished sRGB image, so the table is re-expressed in sRGB-encoded
//! coordinates: each point of the result converts its colour into the table's
//! primaries and curve, reads the table there, and converts back.

use crate::pipeline::{ColorLut, ColorLutError};
use flate2::read::ZlibDecoder;
use std::io::Read;

/// 32 divisions is the most Adobe writes for a 3D table.
const MAX_DIVISIONS_3D: u32 = 32;
const BIG_TABLE_TYPE_RGB: u32 = 1;
const RGB_TABLE_VERSION: u32 = 1;
/// A 32-division table is about 200 KB; leave room for headers.
const MAX_DECOMPRESSED_BYTES: usize = 8 * 1024 * 1024;
/// Points per axis of the sRGB table that results. 33 holds a 32-division
/// table exactly at its resolution.
const RESULT_EDGE: u32 = 33;

const ALPHABET: &[u8; 85] =
    b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-:+=^!/*?`'|()[]{}@%$#";

fn invalid<T>(message: impl Into<String>) -> Result<T, ColorLutError> {
    Err(ColorLutError::Invalid(message.into()))
}

/// Decodes the base-85 text. Characters outside the alphabet (whitespace and
/// line breaks) are skipped.
fn decode_text(text: &str) -> Vec<u8> {
    let mut lookup = [u8::MAX; 256];
    for (value, &character) in ALPHABET.iter().enumerate() {
        lookup[character as usize] = value as u8;
    }
    let mut bytes = Vec::with_capacity(text.len() / 5 * 4 + 4);
    let mut value = 0u32;
    let mut phase = 0u32;
    for &character in text.as_bytes() {
        let digit = lookup[character as usize];
        if digit == u8::MAX {
            continue;
        }
        value = value.wrapping_add(u32::from(digit).wrapping_mul(85u32.wrapping_pow(phase)));
        phase += 1;
        if phase == 5 {
            bytes.extend_from_slice(&value.to_le_bytes());
            value = 0;
            phase = 0;
        }
    }
    // A partial group of n characters holds n - 1 bytes.
    if phase > 1 {
        bytes.extend_from_slice(&value.to_le_bytes()[..phase as usize - 1]);
    }
    bytes
}

fn decompress(bytes: &[u8]) -> Result<Vec<u8>, ColorLutError> {
    if bytes.len() < 5 {
        return invalid("the LUT data is too short");
    }
    let size = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if size > MAX_DECOMPRESSED_BYTES {
        return invalid("the LUT data claims an unreasonable size");
    }
    let mut decompressed = Vec::with_capacity(size);
    ZlibDecoder::new(&bytes[4..])
        .take(size as u64 + 1)
        .read_to_end(&mut decompressed)
        .map_err(|error| ColorLutError::Invalid(format!("the LUT data is damaged: {error}")))?;
    if decompressed.len() > size {
        return invalid("the LUT data is larger than it says");
    }
    Ok(decompressed)
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], ColorLutError> {
        let end = self
            .position
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len());
        let Some(end) = end else {
            return invalid("the LUT data ends early");
        };
        let slice = &self.bytes[self.position..end];
        self.position = end;
        Ok(slice)
    }

    fn u16(&mut self) -> Result<u16, ColorLutError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, ColorLutError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Primaries {
    Srgb,
    AdobeRgb,
    ProPhoto,
    DisplayP3,
    Rec2020,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gamma {
    Linear,
    Srgb,
    Power1_8,
    Power2_2,
    Rec2020,
}

/// A decoded 3D table: `divisions³` colours in the table's own primaries and
/// curve, red outermost and blue innermost, as 16-bit values.
struct RgbTable {
    divisions: usize,
    samples: Vec<[u16; 3]>,
    primaries: Primaries,
    gamma: Gamma,
}

impl RgbTable {
    fn parse(bytes: &[u8]) -> Result<Self, ColorLutError> {
        let mut reader = Reader { bytes, position: 0 };
        if reader.u32()? != BIG_TABLE_TYPE_RGB {
            return invalid("the LUT data is not an RGB table");
        }
        if reader.u32()? != RGB_TABLE_VERSION {
            return Err(ColorLutError::Unsupported(
                "this RGB table version is not supported".to_owned(),
            ));
        }
        let dimensions = reader.u32()?;
        let divisions = reader.u32()?;
        if dimensions != 3 {
            return Err(ColorLutError::Unsupported(
                "only 3D RGB tables are supported".to_owned(),
            ));
        }
        if !(2..=MAX_DIVISIONS_3D).contains(&divisions) {
            return invalid("the RGB table has an invalid size");
        }
        let divisions = divisions as usize;

        // Each point stores its offset from the identity table.
        let identity: Vec<u16> = (0..divisions)
            .map(|index| ((index * 0xFFFF + (divisions >> 1)) / (divisions - 1)) as u16)
            .collect();
        let mut samples = Vec::with_capacity(divisions.pow(3));
        for red in 0..divisions {
            for green in 0..divisions {
                for blue in 0..divisions {
                    samples.push([
                        reader.u16()?.wrapping_add(identity[red]),
                        reader.u16()?.wrapping_add(identity[green]),
                        reader.u16()?.wrapping_add(identity[blue]),
                    ]);
                }
            }
        }

        let primaries = match reader.u32()? {
            0 => Primaries::Srgb,
            1 => Primaries::AdobeRgb,
            2 => Primaries::ProPhoto,
            3 => Primaries::DisplayP3,
            4 => Primaries::Rec2020,
            _ => return invalid("the RGB table uses unknown primaries"),
        };
        let gamma = match reader.u32()? {
            0 => Gamma::Linear,
            1 => Gamma::Srgb,
            2 => Gamma::Power1_8,
            3 => Gamma::Power2_2,
            4 => Gamma::Rec2020,
            _ => return invalid("the RGB table uses an unknown transfer curve"),
        };
        // The gamut option, amount range and flags that follow do not change
        // the colours a table produces.
        Ok(Self {
            divisions,
            samples,
            primaries,
            gamma,
        })
    }

    /// The table's output for `coordinate`, encoded in the table's curve and
    /// each channel in `[0, 1]`. Linear interpolation between points.
    fn sample(&self, coordinate: [f64; 3]) -> [f64; 3] {
        let last = (self.divisions - 1) as f64;
        let mut lower = [0usize; 3];
        let mut fraction = [0.0f64; 3];
        for channel in 0..3 {
            let position = coordinate[channel].clamp(0.0, 1.0) * last;
            let floor = position.floor().min(last - 1.0);
            lower[channel] = floor as usize;
            fraction[channel] = position - floor;
        }
        let at = |red: usize, green: usize, blue: usize, channel: usize| {
            f64::from(self.samples[(red * self.divisions + green) * self.divisions + blue][channel])
                / 65535.0
        };
        let mut output = [0.0; 3];
        for (channel, value) in output.iter_mut().enumerate() {
            let mix = |a: f64, b: f64, t: f64| a + (b - a) * t;
            let plane = |red: usize| {
                let green_low = mix(
                    at(red, lower[1], lower[2], channel),
                    at(red, lower[1], lower[2] + 1, channel),
                    fraction[2],
                );
                let green_high = mix(
                    at(red, lower[1] + 1, lower[2], channel),
                    at(red, lower[1] + 1, lower[2] + 1, channel),
                    fraction[2],
                );
                mix(green_low, green_high, fraction[1])
            };
            *value = mix(plane(lower[0]), plane(lower[0] + 1), fraction[0]);
        }
        output
    }
}

type Matrix = [[f64; 3]; 3];

fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    let mut result = [[0.0; 3]; 3];
    for row in 0..3 {
        for column in 0..3 {
            result[row][column] = (0..3).map(|k| a[row][k] * b[k][column]).sum();
        }
    }
    result
}

fn apply(matrix: &Matrix, vector: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|row| (0..3).map(|k| matrix[row][k] * vector[k]).sum())
}

fn invert(m: &Matrix) -> Matrix {
    let determinant = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let inverse_determinant = 1.0 / determinant;
    [
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inverse_determinant,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inverse_determinant,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inverse_determinant,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inverse_determinant,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inverse_determinant,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inverse_determinant,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inverse_determinant,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inverse_determinant,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inverse_determinant,
        ],
    ]
}

const D50: [f64; 2] = [0.3457, 0.3585];
const D65: [f64; 2] = [0.3127, 0.3290];

fn xyz_from_xy([x, y]: [f64; 2]) -> [f64; 3] {
    [x / y, 1.0, (1.0 - x - y) / y]
}

impl Primaries {
    /// Chromaticities of red, green and blue, and of the white point.
    fn chromaticities(self) -> ([[f64; 2]; 3], [f64; 2]) {
        match self {
            Self::Srgb => ([[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]], D65),
            Self::AdobeRgb => ([[0.64, 0.33], [0.21, 0.71], [0.15, 0.06]], D65),
            Self::ProPhoto => ([[0.7347, 0.2653], [0.1596, 0.8404], [0.0366, 0.0001]], D50),
            Self::DisplayP3 => ([[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]], D65),
            Self::Rec2020 => ([[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]], D65),
        }
    }

    /// Linear RGB to XYZ relative to D50, with D65 spaces adapted by Bradford.
    fn to_xyz_d50(self) -> Matrix {
        let (primaries, white) = self.chromaticities();
        let columns = primaries.map(xyz_from_xy);
        let primaries_matrix = [
            [columns[0][0], columns[1][0], columns[2][0]],
            [columns[0][1], columns[1][1], columns[2][1]],
            [columns[0][2], columns[1][2], columns[2][2]],
        ];
        let scale = apply(&invert(&primaries_matrix), xyz_from_xy(white));
        let to_xyz: Matrix = [0, 1, 2]
            .map(|row| [0, 1, 2].map(|column| primaries_matrix[row][column] * scale[column]));
        if white == D50 {
            to_xyz
        } else {
            multiply(&bradford(white, D50), &to_xyz)
        }
    }
}

/// The Bradford chromatic adaptation from white point `from` to `to`.
fn bradford(from: [f64; 2], to: [f64; 2]) -> Matrix {
    const CONE: Matrix = [
        [0.8951, 0.2664, -0.1614],
        [-0.7502, 1.7135, 0.0367],
        [0.0389, -0.0685, 1.0296],
    ];
    let source = apply(&CONE, xyz_from_xy(from));
    let target = apply(&CONE, xyz_from_xy(to));
    let scale: Matrix = [
        [target[0] / source[0], 0.0, 0.0],
        [0.0, target[1] / source[1], 0.0],
        [0.0, 0.0, target[2] / source[2]],
    ];
    multiply(&invert(&CONE), &multiply(&scale, &CONE))
}

fn srgb_decode(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn srgb_encode(value: f64) -> f64 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

const REC2020_ALPHA: f64 = 1.099_296_826_809_44;
const REC2020_BETA: f64 = 0.018_053_968_510_807;

impl Gamma {
    fn encode(self, linear: f64) -> f64 {
        let linear = linear.clamp(0.0, 1.0);
        match self {
            Self::Linear => linear,
            Self::Srgb => srgb_encode(linear),
            Self::Power1_8 => linear.powf(1.0 / 1.8),
            Self::Power2_2 => linear.powf(1.0 / 2.2),
            Self::Rec2020 => {
                if linear < REC2020_BETA {
                    4.5 * linear
                } else {
                    REC2020_ALPHA * linear.powf(0.45) - (REC2020_ALPHA - 1.0)
                }
            }
        }
    }

    fn decode(self, encoded: f64) -> f64 {
        let encoded = encoded.clamp(0.0, 1.0);
        match self {
            Self::Linear => encoded,
            Self::Srgb => srgb_decode(encoded),
            Self::Power1_8 => encoded.powf(1.8),
            Self::Power2_2 => encoded.powf(2.2),
            Self::Rec2020 => {
                if encoded < 4.5 * REC2020_BETA {
                    encoded / 4.5
                } else {
                    ((encoded + REC2020_ALPHA - 1.0) / REC2020_ALPHA).powf(1.0 / 0.45)
                }
            }
        }
    }
}

/// Decodes the `Table_<fingerprint>` text of a Lightroom profile into a table
/// for sRGB-encoded images.
pub(super) fn decode_srgb_table(text: &str) -> Result<ColorLut, ColorLutError> {
    let table = RgbTable::parse(&decompress(&decode_text(text))?)?;

    let srgb_to_xyz = Primaries::Srgb.to_xyz_d50();
    let into_table = multiply(&invert(&table.primaries.to_xyz_d50()), &srgb_to_xyz);
    let out_of_table = invert(&into_table);
    ColorLut::from_function(RESULT_EDGE, |rgb| {
        let linear = rgb.map(|value| srgb_decode(f64::from(value)));
        let in_table = apply(&into_table, linear);
        let coordinate = in_table.map(|value| table.gamma.encode(value));
        let result = table
            .sample(coordinate)
            .map(|value| table.gamma.decode(value));
        apply(&out_of_table, result).map(|value| srgb_encode(value.clamp(0.0, 1.0)) as f32)
    })
}

#[cfg(test)]
pub(super) mod tests;
