//! Automatic perspective correction: fits the vertical correction and roll that make a photo's
//! vertical edges vertical and its horizontal edges level, as it is displayed.
//!
//! The vertical correction is the displayed one ([`GeometryTransform::displayed_perspective`]):
//! a pitch, or a yaw when the photo is turned a quarter. Each detected edge is mapped through a
//! candidate correction and measured against its nearest displayed axis. A robust (Cauchy)
//! least-squares fit, refined from a coarse grid with a shrinking scale, finds both angles. The
//! correction is kept only where it makes clearly more verticals upright than levelling alone,
//! which telephoto photos, whose verticals barely converge, rarely show, and it is reduced until
//! the largest crop still shows most of the photo, as a full correction of a steep wide-angle
//! view leaves only a magnified sliver. The horizontal correction is left to the user:
//! horizontals converge towards several vanishing points wherever two surfaces meet, and a
//! wrong guess distorts the whole photo. Without convincing verticals the photo is only
//! levelled, as automatic straightening does.

use super::super::crop_fit::polygon_area;
use super::super::lines::{distinct_segments, LineAnalysisImage, LineSegment};
use super::super::mapping::{apply_2x2, multiply_2x2, skew_and_flip, GeometryMapping};
use super::super::straighten::{estimate_from_segments, MAX_TILT_DEGREES};
use super::super::{CropAspectRatio, GeometryTransform};
use super::PerspectiveCorrection;

/// Edges further than this from both axes after a correction are diagonal scene content.
const MAX_EDGE_TILT_DEGREES: f64 = 30.0;
/// Relative weight of horizontal edges in the fit: they inform the roll, but converge whenever
/// the camera was turned towards a surface, and many are not level in the scene.
const HORIZONTAL_WEIGHT: f64 = 0.35;
/// Scales of the robust loss, degrees, from the coarse grid search to the final refinement.
/// A wide one first lets edges far from their axis pull the fit; the narrow one ignores them.
const ROBUST_SCALES_DEGREES: [f64; 3] = [4.0, 2.0, 1.0];
/// Steps of the coarse search for the vertical correction and for roll, degrees.
const CORRECTION_GRID_STEP_DEGREES: f64 = 2.5;
const ROLL_GRID_STEP_DEGREES: f64 = 2.0;
/// A vertical this close to upright after the fit supports it.
const SUPPORT_DEGREES: f64 = 1.0;
/// Supporting verticals needed for a correction: their combined length as a share of the
/// longest edge, the share of the displayed width they spread over (the correction shows in
/// how verticals far apart converge), and the length of the longest of them.
const MIN_VERTICAL_EVIDENCE: f64 = 0.6;
const MIN_VERTICAL_SPREAD: f64 = 0.25;
const MIN_VERTICAL_ANCHOR: f64 = 0.10;
/// How many times more vertical support the correction must give than levelling alone.
const MIN_CORRECTION_GAIN: f64 = 1.25;
/// Share of the photo the largest crop of its own shape must still show after the correction.
const MIN_SHOWN_FRACTION: f64 = 0.7;
const MAX_ITERATIONS: usize = 30;

/// A suggested perspective correction and straighten angle for the current flips and skews.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PerspectiveEstimate {
    /// Replacement for [`GeometryTransform::perspective`]: a displayed vertical correction
    /// only, or the identity when the photo only needs levelling.
    pub perspective: PerspectiveCorrection,
    /// Replacement for [`GeometryTransform::rotation_degrees`], within ±20°.
    pub rotation_degrees: f32,
}

