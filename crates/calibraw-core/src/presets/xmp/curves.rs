//! Tone curves of XMP presets: point curves, the parametric curve, and
//! fitting a curve into CalibRaw's point limit.
//!
//! CalibRaw has no parametric curve, so it is applied to the RGB point curve
//! instead: the importer samples the parametric curve followed by the point
//! curve and keeps the points that best reproduce the result. The parametric
//! curve is a smooth approximation of the format's own: each region slider
//! lifts or lowers its region with a bump that fades out at the centres of
//! the neighbouring regions, and black and white stay fixed.

use super::document::{parse_number, Element, Properties, Warnings};
use crate::pipeline::{PointCurve, MAX_POINT_CURVE_POINTS};

const PARAMETRIC_AMOUNT_KEYS: [&str; 4] = [
    "ParametricShadows",
    "ParametricDarks",
    "ParametricLights",
    "ParametricHighlights",
];
/// The region splits, in percent of the input, with their defaults.
const PARAMETRIC_SPLIT_KEYS: [(&str, f32); 3] = [
    ("ParametricShadowSplit", 25.0),
    ("ParametricMidtoneSplit", 50.0),
    ("ParametricHighlightSplit", 75.0),
];
/// Narrowest region the splits leave, as a share of the input.
const MIN_REGION_WIDTH: f32 = 0.05;
/// How far a region's centre moves at ±100, as a share of the region's width.
const PARAMETRIC_REACH: f32 = 0.5;
/// Input steps at which a composed curve is sampled before fitting.
const COMPOSE_STEPS: usize = 256;
/// Output error below which fitting stops adding points.
const FIT_TOLERANCE: f32 = 1.0 / 1024.0;

/// Reads the point curve property `element` as sorted 0–1 points. An
/// unreadable curve is reported as `what`.
pub(super) fn read_points(
    element: &Element,
    what: &str,
    warnings: &mut Warnings,
) -> Option<Vec<[f32; 2]>> {
    let points = parse_points(&element.seq_texts(), warnings);
    if points.is_none() {
        warnings.push(format!("{what} could not be read; it was left linear."));
    }
    points
}

/// Parses "x, y" points in 0–255 into sorted 0–1 points with distinct
/// inputs. Needs at least two.
fn parse_points(raw_points: &[&str], warnings: &mut Warnings) -> Option<Vec<[f32; 2]>> {
    let mut points = Vec::with_capacity(raw_points.len());
    for raw in raw_points {
        let point = raw
            .split_once(',')
            .and_then(|(x, y)| Some([parse_number(x)?, parse_number(y)?]));
        let Some([x, y]) = point else {
            warnings.push(format!("The tone curve point “{raw}” could not be read."));
            continue;
        };
        if !(0.0..=255.0).contains(&x) || !(0.0..=255.0).contains(&y) {
            warnings.push(format!(
                "The tone curve point “{raw}” is outside the 0–255 range and was skipped."
            ));
            continue;
        }
        points.push([x / 255.0, y / 255.0]);
    }
    points.sort_by(|left, right| left[0].total_cmp(&right[0]));
    points.dedup_by(|left, right| (left[0] - right[0]).abs() < f32::EPSILON);
    (points.len() >= 2).then_some(points)
}

/// The curve through `points`. A curve with more points than CalibRaw
/// stores keeps the points that best reproduce it.
pub(super) fn point_curve(points: &[[f32; 2]], warnings: &mut Warnings) -> PointCurve {
    if points.len() <= MAX_POINT_CURVE_POINTS {
        return curve_from_points(points);
    }
    let fitted = fit_points(points, MAX_POINT_CURVE_POINTS);
    warnings.push(format!(
        "The tone curve has {} points; it was simplified to {}.",
        points.len(),
        fitted.len()
    ));
    curve_from_points(&fitted)
}

/// The RGB curve that applies `parametric` and then the point curve through
/// `points`, fitted into CalibRaw's point limit.
pub(super) fn compose(parametric: &ParametricCurve, points: Option<&[[f32; 2]]>) -> PointCurve {
    // Opposing neighbouring sliders can fold the parametric curve over;
    // holding its highest output so far keeps it rising, as the format does.
    let mut parametric_level = 0.0_f32;
    let samples: Vec<[f32; 2]> = (0..=COMPOSE_STEPS)
        .map(|step| {
            let x = step as f32 / COMPOSE_STEPS as f32;
            parametric_level = parametric_level.max(parametric.value(x));
            let y = points.map_or(parametric_level, |points| {
                interpolate(points, parametric_level)
            });
            [x, y]
        })
        .collect();
    curve_from_points(&fit_points(&samples, MAX_POINT_CURVE_POINTS))
}

/// The four-region parametric tone curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ParametricCurve {
    /// Shadows, darks, lights and highlights, -100..100.
    amounts: [f32; 4],
    /// The three region boundaries, ascending within 0..1.
    splits: [f32; 3],
}

