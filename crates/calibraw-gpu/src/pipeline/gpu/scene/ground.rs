//! B8: the ground plane of a depth result, found on the CPU when the depth is
//! uploaded (`shaders/scene/bake_common.wgsl`, `ground_plane` and
//! `ground_horizon`).
//!
//! The scene is sampled on a coarse grid as points in the camera frame at
//! relief scale 1 (`camera.wgsl`): image point `p` in shorter edges from the
//! centre at frame depth `z` lies at `(2 · focal_tan · p · z, z)`. Points
//! whose normal faces up (the depth model's normals; up is toward the top of
//! the image) are candidates. RANSAC with one point and its normal per
//! hypothesis finds the plane most of them lie on, which is then refined from
//! its inliers. Sky is never ground. A fixed seed makes the result the same on
//! every run.

use calibraw_core::pipeline::{MaskImage, SceneDepth};

/// Samples along the longer edge; the shorter edge gets as many as the
/// aspect gives.
const LONG_SAMPLES: usize = 192;
/// A candidate's normal lies within this angle of up (cosine).
const UP_COSINE: f32 = 0.707;
/// An inlier lies within this fraction of its depth of the plane, and its
/// normal within about 20° of the plane's (cosine).
const PLANE_DISTANCE: f32 = 0.05;
const PLANE_COSINE: f32 = 0.94;
const HYPOTHESES: usize = 128;
/// Hypotheses are scored on every `SCORE_STRIDE`th candidate; the winner is
/// refined on all of them.
const SCORE_STRIDE: usize = 6;
/// A ground needs at least this many inliers and this share of the samples.
const MIN_INLIERS: usize = 64;
const MIN_SHARE: f32 = 0.05;

/// The ground uniforms: the plane (unit normal facing up, offset `w` with
/// `n · X = w`; the camera lies `|w|` above it) and its horizon (`x · p.x +
/// y · p.y + z = 0` for image points `p` in shorter edges from the centre; `w`
/// is 1 when there is a ground). Without a ground both are zero.
pub(super) fn scene_ground(depth: &SceneDepth) -> ([f32; 4], [f32; 4]) {
    let camera = depth.camera();
    let disparity = depth.disparity();
    let (width, height) = (disparity.width() as f32, disparity.height() as f32);
    let short = width.min(height);
    let (columns, rows) = if width >= height {
        (
            LONG_SAMPLES,
            ((LONG_SAMPLES as f32 * height / width).round() as usize).max(1),
        )
    } else {
        (
            ((LONG_SAMPLES as f32 * width / height).round() as usize).max(1),
            LONG_SAMPLES,
        )
    };

    let mut points = Vec::new();
    let mut total = 0usize;
    for row in 0..rows {
        for column in 0..columns {
            let u = (column as f32 + 0.5) / columns as f32;
            let v = (row as f32 + 0.5) / rows as f32;
            total += 1;
            if bilinear_mask(depth.sky(), u, v) >= 0.5 {
                continue;
            }
            let normal = normal_at(depth, u, v);
            if -normal[1] < UP_COSINE {
                continue;
            }
            let i = bilinear_disparity(depth, u, v);
            let z = camera.frame_depth(i);
            let p = [(u - 0.5) * width / short, (v - 0.5) * height / short];
            let lateral = 2.0 * camera.focal_tan * z;
            points.push(GroundPoint {
                position: [p[0] * lateral, p[1] * lateral, z],
                normal,
            });
        }
    }

    let none = ([0.0; 4], [0.0; 4]);
    if points.len() < MIN_INLIERS {
        return none;
    }
    let mut seed = 0x9E37_79B9_u32;
    let mut best: Option<(usize, [f32; 3], f32)> = None;
    for _ in 0..HYPOTHESES {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let pick = &points[(seed >> 8) as usize % points.len()];
        let offset = dot(pick.normal, pick.position);
        let score = points
            .iter()
            .step_by(SCORE_STRIDE)
            .filter(|point| point.fits(pick.normal, offset))
            .count();
        if best.is_none_or(|(best_score, ..)| score > best_score) {
            best = Some((score, pick.normal, offset));
        }
    }
    let Some((_, normal, offset)) = best else {
        return none;
    };

    // Refined: the inliers' mean normal, and their mean offset along it.
    let inliers: Vec<&GroundPoint> = points
        .iter()
        .filter(|point| point.fits(normal, offset))
        .collect();
    let needed = MIN_INLIERS.max((MIN_SHARE * total as f32).ceil() as usize);
    if inliers.len() < needed {
        return none;
    }
    let sum = inliers.iter().fold([0.0f32; 3], |sum, point| {
        [
            sum[0] + point.normal[0],
            sum[1] + point.normal[1],
            sum[2] + point.normal[2],
        ]
    });
    let length = dot(sum, sum).sqrt();
    if length.is_nan() || length <= 0.0 {
        return none;
    }
    let normal = sum.map(|component| component / length);
    let offset = inliers
        .iter()
        .map(|point| dot(normal, point.position))
        .sum::<f32>()
        / inliers.len() as f32;
    let tan2 = 2.0 * camera.focal_tan;
    (
        [normal[0], normal[1], normal[2], offset],
        [tan2 * normal[0], tan2 * normal[1], normal[2], 1.0],
    )
}