/// Estimates the correction for `image` with a camera of `focal_length_35mm`, or `None` when
/// the photo has no clear vertical or horizontal edges.
///
/// The result replaces the current perspective and rotation, so it depends on neither. The flips
/// and skews of `geometry` are applied to the edges first, because they change edge angles
/// before the rotation does, and its quarter turns decide which edges are displayed vertical.
pub fn estimate_perspective(
    image: &LineAnalysisImage,
    geometry: GeometryTransform,
    focal_length_35mm: f32,
) -> Option<PerspectiveEstimate> {
    // The aspect stretch keeps lines upright and is the user's to choose, so the estimate
    // leaves it out.
    let geometry = GeometryTransform {
        perspective: PerspectiveCorrection::default(),
        aspect_stretch: 0.0,
        ..geometry.sanitized()
    };
    let (width, height) = (image.width(), image.height());
    let segments = distinct_segments(image);
    let fit = Fit::new(&segments, width, height, geometry, focal_length_35mm);

    let level = fit.refine([0.0, fit.roll_start()], [false, true]);
    let upright = fit.refine(fit.grid_start(), [true, true]);
    let support = fit.vertical_support(upright);
    if !support.is_clear(&fit)
        || support.evidence < MIN_CORRECTION_GAIN * fit.vertical_support(level).evidence
    {
        return estimate_from_segments(&segments, width, height, geometry).map(|level| {
            PerspectiveEstimate {
                perspective: PerspectiveCorrection::default(),
                rotation_degrees: level.rotation_degrees,
            }
        });
    }
    let [vertical, roll] = fit.natural(upright);
    Some(PerspectiveEstimate {
        perspective: fit.correction(vertical),
        rotation_degrees: (roll as f32).clamp(-MAX_TILT_DEGREES, MAX_TILT_DEGREES),
    })
}

/// An edge in raster pixels.
struct Edge {
    start: [f32; 2],
    end: [f32; 2],
    /// Where its midpoint lies across the displayed width, raster pixels.
    across: f64,
    length: f64,
}

/// The fitting problem: `[vertical, roll]` in degrees, the displayed vertical correction and
/// the straighten rotation.
struct Fit {
    edges: Vec<Edge>,
    width: usize,
    height: usize,
    /// The raster's extent across the displayed width.
    across: f64,
    longest: f64,
    geometry: GeometryTransform,
    focal_length_35mm: f32,
    skew_flip: [[f64; 2]; 2],
    /// Added to every edge angle: the quarter turns, so edges are classified as displayed.
    turn_degrees: f64,
}

/// Where an edge lies after a correction.
#[derive(Clone, Copy)]
struct Tilt {
    /// Angle from the nearest axis, degrees.
    degrees: f64,
    vertical: bool,
}

/// The verticals that are upright after a correction.
struct VerticalSupport {
    /// Their combined length, raster pixels.
    evidence: f64,
    /// The range of their midpoints across the width, raster pixels.
    spread: f64,
    longest: f64,
}

impl VerticalSupport {
    fn is_clear(&self, fit: &Fit) -> bool {
        self.evidence >= MIN_VERTICAL_EVIDENCE * fit.longest
            && self.spread >= MIN_VERTICAL_SPREAD * fit.across
            && self.longest >= MIN_VERTICAL_ANCHOR * fit.longest
    }
}

impl Fit {
    fn new(
        segments: &[LineSegment],
        width: usize,
        height: usize,
        geometry: GeometryTransform,
        focal_length_35mm: f32,
    ) -> Self {
        let [shear, flip] = skew_and_flip(geometry);
        let turned = geometry.quarter_turns % 2 == 1;
        Self {
            edges: segments
                .iter()
                .map(|segment| Edge {
                    start: segment.start,
                    end: segment.end,
                    across: f64::from(segment.midpoint()[usize::from(turned)]),
                    length: f64::from(segment.length()),
                })
                .collect(),
            width,
            height,
            across: if turned { height } else { width } as f64,
            longest: width.max(height) as f64,
            geometry,
            focal_length_35mm,
            skew_flip: multiply_2x2(shear, flip),
            turn_degrees: 90.0 * f64::from(geometry.quarter_turns % 4),
        }
    }

