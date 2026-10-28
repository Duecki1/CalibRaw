//! What the automatic and guided corrections share: how a line of the source is displayed under
//! a candidate correction, and the damped least-squares fit of the corrections and the roll.

use super::super::mapping::{apply_2x2, multiply_2x2, skew_and_flip};
use super::super::GeometryTransform;
use super::PerspectiveCorrection;

const MAX_ITERATIONS: usize = 30;
/// Step of the numerical derivatives in a displayed correction, degrees.
const STEP_DEGREES: f64 = 1e-3;

/// How lines of a source are displayed under candidate corrections: through the flips, skews and
/// quarter turns of a geometry, with corrections made at one focal length.
pub(super) struct LineView {
    size: [f64; 2],
    geometry: GeometryTransform,
    focal_length_35mm: f32,
    skew_flip: [[f64; 2]; 2],
    /// Added to every line angle: the quarter turns, so lines are measured as displayed.
    turn_degrees: f64,
}

impl LineView {
    /// The view of a `width` × `height` source through `geometry`, whose perspective correction
    /// and rotation are replaced by the candidates.
    pub(super) fn new(
        geometry: GeometryTransform,
        width: usize,
        height: usize,
        focal_length_35mm: f32,
    ) -> Self {
        let [shear, flip] = skew_and_flip(geometry);
        Self {
            size: [width as f64, height as f64],
            geometry,
            focal_length_35mm,
            skew_flip: multiply_2x2(shear, flip),
            turn_degrees: 90.0 * f64::from(geometry.quarter_turns % 4),
        }
    }

    pub(super) fn geometry(&self) -> GeometryTransform {
        self.geometry
    }

    /// The displayed correction `[vertical, horizontal]`, degrees.
    pub(super) fn correction(&self, displayed: [f64; 2]) -> PerspectiveCorrection {
        let mut geometry = self.geometry;
        geometry.perspective.focal_length_35mm = self.focal_length_35mm;
        geometry.set_displayed_perspective(displayed.map(|degrees| degrees as f32));
        geometry.perspective.sanitized()
    }

    /// The direction of each line `[start, end]` (source pixels) as displayed after the
    /// correction `displayed`, before the roll, degrees; `None` where it has none.
    pub(super) fn angles(
        &self,
        displayed: [f64; 2],
        lines: impl IntoIterator<Item = [[f32; 2]; 2]>,
    ) -> Vec<Option<f64>> {
        let homography = self.correction(displayed).homography(self.size);
        lines
            .into_iter()
            .map(|[start, end]| {
                let start = homography.apply(start);
                let end = homography.apply(end);
                let [dx, dy] = apply_2x2(
                    self.skew_flip,
                    [f64::from(end[0] - start[0]), f64::from(end[1] - start[1])],
                );
                (dx.is_finite() && dy.is_finite() && dx.hypot(dy) > 0.0)
                    .then(|| dy.atan2(dx).to_degrees() + self.turn_degrees)
            })
            .collect()
    }

    /// How each line's angle changes with the displayed correction's component `axis` (0 for
    /// vertical, 1 for horizontal) at `displayed`, per degree; `None` where it has no angle.
    pub(super) fn angle_derivatives(
        &self,
        displayed: [f64; 2],
        axis: usize,
        lines: impl IntoIterator<Item = [[f32; 2]; 2]> + Clone,
    ) -> Vec<Option<f64>> {
        let mut plus = displayed;
        let mut minus = displayed;
        plus[axis] += STEP_DEGREES;
        minus[axis] -= STEP_DEGREES;
        self.angles(plus, lines.clone())
            .into_iter()
            .zip(self.angles(minus, lines))
            .map(|(plus, minus)| {
                let difference = (plus? - minus? + 180.0).rem_euclid(360.0) - 180.0;
                Some(difference / (2.0 * STEP_DEGREES))
            })
            .collect()
    }
}

/// A residual linearized at a point: its weight, its value and its derivatives in each
/// parameter.
pub(super) struct Linearized<const N: usize> {
    pub(super) weight: f64,
    pub(super) value: f64,
    pub(super) derivatives: [f64; N],
}

/// Minimizes `cost` over the parameters marked `free`, starting from `params`, with
/// Levenberg–Marquardt steps on the weighted residuals `linearize` gives at each point; each
/// step is limited by `clamp`. Weights may depend on the point, which makes the fit an
/// iteratively reweighted one.
pub(super) fn levenberg_marquardt<const N: usize>(
    mut params: [f64; N],
    free: [bool; N],
    linearize: impl Fn([f64; N]) -> Vec<Linearized<N>>,
    cost: impl Fn([f64; N]) -> f64,
    clamp: impl Fn([f64; N]) -> [f64; N],
) -> [f64; N] {
    let indices: Vec<usize> = (0..N).filter(|&index| free[index]).collect();
    let size = indices.len();
    let mut current = cost(params);
    let mut damping = 1e-3;
    for _ in 0..MAX_ITERATIONS {
        let mut normal = [[0.0f64; N]; N];
        let mut gradient = [0.0f64; N];
        for residual in linearize(params) {
            for a in 0..size {
                let derivative = residual.derivatives[indices[a]];
                gradient[a] += residual.weight * derivative * residual.value;
                for b in 0..size {
                    normal[a][b] += residual.weight * derivative * residual.derivatives[indices[b]];
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
            candidate = clamp(candidate);
            let candidate_cost = cost(candidate);
            if candidate_cost < current {
                let moved = (0..N)
                    .map(|index| (candidate[index] - params[index]).abs())
                    .fold(0.0, f64::max);
                params = candidate;
                current = candidate_cost;
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

/// Solves the leading `size`×`size` block of `matrix · x = rhs` by Gaussian elimination with
/// partial pivoting; `None` when it is singular.
fn solve<const N: usize>(
    mut matrix: [[f64; N]; N],
    mut rhs: [f64; N],
    size: usize,
) -> Option<[f64; N]> {
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
    let mut solution = [0.0; N];
    for row in (0..size).rev() {
        let known: f64 = (row + 1..size).map(|k| matrix[row][k] * solution[k]).sum();
        solution[row] = (rhs[row] - known) / matrix[row][row];
    }
    Some(solution)
}
