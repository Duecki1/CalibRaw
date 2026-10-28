//! The straight edges of a photo, for levelling it and correcting its perspective.

mod luminance;
mod segments;
pub use luminance::LineAnalysisImage;
use segments::detect_line_segments_multiscale;
pub(super) use segments::LineSegment;

/// Shortest segment considered, as a share of the raster's longest edge.
const MIN_SEGMENT_FRACTION: f32 = 0.03;
/// Parallel segments closer than this share of the longest edge, and overlapping along their
/// length, are one structure: two sides of a pole, a set of rails, a grating or light trails.
const STRUCTURE_DISTANCE_FRACTION: f32 = 0.03;
/// Largest angle between segments of one structure.
const STRUCTURE_PARALLEL_DEGREES: f32 = 3.0;

/// The straight edges of `image` in raster pixels, one per structure.
pub(super) fn distinct_segments(image: &LineAnalysisImage) -> Vec<LineSegment> {
    let longest = image.width.max(image.height) as f32;
    distinct_structures(
        detect_line_segments_multiscale(
            image.width,
            image.height,
            &image.values,
            longest * MIN_SEGMENT_FRACTION,
        ),
        longest * STRUCTURE_DISTANCE_FRACTION,
    )
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