    /// The displayed vertical correction `vertical`, degrees.
    fn correction(&self, vertical: f64) -> PerspectiveCorrection {
        let mut geometry = self.geometry;
        geometry.perspective.focal_length_35mm = self.focal_length_35mm;
        geometry.set_displayed_perspective([vertical as f32, 0.0]);
        geometry.perspective.sanitized()
    }

    /// Every edge's direction as displayed after the vertical correction `vertical`, before
    /// the roll, degrees; `None` where it has none.
    fn angles(&self, vertical: f64) -> Vec<Option<f64>> {
        let homography = self
            .correction(vertical)
            .homography([self.width as f64, self.height as f64]);
        self.edges
            .iter()
            .map(|edge| {
                let start = homography.apply(edge.start);
                let end = homography.apply(edge.end);
                let [dx, dy] = apply_2x2(
                    self.skew_flip,
                    [f64::from(end[0] - start[0]), f64::from(end[1] - start[1])],
                );
                (dx.is_finite() && dy.is_finite() && dx.hypot(dy) > 0.0)
                    .then(|| dy.atan2(dx).to_degrees() + self.turn_degrees)
            })
            .collect()
    }

    fn weight(&self, edge: &Edge, tilt: Tilt) -> f64 {
        let orientation = if tilt.vertical {
            1.0
        } else {
            HORIZONTAL_WEIGHT
        };
        edge.length / self.longest * orientation
    }

    /// The robust cost of `angles` rolled by `roll` at `scale` degrees. Diagonal edges cost as
    /// much as the furthest edge that counts, so an edge leaving the window does not lower it.
    fn cost_of(&self, angles: &[Option<f64>], roll: f64, scale: f64) -> f64 {
        let loss = |degrees: f64| (degrees / scale).powi(2).ln_1p();
        angles
            .iter()
            .zip(&self.edges)
            .map(
                |(angle, edge)| match angle.and_then(|angle| tilt(angle, roll)) {
                    Some(tilt) => self.weight(edge, tilt) * loss(tilt.degrees),
                    None => {
                        edge.length / self.longest * HORIZONTAL_WEIGHT * loss(MAX_EDGE_TILT_DEGREES)
                    }
                },
            )
            .sum()
    }

    fn cost(&self, [vertical, roll]: [f64; 2], scale: f64) -> f64 {
        self.cost_of(&self.angles(vertical), roll, scale)
    }

