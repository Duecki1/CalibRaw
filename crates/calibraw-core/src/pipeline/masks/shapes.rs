//! Radial, linear and path shape rasterization and outlines.

use super::*;

pub(super) fn rasterize_radial(
    space: MaskRasterSpace,
    center: [f32; 2],
    radius: [f32; 2],
    rotation: f32,
    feather: f32,
) -> Vec<f32> {
    let [width, height] = space.raster;
    let [image_width, image_height] = space.image;
    let row_stride = width as usize;
    let mut out = vec![0.0f32; row_stride * height as usize];
    let cos_r = rotation.cos();
    let sin_r = rotation.sin();
    let rx = (radius[0].abs() * image_width.max(1) as f32).max(1.0);
    let ry = (radius[1].abs() * image_height.max(1) as f32).max(1.0);
    let inner = (1.0 - feather.clamp(0.0, 1.0) * 0.98).clamp(0.0, 0.995);

    out.par_chunks_mut(row_stride)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, value) in row.iter_mut().enumerate() {
                let [u, v] = space.shape_uv(x, y);
                let dy = (v - center[1]) * image_height.max(1) as f32;
                let dx = (u - center[0]) * image_width.max(1) as f32;
                let local_x = cos_r * dx + sin_r * dy;
                let local_y = -sin_r * dx + cos_r * dy;
                let distance = ((local_x / rx).powi(2) + (local_y / ry).powi(2)).sqrt();
                *value = 1.0 - smoothstep(inner, 1.0, distance);
            }
        });
    out
}

/// Flatten a closed freeform path into normalized source-coordinate line
/// segments. Straight points (zero-length handles) remain polygon corners;
/// non-zero handles form cubic Bézier segments.
pub fn path_outline_points(points: &[PathPoint], segments_per_curve: usize) -> Vec<[f32; 2]> {
    if points.len() < 2 {
        return points.iter().map(|point| point.position).collect();
    }
    let steps = segments_per_curve.max(1);
    let mut out = Vec::with_capacity(points.len() * steps + 1);
    for index in 0..points.len() {
        let current = points[index];
        let next = points[(index + 1) % points.len()];
        if index == 0 {
            out.push(current.position);
        }
        let p0 = current.position;
        let p1 = current.outgoing();
        let p2 = next.incoming();
        let p3 = next.position;
        for step in 1..=steps {
            let t = step as f32 / steps as f32;
            let omt = 1.0 - t;
            let omt2 = omt * omt;
            let t2 = t * t;
            out.push([
                omt2 * omt * p0[0]
                    + 3.0 * omt2 * t * p1[0]
                    + 3.0 * omt * t2 * p2[0]
                    + t2 * t * p3[0],
                omt2 * omt * p0[1]
                    + 3.0 * omt2 * t * p1[1]
                    + 3.0 * omt * t2 * p2[1]
                    + t2 * t * p3[1],
            ]);
        }
    }
    out
}

pub(super) fn rasterize_path(
    space: MaskRasterSpace,
    points: &[PathPoint],
    grow: f32,
    feather: f32,
) -> Vec<f32> {
    let [width, height] = space.raster;
    if width == 0 || height == 0 || points.len() < 3 {
        return vec![0.0; width as usize * height as usize];
    }

    // A small fixed subdivision is enough for the mask atlas while keeping
    // scanline rasterization bounded even for large paths.
    let outline = path_outline_points(points, 12);
    if outline.len() < 4 {
        return vec![0.0; width as usize * height as usize];
    }
    let edges = outline
        .windows(2)
        .map(|pair| {
            (
                [pair[0][0] * width as f32, pair[0][1] * height as f32],
                [pair[1][0] * width as f32, pair[1][1] * height as f32],
            )
        })
        .collect::<Vec<_>>();
    let row_stride = width as usize;
    let mut out = vec![0.0f32; row_stride * height as usize];
    out.par_chunks_mut(row_stride)
        .enumerate()
        .for_each(|(y, row)| {
            let py = y as f32 + 0.5;
            let mut intersections = Vec::with_capacity(edges.len() / 2 + 2);
            for &(a, b) in &edges {
                if (a[1] <= py && b[1] > py) || (b[1] <= py && a[1] > py) {
                    let t = (py - a[1]) / (b[1] - a[1]);
                    intersections.push(a[0] + (b[0] - a[0]) * t);
                }
            }
            intersections.sort_unstable_by(|a, b| a.total_cmp(b));
            for pair in intersections.chunks_exact(2) {
                let left = pair[0].min(pair[1]);
                let right = pair[0].max(pair[1]);
                let start = (left - 0.5).ceil().max(0.0) as usize;
                let end = (right - 0.5).floor().min(width.saturating_sub(1) as f32) as isize;
                if end >= start as isize {
                    row[start..=end as usize].fill(1.0);
                }
            }
        });

    if grow.abs() > 1e-5 || feather > 1e-5 {
        shape_distance_mask(&mut out, width, height, grow, feather, true, None);
    }
    out
}

pub(super) fn rasterize_linear(
    space: MaskRasterSpace,
    start: [f32; 2],
    end: [f32; 2],
    feather: f32,
) -> Vec<f32> {
    let [width, height] = space.raster;
    let [image_width, image_height] = space.image;
    let row_stride = width as usize;
    let mut out = vec![0.0f32; row_stride * height as usize];
    let sx = start[0] * image_width.max(1) as f32;
    let sy = start[1] * image_height.max(1) as f32;
    let dx = (end[0] - start[0]) * image_width.max(1) as f32;
    let dy = (end[1] - start[1]) * image_height.max(1) as f32;
    let length_sq = (dx * dx + dy * dy).max(1.0);
    let width_factor = feather.clamp(0.02, 1.0);
    let edge0 = 0.5 - 0.5 * width_factor;
    let edge1 = 0.5 + 0.5 * width_factor;

    out.par_chunks_mut(row_stride)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, value) in row.iter_mut().enumerate() {
                let [u, v] = space.shape_uv(x, y);
                let px = u * image_width.max(1) as f32;
                let py = v * image_height.max(1) as f32;
                let t = ((px - sx) * dx + (py - sy) * dy) / length_sq;
                *value = 1.0 - smoothstep(edge0, edge1, t);
            }
        });
    out
}

pub(super) fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    if edge1 <= edge0 {
        return if value < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn ellipse_outline_points(
    center: [f32; 2],
    radius: [f32; 2],
    rotation: f32,
    segments: usize,
) -> Vec<[f32; 2]> {
    let cos_r = rotation.cos();
    let sin_r = rotation.sin();
    (0..=segments.max(12))
        .map(|index| {
            let angle = TAU * index as f32 / segments.max(12) as f32;
            let x = radius[0] * angle.cos();
            let y = radius[1] * angle.sin();
            [
                center[0] + cos_r * x - sin_r * y,
                center[1] + sin_r * x + cos_r * y,
            ]
        })
        .collect()
}
