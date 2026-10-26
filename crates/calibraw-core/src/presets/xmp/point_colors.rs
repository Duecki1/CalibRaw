//! Point Color swatches of XMP presets.
//!
//! A swatch is 19 numbers: the sampled hue (in sixths of a turn), saturation
//! and value; the hue, saturation and value shifts (±1 at the slider ends);
//! the range amount (0–1); then the hue, saturation and luminance ranges,
//! each four points from "no effect" through "full effect" and back, as
//! fractions (0–1) of a window around the sample. Unused slots hold -1. The
//! sample is HSV of linear ProPhoto RGB.
//!
//! CalibRaw samples HSL of the display-linear Rec.2020 colour and shifts it
//! in its own units, so the translation is approximate: the sample colour is
//! converted exactly, the shifts and ranges are rescaled to CalibRaw's
//! slider ranges, and the per-swatch variance is not imported.

use super::document::{parse_number, Element, Warnings};
use crate::color_math::LINEAR_PROPHOTO_TO_REC2020;
use crate::matrix::transform;
use crate::pipeline::{PointColor, PointColorRange, PointColors, MAX_POINT_COLORS};

const SWATCH_VALUES: usize = 19;
/// Hue rotation, in turns, of a hue shift of ±1.
const HUE_SHIFT_TURNS: f32 = 35.0 / 360.0;
/// CalibRaw's hue shift that rotates the hue by one turn.
const CALIBRAW_HUE_SHIFT_PER_TURN: f32 = 200.0;
/// Half the hue window, in turns, that a 0–1 hue range spans: the outer
/// edge of CalibRaw's default hue range.
const HUE_WINDOW: f32 = 0.125;
/// Half the saturation and luminance window that a 0–1 range spans: the
/// outer edge of CalibRaw's default channel ranges.
const CHANNEL_WINDOW: f32 = 0.5;

/// The swatches of the `rdf:Seq` property `swatches`. `variances` is the
/// matching variance list, and `owner` names where the swatches are, such
/// as " of the mask “Sky”", for warnings.
pub(super) fn read_point_colors(
    swatches: &Element,
    variances: Option<&Element>,
    owner: &str,
    warnings: &mut Warnings,
) -> PointColors {
    let variances: Vec<f32> = variances.map_or_else(Vec::new, |variances| {
        variances
            .seq_texts()
            .into_iter()
            .map(|value| parse_number(value).unwrap_or(0.0))
            .collect()
    });
    let mut colors = PointColors::default();
    for (index, raw) in swatches.seq_texts().into_iter().enumerate() {
        let Some(values) = parse_swatch(raw) else {
            warnings.push(format!("A point color{owner} could not be read."));
            continue;
        };
        if values[0] < 0.0 {
            continue;
        }
        let Some(color) = point_color(&values) else {
            warnings.push(format!("A point color{owner} could not be read."));
            continue;
        };
        if !colors.push(color) {
            warnings.push(format!(
                "Only the first {MAX_POINT_COLORS} point colors{owner} were imported."
            ));
            break;
        }
        if variances
            .get(index)
            .is_some_and(|variance| variance.abs() > 1e-6)
        {
            warnings.push(format!("Point color variance{owner} was not imported."));
        }
    }
    colors
}

fn parse_swatch(raw: &str) -> Option<[f32; SWATCH_VALUES]> {
    let values = raw
        .split(',')
        .map(parse_number)
        .collect::<Option<Vec<f32>>>()?;
    values.try_into().ok()
}

fn point_color(values: &[f32; SWATCH_VALUES]) -> Option<PointColor> {
    let [hue, saturation, value] = [values[0], values[1], values[2]];
    if hue >= 6.0 {
        return None;
    }
    let prophoto = hsv_to_rgb(hue, saturation.clamp(0.0, 1.0), value.max(0.0));
    let rec2020 = transform(LINEAR_PROPHOTO_TO_REC2020, prophoto).map(|channel| channel.max(0.0));
    // `from_srgb` takes the HSL of any RGB triple; Point Color compares in
    // display-linear Rec.2020.
    let mut color = PointColor::from_srgb(rec2020);
    color.hue_shift = values[3] * HUE_SHIFT_TURNS * CALIBRAW_HUE_SHIFT_PER_TURN;
    color.saturation_shift = values[4] * 100.0;
    // The format scales the value, CalibRaw adds to the lightness: the
    // same scale moves the lightness by the sample's own lightness.
    color.luminance_shift = values[5] * 100.0 * color.sample_hsl[2];
    color.range = values[6] * 100.0;
    color.hue_range = window(&values[7..11], HUE_WINDOW);
    color.saturation_range = window(&values[11..15], CHANNEL_WINDOW);
    color.luminance_range = window(&values[15..19], CHANNEL_WINDOW);
    Some(color.sanitized())
}

/// Maps four 0–1 positions in a window onto offsets within `±half_width`.
fn window(points: &[f32], half_width: f32) -> PointColorRange {
    let [min, inner_min, inner_max, max] =
        std::array::from_fn(|index| (2.0 * points[index].clamp(0.0, 1.0) - 1.0) * half_width);
    PointColorRange::new(min, inner_min, inner_max, max)
}

/// RGB of `hue` (sixths of a turn, 0..6), `saturation` and `value`.
fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> [f32; 3] {
    let chroma = value * saturation;
    let secondary = chroma * (1.0 - (hue.rem_euclid(2.0) - 1.0).abs());
    let [red, green, blue] = match hue as u32 {
        0 => [chroma, secondary, 0.0],
        1 => [secondary, chroma, 0.0],
        2 => [0.0, chroma, secondary],
        3 => [0.0, secondary, chroma],
        4 => [secondary, 0.0, chroma],
        _ => [chroma, 0.0, secondary],
    };
    let offset = value - chroma;
    [red + offset, green + offset, blue + offset]
}
