//! Warmth: the colour of light along the Planckian locus, shared by the
//! effects with a Warmth control (Glow, Light Rays). Warmth runs from -1
//! (cool) to 1 (warm); ±1 moves the light's temperature one octave from
//! neutral, so 0 is exactly white, 1 is 3252 K and -1 is 13008 K.

use crate::color_math::{rec2020_to_linear_srgb, srgb_encode};

/// The temperature of Warmth 0: white in the working space (D65).
const NEUTRAL_KELVIN: f32 = 6504.0;
/// Octaves of temperature that Warmth ±1 moves from neutral.
const WARMTH_OCTAVES: f32 = 1.0;

/// The tint of Warmth `warmth` (in [-1, 1]): scene-linear Rec.2020 with
/// luminance 1.
pub(super) fn warmth_tint(warmth: f32) -> [f32; 3] {
    planckian_tint(NEUTRAL_KELVIN * (-WARMTH_OCTAVES * warmth).exp2())
}

/// The tint of Warmth `warmth` as a display colour for swatches and canvas
/// handles: sRGB encoded, its brightest channel at 1.
pub(super) fn warmth_swatch(warmth: f32) -> [f32; 3] {
    let linear = rec2020_to_linear_srgb(warmth_tint(warmth)).map(|value| value.max(0.0));
    let peak = linear.into_iter().fold(f32::MIN_POSITIVE, f32::max);
    linear.map(|value| srgb_encode(value / peak))
}

/// The colour of a blackbody at `kelvin` relative to one at
/// [`NEUTRAL_KELVIN`], scene-linear Rec.2020 with luminance 1, so a tint at
/// the neutral temperature is exactly white.
fn planckian_tint(kelvin: f32) -> [f32; 3] {
    let rgb = planckian_rec2020(kelvin);
    let neutral = planckian_rec2020(NEUTRAL_KELVIN);
    let ratio: [f32; 3] = std::array::from_fn(|channel| rgb[channel] / neutral[channel]);
    let luminance = REC2020_LUMA
        .iter()
        .zip(ratio)
        .map(|(weight, value)| weight * value)
        .sum::<f32>();
    ratio.map(|value| value / luminance)
}

/// Rec.2020 luminance weights.
const REC2020_LUMA: [f32; 3] = [0.2627002, 0.6779981, 0.0593017];

/// The linear Rec.2020 colour of a blackbody at `kelvin` (1667 K to 25000 K),
/// with luminance 1: its chromaticity from the cubic approximation of the
/// Planckian locus by Kim et al. (2002).
fn planckian_rec2020(kelvin: f32) -> [f32; 3] {
    let t = f64::from(kelvin.clamp(1667.0, 25000.0));
    let (t2, t3) = (t * t, t * t * t);
    let x = if t <= 4000.0 {
        -0.2661239e9 / t3 - 0.2343589e6 / t2 + 0.8776956e3 / t + 0.179910
    } else {
        -3.0258469e9 / t3 + 2.1070379e6 / t2 + 0.2226347e3 / t + 0.240390
    };
    let (x2, x3) = (x * x, x * x * x);
    let y = if t <= 2222.0 {
        -1.1063814 * x3 - 1.34811020 * x2 + 2.18555832 * x - 0.20219683
    } else if t <= 4000.0 {
        -0.9549476 * x3 - 1.37418593 * x2 + 2.09137015 * x - 0.16748867
    } else {
        3.0817580 * x3 - 5.87338670 * x2 + 3.75112997 * x - 0.37001483
    };
    let xyz = [x / y, 1.0, (1.0 - x - y) / y];
    // CIE XYZ (D65) to linear Rec.2020 (ITU-R BT.2020).
    const XYZ_TO_REC2020: [[f64; 3]; 3] = [
        [1.7166512, -0.3556708, -0.2533663],
        [-0.6666844, 1.6164812, 0.0157685],
        [0.0176399, -0.0427706, 0.9421031],
    ];
    XYZ_TO_REC2020.map(|row| (row[0] * xyz[0] + row[1] * xyz[1] + row[2] * xyz[2]) as f32)
}