impl ParametricCurve {
    /// The parametric curve stored in `properties`, or `None` when it leaves
    /// every tone unchanged.
    pub(super) fn from_properties(
        properties: &Properties<'_>,
        warnings: &mut Warnings,
    ) -> Option<Self> {
        let amounts = PARAMETRIC_AMOUNT_KEYS.map(|key| {
            properties
                .number(key, warnings)
                .unwrap_or(0.0)
                .clamp(-100.0, 100.0)
        });
        if amounts.iter().all(|amount| amount.abs() <= 1e-6) {
            return None;
        }
        let [shadow, midtone, highlight] = PARAMETRIC_SPLIT_KEYS
            .map(|(key, default)| properties.number(key, warnings).unwrap_or(default) / 100.0);
        let shadow = shadow.clamp(MIN_REGION_WIDTH, 1.0 - 3.0 * MIN_REGION_WIDTH);
        let midtone = midtone.clamp(shadow + MIN_REGION_WIDTH, 1.0 - 2.0 * MIN_REGION_WIDTH);
        let highlight = highlight.clamp(midtone + MIN_REGION_WIDTH, 1.0 - MIN_REGION_WIDTH);
        Some(Self {
            amounts,
            splits: [shadow, midtone, highlight],
        })
    }

    /// The output at `x`, both 0–1.
    fn value(&self, x: f32) -> f32 {
        let [shadow, midtone, highlight] = self.splits;
        let bounds = [0.0, shadow, midtone, highlight, 1.0];
        let centres: [f32; 4] =
            std::array::from_fn(|region| (bounds[region] + bounds[region + 1]) * 0.5);
        let mut y = x;
        for (region, amount) in self.amounts.into_iter().enumerate() {
            let rise_from = if region == 0 {
                0.0
            } else {
                centres[region - 1]
            };
            let fall_to = centres.get(region + 1).copied().unwrap_or(1.0);
            let centre = centres[region];
            let weight = if x < centre {
                smoothstep(rise_from, centre, x)
            } else {
                1.0 - smoothstep(centre, fall_to, x)
            };
            let width = bounds[region + 1] - bounds[region];
            y += amount / 100.0 * PARAMETRIC_REACH * width * weight;
        }
        y.clamp(0.0, 1.0)
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The shape-preserving cubic through `points` (sorted, at least two, with
/// distinct inputs) at `x`, constant beyond the end points. Tangents are the
/// harmonic mean of the neighbouring slopes, so the curve never overshoots
/// a point (Fritsch–Butland).
fn interpolate(points: &[[f32; 2]], x: f32) -> f32 {
    let last = points.len() - 1;
    if x <= points[0][0] {
        return points[0][1];
    }
    if x >= points[last][0] {
        return points[last][1];
    }
    let secant = |index: usize| {
        let [x0, y0] = points[index];
        let [x1, y1] = points[index + 1];
        (y1 - y0) / (x1 - x0)
    };
    let tangent = |index: usize| {
        if index == 0 {
            return secant(0);
        }
        if index == last {
            return secant(last - 1);
        }
        let (before, after) = (secant(index - 1), secant(index));
        if before * after <= 0.0 {
            0.0
        } else {
            2.0 * before * after / (before + after)
        }
    };
    let segment = points
        .windows(2)
        .position(|pair| x <= pair[1][0])
        .unwrap_or(last - 1);
    let [x0, y0] = points[segment];
    let [x1, y1] = points[segment + 1];
    let width = x1 - x0;
    let t = (x - x0) / width;
    let (t2, t3) = (t * t, t * t * t);
    (2.0 * t3 - 3.0 * t2 + 1.0) * y0
        + (t3 - 2.0 * t2 + t) * width * tangent(segment)
        + (-2.0 * t3 + 3.0 * t2) * y1
        + (t3 - t2) * width * tangent(segment + 1)
}

/// At most `max_points` of `points` (sorted, at least two), always keeping
/// both ends. Points are added where the polyline through the kept points
/// misses the others most, until it is within [`FIT_TOLERANCE`].
fn fit_points(points: &[[f32; 2]], max_points: usize) -> Vec<[f32; 2]> {
    let mut kept = vec![0, points.len() - 1];
    while kept.len() < max_points {
        let worst = kept
            .windows(2)
            .flat_map(|pair| {
                let (start, end) = (pair[0], pair[1]);
                (start + 1..end).map(move |index| (index, chord_error(points, start, end, index)))
            })
            .max_by(|left, right| left.1.total_cmp(&right.1));
        match worst {
            Some((index, error)) if error > FIT_TOLERANCE => {
                let at = kept.partition_point(|&kept| kept < index);
                kept.insert(at, index);
            }
            _ => break,
        }
    }
    kept.into_iter().map(|index| points[index]).collect()
}

/// How far `points[index]` lies from the chord between `points[start]` and
/// `points[end]`, measured along the output.
fn chord_error(points: &[[f32; 2]], start: usize, end: usize, index: usize) -> f32 {
    let [x0, y0] = points[start];
    let [x1, y1] = points[end];
    let [x, y] = points[index];
    let on_chord = if x1 - x0 <= f32::EPSILON {
        y0
    } else {
        y0 + (y1 - y0) * (x - x0) / (x1 - x0)
    };
    (y - on_chord).abs()
}

fn curve_from_points(points: &[[f32; 2]]) -> PointCurve {
    let mut curve = PointCurve::linear();
    curve.points = [[1.0, 1.0]; MAX_POINT_CURVE_POINTS];
    let len = points.len().min(MAX_POINT_CURVE_POINTS);
    curve.points[..len].copy_from_slice(&points[..len]);
    curve.len = len as u32;
    curve.sanitize();
    curve
}