struct GroundPoint {
    position: [f32; 3],
    normal: [f32; 3],
}

impl GroundPoint {
    fn fits(&self, normal: [f32; 3], offset: f32) -> bool {
        (dot(normal, self.position) - offset).abs() < PLANE_DISTANCE * self.position[2]
            && dot(normal, self.normal) > PLANE_COSINE
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Bilinear position in an image of `width` × `height` texels at full-image
/// `(u, v)`: the two texels on each axis and the weight of the second.
fn axis(t: f32, extent: u32) -> (usize, usize, f32) {
    let position = (t * extent as f32 - 0.5).clamp(0.0, (extent - 1) as f32);
    let low = position.floor() as usize;
    (
        low,
        (low + 1).min(extent as usize - 1),
        position - low as f32,
    )
}

fn bilinear(width: u32, height: u32, u: f32, v: f32, at: impl Fn(usize, usize) -> f32) -> f32 {
    let (x0, x1, fx) = axis(u, width);
    let (y0, y1, fy) = axis(v, height);
    let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * fx;
    let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * fx;
    top + (bottom - top) * fy
}

fn bilinear_disparity(depth: &SceneDepth, u: f32, v: f32) -> f32 {
    let map = depth.disparity();
    let width = map.width() as usize;
    bilinear(map.width(), map.height(), u, v, |x, y| {
        calibraw_core::pipeline::DisparityMap::decode(map.values()[y * width + x])
    })
}

fn bilinear_mask(image: &MaskImage, u: f32, v: f32) -> f32 {
    let width = image.width as usize;
    bilinear(image.width, image.height, u, v, |x, y| {
        f32::from(image.pixels[y * width + x]) / 255.0
    })
}

/// The depth model's unit normal at `(u, v)`, facing the camera
/// (`SceneNormals`).
fn normal_at(depth: &SceneDepth, u: f32, v: f32) -> [f32; 3] {
    let normals = depth.normals();
    let component = |image: &MaskImage| {
        let width = image.width as usize;
        bilinear(image.width, image.height, u, v, |x, y| {
            (f32::from(image.pixels[y * width + x]) - 128.0) / 127.0
        })
        .clamp(-1.0, 1.0)
    };
    let (x, y) = (component(normals.x()), component(normals.y()));
    let z = -(1.0 - x * x - y * y).max(0.0).sqrt();
    let length = (x * x + y * y + z * z).sqrt().max(1e-6);
    [x / length, y / length, z / length]
}
