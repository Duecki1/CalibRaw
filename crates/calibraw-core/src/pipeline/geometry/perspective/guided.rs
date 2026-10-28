//! Guided perspective correction: the user draws lines along edges that should be displayed
//! vertical or horizontal, and the correction and roll that make them so are fitted.
//!
//! Each guide aims at the displayed axis it is nearer to under the current geometry, so a guide
//! keeps its aim once the correction has made it upright. The roll is fitted to every guide.
//! Two guides along one axis fix where those lines converge, so they fit the displayed
//! correction along that axis. A single guide along an axis fits it only when at least two
//! other guides fix the roll, and it is held near its current value, as a guide near the centre
//! barely turns with it. Corrections no guide fixes stay as they are, so a single guide only
//! levels the photo, as the straighten line does.

use super::super::GeometryTransform;
use super::estimate::PerspectiveEstimate;
use super::fit::{levenberg_marquardt, LineView, Linearized};
use super::PerspectiveCorrection;

/// Weight of a correction fitted to a single guide staying at its current value: the square
/// of its change, degrees, against the squared tilt of the guides.
const SINGLE_GUIDE_ANCHOR: f64 = 1e-4;

/// A line drawn along an edge of the photo that should be displayed vertical or horizontal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PerspectiveGuide {
    /// The line's ends, as positions normalized to the lens-corrected source (0–1 across it).
    pub start: [f32; 2],
    pub end: [f32; 2],
}

/// The correction and straighten rotation that display `guides` upright on a
/// `source_width` × `source_height` source, or `None` without a guide that has a direction.
///
/// The current correction and rotation of `geometry` are the starting point, and the parts of
/// the correction the guides do not fix are kept. A new correction is made at
/// `focal_length_35mm`; an existing one keeps its own focal length.
pub fn solve_guided_perspective(
    guides: &[PerspectiveGuide],
    geometry: GeometryTransform,
    source_width: u32,
    source_height: u32,
    focal_length_35mm: f32,
) -> Option<PerspectiveEstimate> {
    let geometry = geometry.sanitized();
    let focal_length_35mm = if geometry.perspective.is_identity() {
        focal_length_35mm
    } else {
        geometry.perspective.focal_length_35mm
    };
    let (width, height) = (source_width.max(1), source_height.max(1));
    let view = LineView::new(geometry, width as usize, height as usize, focal_length_35mm);
    let lines: Vec<[[f32; 2]; 2]> = guides
        .iter()
        .map(|guide| [guide.start, guide.end].map(|[u, v]| [u * width as f32, v * height as f32]))
        .collect();

    let [vertical, horizontal] = geometry.displayed_perspective().map(f64::from);
    let start = [vertical, horizontal, f64::from(geometry.rotation_degrees)];
    let aims: Vec<Option<bool>> = view
        .angles([vertical, horizontal], lines.iter().copied())
        .into_iter()
        .map(|angle| angle.map(|angle| aims_vertical(angle + start[2])))
        .collect();
    let counts =
        [true, false].map(|vertical| aims.iter().filter(|aim| **aim == Some(vertical)).count());
    let total = counts[0] + counts[1];
    if total == 0 {
        return None;
    }
    let fitted = counts.map(|count| count >= 2 || (count == 1 && total >= 3));
    let anchored = counts.map(|count| count == 1);
    let free = [fitted[0], fitted[1], true];
    // How far `params` moves the corrections anchored to their current values.
    let anchors = move |params: [f64; 3]| {
        [0, 1]
            .into_iter()
            .filter(move |&axis| fitted[axis] && anchored[axis])
            .map(move |axis| (axis, params[axis] - start[axis]))
    };

    let tilts = |[vertical, horizontal, roll]: [f64; 3]| -> Vec<Option<f64>> {
        view.angles([vertical, horizontal], lines.iter().copied())
            .into_iter()
            .zip(&aims)
            .map(|(angle, aim)| Some(tilt(angle? + roll, (*aim)?)))
            .collect()
    };
    let linearize = |params: [f64; 3]| -> Vec<Linearized<3>> {
        let displayed = [params[0], params[1]];
        let derivatives = [0, 1].map(|axis| {
            free[axis].then(|| view.angle_derivatives(displayed, axis, lines.iter().copied()))
        });
        let guides = tilts(params)
            .into_iter()
            .enumerate()
            .filter_map(|(index, tilt)| {
                let derivative = |axis: usize| match &derivatives[axis] {
                    Some(column) => column[index],
                    None => Some(0.0),
                };
                Some(Linearized {
                    weight: 1.0,
                    value: tilt?,
                    derivatives: [derivative(0)?, derivative(1)?, 1.0],
                })
            });
        let anchors = anchors(params).map(|(axis, change)| {
            let mut derivatives = [0.0; 3];
            derivatives[axis] = 1.0;
            Linearized {
                weight: SINGLE_GUIDE_ANCHOR,
                value: change,
                derivatives,
            }
        });
        guides.chain(anchors).collect()
    };
    let cost = |params: [f64; 3]| -> f64 {
        let guides: f64 = tilts(params)
            .into_iter()
            .flatten()
            .map(|tilt| tilt * tilt)
            .sum();
        let anchors: f64 = anchors(params)
            .map(|(_, change)| SINGLE_GUIDE_ANCHOR * change * change)
            .sum();
        guides + anchors
    };
    let max = f64::from(PerspectiveCorrection::MAX_DEGREES);
    let max_roll = f64::from(GeometryTransform::MAX_ROTATION_DEGREES);
    let clamp = |[vertical, horizontal, roll]: [f64; 3]| {
        [
            vertical.clamp(-max, max),
            horizontal.clamp(-max, max),
            roll.clamp(-max_roll, max_roll),
        ]
    };
    let [vertical, horizontal, roll] = levenberg_marquardt(start, free, linearize, cost, clamp);
    Some(PerspectiveEstimate {
        perspective: view.correction([vertical, horizontal]),
        rotation_degrees: roll as f32,
    })
}

/// Whether a line displayed at `angle` (degrees) is nearer to vertical than to horizontal.
fn aims_vertical(angle: f64) -> bool {
    (angle.rem_euclid(180.0) - 90.0).abs() < 45.0
}

/// How far a line displayed at `angle` (degrees) is from the axis it aims at, degrees.
fn tilt(angle: f64, vertical: bool) -> f64 {
    let target = if vertical { 90.0 } else { 0.0 };
    (angle - target + 90.0).rem_euclid(180.0) - 90.0
}
