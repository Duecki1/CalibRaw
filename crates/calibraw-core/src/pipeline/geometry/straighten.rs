//! Automatic levelling: estimates the `rotation_degrees` that makes the photo's dominant
//! straight edges horizontal or vertical.

use super::lines::{distinct_segments, LineAnalysisImage, LineSegment};
use super::mapping::{apply_2x2, multiply_2x2, skew_and_flip};
use super::GeometryTransform;

/// Segments tilted further than this from both axes are treated as diagonal scene content.
pub(super) const MAX_TILT_DEGREES: f32 = 20.0;
/// Evidence needed for a level photo's estimate, as a share of the longest edge: the combined
/// length of agreeing structures, each scaled by its orientation weight. A level vertical counts
/// fully; a horizontal needs to be much longer.
const MIN_EVIDENCE_FRACTION: f32 = 0.12;
/// Correction at which the required evidence doubles, so a large correction needs more
/// agreement than a small one.
const TILT_DOUBLE_EVIDENCE_DEGREES: f32 = 20.0;
/// Tilt at which a near-horizontal edge's evidence halves. Horizontals converge under yaw, so
/// rails, wires, rooflines and masonry courses receding in depth sit at arbitrary angles, and a
/// tilted one says little about roll. Verticals are barely affected by the usual camera pitch
/// and, since a rolled photo is common, are not penalised for tilt at all.
const HORIZONTAL_TILT_HALF_EVIDENCE_DEGREES: f32 = 6.0;
/// Angular width of the first, coarse peak search. Perspective fans parallel verticals out over
/// a few degrees, so the pillars of one wall only form a single peak at this width; the
/// estimate is then refined at each segment's own precision within that peak.
const COARSE_SIGMA_DEGREES: f32 = 1.5;
/// A clear estimate needs at least one straight edge this long among its supporters, as a share
/// of the longest edge. A few short segments (grass, twigs, brickwork) agree by accident.
const MIN_ANCHOR_FRACTION: f32 = 0.10;
/// Supporters that make a long anchor unnecessary: this many independent structures agreeing
/// within a fraction of a degree is not a coincidence, as in a street of windows and doors.
const MIN_CORROBORATING_LINES: usize = 16;
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
/// The result replaces the current rotation, so it does not depend on it. The perspective
/// correction, flips and skews of `geometry` are applied to the detected edges first, because
/// they change edge angles before the rotation does; quarter turns do not change which angles
/// are level.
pub fn estimate_straighten_rotation(
    image: &LineAnalysisImage,
    geometry: GeometryTransform,
) -> Option<StraightenEstimate> {
    estimate_from_segments(
        &distinct_segments(image),
        image.width(),
        image.height(),
        geometry,
    )
}

/// [`estimate_straighten_rotation`] for segments already found in a `width`×`height` raster.
pub(super) fn estimate_from_segments(
    segments: &[LineSegment],
    width: usize,
    height: usize,
    geometry: GeometryTransform,
) -> Option<StraightenEstimate> {
    let longest = width.max(height) as f32;
    let votes = votes(segments, width, height, geometry.sanitized());
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

/// Converts segments into votes in the space the rotation acts on.
///
/// The output displacement is `R(rotation) · S · F · G(source)` (see `GeometryMapping`), with
/// `G` the perspective correction, `S` the skews and `F` the flips, so an edge at angle φ after
/// `S · F · G` is level when the rotation is −φ modulo 90°.
fn votes(
    segments: &[LineSegment],
    width: usize,
    height: usize,
    geometry: GeometryTransform,
) -> Vec<Vote> {
    let perspective = geometry
        .perspective
        .homography([width as f64, height as f64]);
    let [shear, flip] = skew_and_flip(geometry);
    let skew_flip = multiply_2x2(shear, flip);
    let transform = |[x, y]: [f32; 2]| {
        apply_2x2(skew_flip, [f64::from(x), f64::from(y)]).map(|value| value as f32)
    };
    let centre = [width as f32 * 0.5, height as f32 * 0.5];

    segments
        .iter()
        .filter_map(|segment| {
            let start = perspective.apply(segment.start);
            let end = perspective.apply(segment.end);
            let [dx, dy] = transform([end[0] - start[0], end[1] - start[1]]);
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
            let midpoint = perspective.apply(segment.midpoint());
            let [mx, my] = transform([midpoint[0] - centre[0], midpoint[1] - centre[1]]);
            let offset = (mx * dy - my * dx).abs() / length;
            let half_extent = if vertical { centre[0] } else { centre[1] };
            let relative = offset / half_extent.max(1.0);
            let centrality = 1.0 / (1.0 + 3.0 * relative * relative);

            let orientation = if vertical { 1.0 } else { HORIZONTAL_WEIGHT };
            let relative_tilt = tilt / HORIZONTAL_TILT_HALF_EVIDENCE_DEGREES;
            let tilt_penalty = if vertical {
                1.0
            } else {
                1.0 / (1.0 + relative_tilt * relative_tilt)
            };
            let evidence = segment.length() * orientation * tilt_penalty;

            Some(Vote {
                rotation: -tilt,
                sigma: (1.0 / length)
                    .atan()
                    .to_degrees()
                    .max(MIN_ANGLE_SIGMA_DEGREES),
                // The penalty counts twice here: once in the evidence and once more so that a
                // tilted horizontal cannot pull the peak away from verticals.
                weight: evidence * centrality * tilt_penalty,
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
    let centre = peak_of(votes, COARSE_SIGMA_DEGREES, None)?;
    let reach = 2.0 * COARSE_SIGMA_DEGREES;
    peak_of(votes, 0.0, Some((centre - reach, centre + reach)))
}

/// Peak of the density with every vote's sigma at least `min_sigma`, optionally within a window.
fn peak_of(votes: &[Vote], min_sigma: f32, window: Option<(f32, f32)>) -> Option<f32> {
    let steps = (2.0 * MAX_TILT_DEGREES / DENSITY_STEP_DEGREES).round() as usize;
    let mut density = vec![0.0f32; steps + 1];
    let position = |step: usize| -MAX_TILT_DEGREES + step as f32 * DENSITY_STEP_DEGREES;
    for vote in votes {
        let sigma = vote.sigma.max(min_sigma);
        let reach = 4.0 * sigma;
        let first = ((vote.rotation - reach + MAX_TILT_DEGREES) / DENSITY_STEP_DEGREES)
            .floor()
            .max(0.0) as usize;
        let last = (((vote.rotation + reach + MAX_TILT_DEGREES) / DENSITY_STEP_DEGREES).ceil()
            as usize)
            .min(steps);
        for (step, value) in density.iter_mut().enumerate().take(last + 1).skip(first) {
            let z = (position(step) - vote.rotation) / sigma;
            *value += vote.weight * (-0.5 * z * z).exp();
        }
    }
    density
        .iter()
        .enumerate()
        .filter(|(step, _)| window.is_none_or(|(lo, hi)| (lo..=hi).contains(&position(*step))))
        .max_by(|a, b| a.1.total_cmp(b.1))
        .filter(|(_, &value)| value > 0.0)
        .map(|(step, _)| position(step))
}
