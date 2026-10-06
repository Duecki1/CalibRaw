//! Automatic levelling: estimates the `rotation_degrees` that makes the photo's dominant
//! straight edges horizontal or vertical.

use super::GeometryTransform;

mod luminance;
mod segments;
pub use luminance::LineAnalysisImage;
use segments::{detect_line_segments_multiscale, LineSegment};

/// Segments tilted further than this from both axes are treated as diagonal scene content.
const MAX_TILT_DEGREES: f32 = 20.0;
/// Shortest segment considered, as a share of the raster's longest edge.
const MIN_SEGMENT_FRACTION: f32 = 0.03;
/// Evidence needed for a level photo's estimate, as a share of the longest edge: the combined
/// length of agreeing structures, each scaled by its orientation weight. A level vertical counts
/// fully; a horizontal needs to be much longer.
const MIN_EVIDENCE_FRACTION: f32 = 0.12;
/// Correction at which the required evidence doubles. Most photos are within a few degrees of
/// level, while edges receding in depth (rails, wires, rooflines) sit at arbitrary angles, so a
/// large correction needs proportionally more agreement.
const TILT_DOUBLE_EVIDENCE_DEGREES: f32 = 5.5;
/// Tilt at which a vote's weight halves when locating the peak. This is deliberately gentle: it
/// only breaks ties between clusters, so that a photo rolled by 6° still finds its verticals and
/// impost lines instead of a larger but scattered set of receding courses at 14°.
const TILT_HALF_WEIGHT_DEGREES: f32 = 12.0;
/// A clear estimate needs at least one straight edge this long among its supporters, as a share
/// of the longest edge. A few short segments (grass, twigs, brickwork) agree by accident.
const MIN_ANCHOR_FRACTION: f32 = 0.10;
/// Supporters that make a long anchor unnecessary: this many independent structures agreeing
/// within a fraction of a degree is not a coincidence, as in a street of windows and doors.
const MIN_CORROBORATING_LINES: usize = 16;
/// Parallel segments closer than this share of the longest edge, and overlapping along their
/// length, are one structure: two sides of a pole, a set of rails, a grating or light trails.
const STRUCTURE_DISTANCE_FRACTION: f32 = 0.03;
/// Largest angle between segments of one structure.
const STRUCTURE_PARALLEL_DEGREES: f32 = 3.0;
/// Floor for a segment's angular uncertainty; shorter segments use about one pixel of rise.
const MIN_ANGLE_SIGMA_DEGREES: f32 = 0.15;
/// Relative weight of near-horizontal edges. Verticals stay nearly vertical under the usual
/// small camera pitch, but horizontals converge under yaw, which is common and often large.
const HORIZONTAL_WEIGHT: f32 = 0.35;
const DENSITY_STEP_DEGREES: f32 = 0.02;

/// A suggested absolute straighten angle for the current flips and perspective.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StraightenEstimate {
    /// Replacement for [`GeometryTransform::rotation_degrees`], within ±`MAX_TILT_DEGREES`.
    pub rotation_degrees: f32,
    /// Number of detected segments that agree with the estimate.
    pub supporting_lines: usize,
}

/// One segment's vote: the rotation that would level it, how sure that angle is and how much
/// it counts.
struct Vote {
    rotation: f32,
    sigma: f32,
    weight: f32,
    /// Segment length in raster pixels; see `MIN_ANCHOR_FRACTION`.
    length: f32,
    /// Length scaled by orientation weight; see `MIN_EVIDENCE_FRACTION`.
    evidence: f32,
}

/// Estimates the straighten angle for `image`, or `None` when it has no clear horizontal or
/// vertical edges.
///
/// The result replaces the current rotation, so it does not depend on it. Flips and the
/// horizontal/vertical perspective of `geometry` are applied to the detected edges first,
/// because they change edge angles before the rotation does; quarter turns do not change which
/// angles are level.
pub fn estimate_straighten_rotation(
    image: &LineAnalysisImage,
    geometry: GeometryTransform,
) -> Option<StraightenEstimate> {
    let longest = image.width.max(image.height) as f32;
    let segments = distinct_structures(
        detect_line_segments_multiscale(
            image.width,
            image.height,
            &image.values,
            longest * MIN_SEGMENT_FRACTION,
        ),
        longest * STRUCTURE_DISTANCE_FRACTION,
    );
    let votes = votes(&segments, image.width, image.height, geometry.sanitized());
    let peak = density_peak(&votes)?;

    let mut longest_supporting = 0.0f32;
    let mut supporting_lines = 0;
    let mut evidence = 0.0;
    let mut weighted_sum = 0.0;
    let mut weight_sum = 0.0;
    for vote in &votes {
        if (vote.rotation - peak).abs() <= (2.0 * vote.sigma).max(0.5) {
            let weight = vote.weight / (vote.sigma * vote.sigma);
            weighted_sum += weight * vote.rotation;
            weight_sum += weight;
            evidence += vote.evidence;
            supporting_lines += 1;
            longest_supporting = longest_supporting.max(vote.length);
        }
    }
    if weight_sum <= 0.0 {
        return None;
    }
    let rotation_degrees = (weighted_sum / weight_sum).clamp(-MAX_TILT_DEGREES, MAX_TILT_DEGREES);
    let correction = rotation_degrees / TILT_DOUBLE_EVIDENCE_DEGREES;
    let required = longest * MIN_EVIDENCE_FRACTION * (1.0 + correction * correction);
    let anchored = longest_supporting >= longest * MIN_ANCHOR_FRACTION
        || supporting_lines >= MIN_CORROBORATING_LINES;
    if evidence < required || !anchored {
        return None;
    }
    Some(StraightenEstimate {
        rotation_degrees,
        supporting_lines,
    })
}

