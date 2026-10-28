//! Keeping the crop inside the transformed source.
//!
//! In frame pixels the corrected source is a convex quadrilateral, and the crop is a
//! parallelogram (`A⁻¹` of the output rectangle) about its centre. A crop of a given size fits
//! wherever its four corners lie on the inner side of all four source edges, so the centres it
//! fits at are the intersection of four half-planes: a convex polygon. Without perspective the
//! edges are axis-aligned and the polygon is a rectangle.

use super::mapping::GeometryMapping;

/// Distance, frame pixels, by which each source edge is relaxed when looking for centres, so a
/// crop that fits exactly is still found.
const CENTER_TOLERANCE: f64 = 0.5e-4;
/// Distance, source pixels, by which a fitting crop may cross the source's edge.
const FIT_TOLERANCE: f64 = 1e-3;

/// A convex polygon of crop centres, frame pixels, with the interior on the left of each edge
/// in y-down coordinates.
#[derive(Clone, Debug)]
pub(super) struct CropCenters {
    vertices: Vec<[f64; 2]>,
}

impl CropCenters {
    /// The point of the polygon nearest to `point`.
    pub(super) fn nearest(&self, point: [f64; 2]) -> [f64; 2] {
        let edges = || {
            let count = self.vertices.len();
            (0..count).map(move |index| (self.vertices[index], self.vertices[(index + 1) % count]))
        };
        if self.vertices.len() >= 3 && edges().all(|(a, b)| cross(sub(b, a), sub(point, a)) >= 0.0)
        {
            return point;
        }
        edges()
            .map(|(a, b)| closest_on_segment(point, a, b))
            .min_by(|left, right| {
                distance_squared(point, *left).total_cmp(&distance_squared(point, *right))
            })
            .unwrap_or(point)
    }

    /// The part of the polygon within the axis-aligned box from `min` to `max`.
    pub(super) fn within(self, min: [f64; 2], max: [f64; 2]) -> Option<Self> {
        [
            ([1.0, 0.0], min[0]),
            ([-1.0, 0.0], -max[0]),
            ([0.0, 1.0], min[1]),
            ([0.0, -1.0], -max[1]),
        ]
        .into_iter()
        .try_fold(self.vertices, |vertices, (normal, bound)| {
            let clipped = clip(&vertices, normal, bound);
            (!clipped.is_empty()).then_some(clipped)
        })
        .map(|vertices| Self { vertices })
    }
}

/// The centres, frame pixels, at which a crop of `size` (output pixels before quarter turns)
/// lies inside the corrected source, or `None` when it fits nowhere.
pub(super) fn feasible_crop_centers(
    mapping: &GeometryMapping,
    size: [f64; 2],
) -> Option<CropCenters> {
    let corners = crop_corner_offsets(mapping, size);
    let source = mapping.frame_corners();
    let min = [0, 1].map(|axis| {
        source
            .iter()
            .map(|point| point[axis])
            .fold(f64::INFINITY, f64::min)
    });
    let max = [0, 1].map(|axis| {
        source
            .iter()
            .map(|point| point[axis])
            .fold(f64::NEG_INFINITY, f64::max)
    });
    let mut vertices = vec![
        [min[0] - 1.0, min[1] - 1.0],
        [max[0] + 1.0, min[1] - 1.0],
        [max[0] + 1.0, max[1] + 1.0],
        [min[0] - 1.0, max[1] + 1.0],
    ];
    for index in 0..4 {
        let a = source[index];
        let edge = sub(source[(index + 1) % 4], a);
        let length = edge[0].hypot(edge[1]);
        if length <= 0.0 {
            return None;
        }
        // Inward unit normal; a centre `c` fits this edge when every corner `c + k` has
        // `normal · (c + k) >= normal · a`.
        let normal = [-edge[1] / length, edge[0] / length];
        let reach = corners
            .iter()
            .map(|corner| dot(normal, *corner))
            .fold(f64::INFINITY, f64::min);
        vertices = clip(&vertices, normal, dot(normal, a) - reach - CENTER_TOLERANCE);
        if vertices.is_empty() {
            return None;
        }
    }
    Some(CropCenters { vertices })
}

/// Whether the mapping's crop lies inside the transformed source.
pub(super) fn crop_fits(mapping: &GeometryMapping) -> bool {
    let [width, height] = mapping.source_size();
    mapping.crop_source_corners().into_iter().all(|[x, y]| {
        x >= -FIT_TOLERANCE
            && x <= width + FIT_TOLERANCE
            && y >= -FIT_TOLERANCE
            && y <= height + FIT_TOLERANCE
    })
}

/// A stored crop (normalized to the frame) with `center` (frame pixels) and `size`.
pub(super) fn crop_around(center: [f64; 2], size: [f64; 2], frame: [f64; 2]) -> [f32; 4] {
    [
        (center[0] - size[0] * 0.5) / frame[0],
        (center[1] - size[1] * 0.5) / frame[1],
        (center[0] + size[0] * 0.5) / frame[0],
        (center[1] + size[1] * 0.5) / frame[1],
    ]
    .map(|value| value as f32)
}

/// The area of the simple polygon `vertices`.
pub(super) fn polygon_area(vertices: &[[f64; 2]]) -> f64 {
    let count = vertices.len();
    let twice: f64 = (0..count)
        .map(|index| cross(vertices[index], vertices[(index + 1) % count]))
        .sum();
    0.5 * twice.abs()
}

/// The crop's corners relative to its centre, frame pixels.
fn crop_corner_offsets(mapping: &GeometryMapping, size: [f64; 2]) -> [[f64; 2]; 4] {
    let [half_width, half_height] = size.map(|value| value * 0.5);
    [
        [-half_width, -half_height],
        [half_width, -half_height],
        [half_width, half_height],
        [-half_width, half_height],
    ]
    .map(|corner| mapping.unrotate(corner))
}

/// The part of the convex polygon `vertices` where `normal · p >= bound`.
fn clip(vertices: &[[f64; 2]], normal: [f64; 2], bound: f64) -> Vec<[f64; 2]> {
    let mut clipped = Vec::with_capacity(vertices.len() + 1);
    let Some(&last) = vertices.last() else {
        return clipped;
    };
    let mut previous = last;
    let mut previous_distance = dot(normal, previous) - bound;
    for &current in vertices {
        let distance = dot(normal, current) - bound;
        if (distance >= 0.0) != (previous_distance >= 0.0) {
            let t = previous_distance / (previous_distance - distance);
            clipped.push([
                previous[0] + (current[0] - previous[0]) * t,
                previous[1] + (current[1] - previous[1]) * t,
            ]);
        }
        if distance >= 0.0 {
            clipped.push(current);
        }
        previous = current;
        previous_distance = distance;
    }
    clipped
}

fn closest_on_segment(point: [f64; 2], a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let edge = sub(b, a);
    let length_squared = dot(edge, edge);
    if length_squared <= 0.0 {
        return a;
    }
    let t = (dot(sub(point, a), edge) / length_squared).clamp(0.0, 1.0);
    [a[0] + edge[0] * t, a[1] + edge[1] * t]
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn distance_squared(a: [f64; 2], b: [f64; 2]) -> f64 {
    let delta = sub(a, b);
    dot(delta, delta)
}