    /// The best vertical correction and roll on a coarse grid.
    fn grid_start(&self) -> [f64; 2] {
        grid(
            PerspectiveCorrection::MAX_DEGREES.into(),
            CORRECTION_GRID_STEP_DEGREES,
        )
        .flat_map(|vertical| {
            let angles = self.angles(vertical);
            grid(MAX_TILT_DEGREES.into(), ROLL_GRID_STEP_DEGREES)
                .map(|roll| {
                    let cost = self.cost_of(&angles, roll, ROBUST_SCALES_DEGREES[0]);
                    ([vertical, roll], cost)
                })
                .collect::<Vec<_>>()
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or([0.0; 2], |(params, _)| params)
    }

    /// The best roll on a coarse grid, without a correction.
    fn roll_start(&self) -> f64 {
        let angles = self.angles(0.0);
        grid(MAX_TILT_DEGREES.into(), ROLL_GRID_STEP_DEGREES)
            .map(|roll| (roll, self.cost_of(&angles, roll, ROBUST_SCALES_DEGREES[0])))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map_or(0.0, |(roll, _)| roll)
    }

    /// Refines the parameters marked `free` from `start`, narrowing the robust scale.
    fn refine(&self, start: [f64; 2], free: [bool; 2]) -> [f64; 2] {
        ROBUST_SCALES_DEGREES
            .into_iter()
            .fold(start, |params, scale| {
                self.levenberg_marquardt(params, free, scale)
            })
    }

    /// Minimizes the robust cost with Levenberg–Marquardt steps on iteratively reweighted
    /// residuals (each edge's tilt), differentiated numerically in the vertical correction; a
    /// tilt changes one-for-one with the roll.
    fn levenberg_marquardt(&self, mut params: [f64; 2], free: [bool; 2], scale: f64) -> [f64; 2] {
        const STEP_DEGREES: f64 = 1e-3;
        let indices: Vec<usize> = (0..2).filter(|&index| free[index]).collect();
        let size = indices.len();
        let mut cost = self.cost(params, scale);
        let mut damping = 1e-3;
        for _ in 0..MAX_ITERATIONS {
            let angles = self.angles(params[0]);
            // d(tilt)/d(vertical) per edge, when the correction is free.
            let vertical_derivatives: Option<Vec<Option<f64>>> = free[0].then(|| {
                self.angles(params[0] + STEP_DEGREES)
                    .into_iter()
                    .zip(self.angles(params[0] - STEP_DEGREES))
                    .map(|(plus, minus)| {
                        let difference = (plus? - minus? + 180.0).rem_euclid(360.0) - 180.0;
                        Some(difference / (2.0 * STEP_DEGREES))
                    })
                    .collect()
            });

            let mut normal = [[0.0f64; 2]; 2];
            let mut gradient = [0.0f64; 2];
            for (index, (angle, edge)) in angles.iter().zip(&self.edges).enumerate() {
                let Some(tilt) = angle.and_then(|angle| tilt(angle, params[1])) else {
                    continue;
                };
                let Some(row) = indices
                    .iter()
                    .map(|&parameter| match parameter {
                        0 => vertical_derivatives
                            .as_ref()
                            .and_then(|column| column[index]),
                        _ => Some(1.0),
                    })
                    .collect::<Option<Vec<f64>>>()
                else {
                    continue;
                };
                let weight = self.weight(edge, tilt) / (1.0 + (tilt.degrees / scale).powi(2));
                for a in 0..size {
                    gradient[a] += weight * row[a] * tilt.degrees;
                    for b in 0..size {
                        normal[a][b] += weight * row[a] * row[b];
                    }
                }
            }

            let mut improved = false;
            while damping < 1e8 {
                let mut damped = normal;
                for (index, row) in damped.iter_mut().enumerate().take(size) {
                    row[index] += damping * normal[index][index].max(1e-12);
                }
                let Some(step) = solve(damped, gradient, size) else {
                    damping *= 10.0;
                    continue;
                };
                let mut candidate = params;
                for (slot, &index) in indices.iter().enumerate() {
                    candidate[index] -= step[slot];
                }
                candidate = clamp_params(candidate);
                let candidate_cost = self.cost(candidate, scale);
                if candidate_cost < cost {
                    let moved = (0..2)
                        .map(|index| (candidate[index] - params[index]).abs())
                        .fold(0.0, f64::max);
                    params = candidate;
                    cost = candidate_cost;
                    damping = (damping * 0.3).max(1e-9);
                    improved = moved >= 1e-4;
                    break;
                }
                damping *= 4.0;
            }
            if !improved {
                break;
            }
        }
        params
    }

    /// The verticals within `SUPPORT_DEGREES` of upright after `[vertical, roll]`.
    fn vertical_support(&self, [vertical, roll]: [f64; 2]) -> VerticalSupport {
        let mut support = VerticalSupport {
            evidence: 0.0,
            spread: 0.0,
            longest: 0.0,
        };
        let mut range = [f64::INFINITY, f64::NEG_INFINITY];
        for (angle, edge) in self.angles(vertical).iter().zip(&self.edges) {
            if angle
                .and_then(|angle| tilt(angle, roll))
                .is_some_and(|tilt| tilt.vertical && tilt.degrees.abs() <= SUPPORT_DEGREES)
            {
                support.evidence += edge.length;
                support.longest = support.longest.max(edge.length);
                range = [range[0].min(edge.across), range[1].max(edge.across)];
            }
        }
        support.spread = (range[1] - range[0]).max(0.0);
        support
    }

    /// `[vertical, roll]` with the correction reduced until the largest crop of the photo's
    /// own shape shows at least `MIN_SHOWN_FRACTION` of it.
    fn natural(&self, [vertical, roll]: [f64; 2]) -> [f64; 2] {
        if self.shown_fraction(vertical, roll) >= MIN_SHOWN_FRACTION {
            return [vertical, roll];
        }
        let mut low = 0.0;
        let mut high = 1.0;
        for _ in 0..16 {
            let mid = 0.5 * (low + high);
            if self.shown_fraction(vertical * mid, roll) >= MIN_SHOWN_FRACTION {
                low = mid;
            } else {
                high = mid;
            }
        }
        [vertical * low, roll]
    }

    /// The share of the photo the largest crop of its own shape shows after the vertical
    /// correction `vertical` and `roll`.
    fn shown_fraction(&self, vertical: f64, roll: f64) -> f64 {
        let (width, height) = (self.width as u32, self.height as u32);
        let mut geometry = GeometryTransform {
            crop: GeometryTransform::default().crop,
            aspect_ratio: CropAspectRatio::Original,
            rotation_degrees: roll as f32,
            perspective: self.correction(vertical),
            ..self.geometry
        };
        geometry.conform_crop_to_aspect(width, height);
        geometry.fit_crop_inside_transformed_source(width, height);
        let corners = GeometryMapping::new(geometry, width, height).crop_source_corners();
        polygon_area(&corners) / (self.width * self.height) as f64
    }
}

/// Where an edge at `angle` (degrees) lies after rolling by `roll`, or `None` when it is
/// diagonal.
fn tilt(angle: f64, roll: f64) -> Option<Tilt> {
    let angle = angle + roll;
    let quarter = (angle / 90.0).round();
    let degrees = angle - quarter * 90.0;
    (degrees.abs() <= MAX_EDGE_TILT_DEGREES).then_some(Tilt {
        degrees,
        vertical: (quarter as i64).rem_euclid(2) == 1,
    })
}

fn clamp_params([vertical, roll]: [f64; 2]) -> [f64; 2] {
    let max = f64::from(PerspectiveCorrection::MAX_DEGREES);
    let max_roll = f64::from(MAX_TILT_DEGREES);
    [vertical.clamp(-max, max), roll.clamp(-max_roll, max_roll)]
}

/// `-limit..=limit` in steps of `step`.
fn grid(limit: f64, step: f64) -> impl Iterator<Item = f64> + Clone {
    let steps = (limit / step).floor() as i32;
    (-steps..=steps).map(move |index| f64::from(index) * step)
}

/// Solves the leading `size`×`size` block of `matrix · x = rhs` by Gaussian elimination with
/// partial pivoting; `None` when it is singular.
fn solve(mut matrix: [[f64; 2]; 2], mut rhs: [f64; 2], size: usize) -> Option<[f64; 2]> {
    for column in 0..size {
        let pivot = (column..size)
            .max_by(|&a, &b| matrix[a][column].abs().total_cmp(&matrix[b][column].abs()))?;
        if matrix[pivot][column].abs() < 1e-15 {
            return None;
        }
        matrix.swap(column, pivot);
        rhs.swap(column, pivot);
        let pivot_row = matrix[column];
        for row in column + 1..size {
            let factor = matrix[row][column] / pivot_row[column];
            for (value, pivot_value) in matrix[row][column..size]
                .iter_mut()
                .zip(&pivot_row[column..size])
            {
                *value -= factor * pivot_value;
            }
            rhs[row] -= factor * rhs[column];
        }
    }
    let mut solution = [0.0; 2];
    for row in (0..size).rev() {
        let known: f64 = (row + 1..size).map(|k| matrix[row][k] * solution[k]).sum();
        solution[row] = (rhs[row] - known) / matrix[row][row];
    }
    Some(solution)
}