/// Keeps the longest segment of each structure, so that repeated texture (gratings, rails,
/// light trails, both edges of a bar) votes once instead of once per line.
fn distinct_structures(mut segments: Vec<LineSegment>, max_distance: f32) -> Vec<LineSegment> {
    segments.sort_by(|a, b| b.length().total_cmp(&a.length()));
    let max_angle = STRUCTURE_PARALLEL_DEGREES.to_radians();
    let mut kept: Vec<LineSegment> = Vec::with_capacity(segments.len());
    for segment in segments {
        let [dx, dy] = segment.delta();
        let length = segment.length();
        let midpoint = segment.midpoint();
        let duplicate = kept.iter().any(|structure| {
            let [sx, sy] = structure.delta();
            let structure_length = structure.length();
            // |sin| of the angle between the lines, independent of direction.
            let sin = (sx * dy - sy * dx).abs() / (structure_length * length);
            if sin > max_angle.sin() {
                return false;
            }
            let centre = structure.midpoint();
            let (ox, oy) = (midpoint[0] - centre[0], midpoint[1] - centre[1]);
            let across = (sx * oy - sy * ox).abs() / structure_length;
            let along = (sx * ox + sy * oy).abs() / structure_length;
            across <= max_distance && along <= (structure_length + length) * 0.5
        });
        if !duplicate {
            kept.push(segment);
        }
    }
    kept
}

/// Converts segments into votes in the space the rotation acts on.
///
/// The output displacement is `R(rotation) · S · F · source` (see `GeometryInverseMap`), with
/// `S` the perspective shear and `F` the flips, so an edge at angle φ after `S · F` is level when
/// the rotation is −φ modulo 90°.
fn votes(
    segments: &[LineSegment],
    width: usize,
    height: usize,
    geometry: GeometryTransform,
) -> Vec<Vote> {
    let fx = if geometry.flip_horizontal { -1.0 } else { 1.0 };
    let fy = if geometry.flip_vertical { -1.0 } else { 1.0 };
    let shx = geometry.horizontal_transform.to_radians().tan();
    let shy = geometry.vertical_transform.to_radians().tan();
    let transform = |[x, y]: [f32; 2]| [fx * x + shx * fy * y, shy * fx * x + fy * y];
    let centre = [width as f32 * 0.5, height as f32 * 0.5];

    segments
        .iter()
        .filter_map(|segment| {
            let [dx, dy] = transform(segment.delta());
            let length = dx.hypot(dy);
            if length <= 0.0 {
                return None;
            }
            let angle = dy.atan2(dx).to_degrees();
            let quarter = (angle / 90.0).round();
            let tilt = angle - quarter * 90.0;
            if tilt.abs() > MAX_TILT_DEGREES {
                return None;
            }
            let vertical = quarter as i32 % 2 != 0;

            // Pitch makes verticals converge and yaw makes horizontals converge, but a line
            // through the image centre keeps its roll-only angle under both. Lines far from the
            // centre therefore count less.
            let midpoint = segment.midpoint();
            let [mx, my] = transform([midpoint[0] - centre[0], midpoint[1] - centre[1]]);
            let offset = (mx * dy - my * dx).abs() / length;
            let half_extent = if vertical { centre[0] } else { centre[1] };
            let relative = offset / half_extent.max(1.0);
            let centrality = 1.0 / (1.0 + 3.0 * relative * relative);

            let orientation = if vertical { 1.0 } else { HORIZONTAL_WEIGHT };
            let evidence = segment.length() * orientation;
            let relative_tilt = tilt / TILT_HALF_WEIGHT_DEGREES;
            let prior = 1.0 / (1.0 + relative_tilt * relative_tilt);

            Some(Vote {
                rotation: -tilt,
                sigma: (1.0 / length)
                    .atan()
                    .to_degrees()
                    .max(MIN_ANGLE_SIGMA_DEGREES),
                weight: evidence * centrality * prior,
                length: segment.length(),
                evidence,
            })
        })
        .collect()
}

/// The rotation with the highest kernel density of weighted votes.
fn density_peak(votes: &[Vote]) -> Option<f32> {
    if votes.is_empty() {
        return None;
    }
    let steps = (2.0 * MAX_TILT_DEGREES / DENSITY_STEP_DEGREES).round() as usize;
    let mut density = vec![0.0f32; steps + 1];
    let position = |step: usize| -MAX_TILT_DEGREES + step as f32 * DENSITY_STEP_DEGREES;
    for vote in votes {
        let reach = 4.0 * vote.sigma;
        let first = ((vote.rotation - reach + MAX_TILT_DEGREES) / DENSITY_STEP_DEGREES)
            .floor()
            .max(0.0) as usize;
        let last = (((vote.rotation + reach + MAX_TILT_DEGREES) / DENSITY_STEP_DEGREES).ceil()
            as usize)
            .min(steps);
        for (step, value) in density.iter_mut().enumerate().take(last + 1).skip(first) {
            let z = (position(step) - vote.rotation) / vote.sigma;
            *value += vote.weight * (-0.5 * z * z).exp();
        }
    }
    density
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .filter(|(_, &value)| value > 0.0)
        .map(|(step, _)| position(step))
}

#[cfg(test)]
mod tests;
