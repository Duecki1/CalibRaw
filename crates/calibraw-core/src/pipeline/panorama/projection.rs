//! Panorama projections: how world directions (x right, y down, z ahead) map onto the
//! panorama plane. Supported projections are spherical (equirectangular), cylindrical
//! and perspective (rectilinear). The formulas follow OpenCV's `warpers_inl.hpp`
//! (BSD-3-Clause). Plane coordinates are in units of the focal length, so one unit is one
//! radian across the centre for every projection.

use super::camera::Camera;
use super::math::Vec3;
use anyhow::{ensure, Result};
use std::f64::consts::{FRAC_PI_2, PI};

/// How the panorama is projected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Projection {
    /// Longitude and latitude: any field of view, including full spheres.
    Spherical,
    /// Longitude and the tangent of latitude: keeps verticals straight; any width, a
    /// limited height.
    Cylindrical,
    /// A flat picture plane: keeps every straight line straight; a limited field of view.
    Perspective,
}

/// The projection a merge asks for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProjectionChoice {
    /// Perspective while the view is narrow enough, cylindrical while it is not too tall,
    /// spherical beyond.
    #[default]
    Auto,
    Spherical,
    Cylindrical,
    Perspective,
}

/// Auto chooses Perspective while every frame edge stays within this angle of the
/// centre, horizontally and vertically: rectilinear stretching at 50° is 1/cos²(50°)
/// ≈ 2.4 times the centre's.
const PERSPECTIVE_LIMIT: f64 = 50.0 * PI / 180.0;
/// Auto chooses Cylindrical while every frame edge stays within this latitude.
const CYLINDRICAL_LIMIT: f64 = 50.0 * PI / 180.0;
/// A cylinder cannot show latitudes near the poles; beyond this the merge asks for
/// another projection.
const CYLINDRICAL_MAX_LATITUDE: f64 = 75.0 * PI / 180.0;
/// Frame edge samples per side when tracing a footprint.
const EDGE_SAMPLES: usize = 64;

impl Projection {
    /// Plane coordinates of world direction `d`, or `None` where the projection cannot
    /// show it.
    pub(super) fn forward(self, d: Vec3) -> Option<[f64; 2]> {
        let horizontal = d[0].hypot(d[2]);
        match self {
            Self::Spherical => Some([d[0].atan2(d[2]), d[1].atan2(horizontal)]),
            Self::Cylindrical => {
                (horizontal > 1e-12).then(|| [d[0].atan2(d[2]), d[1] / horizontal])
            }
            Self::Perspective => (d[2] > 1e-9).then(|| [d[0] / d[2], d[1] / d[2]]),
        }
    }

    /// The world direction at plane coordinates `p` (not normalized).
    pub(super) fn inverse(self, p: [f64; 2]) -> Vec3 {
        match self {
            Self::Spherical => {
                let (sin_u, cos_u) = p[0].sin_cos();
                let (sin_v, cos_v) = p[1].sin_cos();
                [sin_u * cos_v, sin_v, cos_u * cos_v]
            }
            Self::Cylindrical => {
                let (sin_u, cos_u) = p[0].sin_cos();
                [sin_u, p[1], cos_u]
            }
            Self::Perspective => [p[0], p[1], 1.0],
        }
    }

    /// Whether plane coordinates `p` lie inside the projection's domain.
    fn valid(self, p: [f64; 2]) -> bool {
        match self {
            Self::Spherical => p[0].abs() <= PI && p[1].abs() <= FRAC_PI_2,
            Self::Cylindrical => p[0].abs() <= PI,
            Self::Perspective => true,
        }
    }
}

impl ProjectionChoice {
    /// The projection for `cameras`: the chosen one, or Auto's pick.
    pub(super) fn resolve(self, cameras: &[Camera]) -> Projection {
        match self {
            Self::Spherical => Projection::Spherical,
            Self::Cylindrical => Projection::Cylindrical,
            Self::Perspective => Projection::Perspective,
            Self::Auto => {
                let (mut horizontal, mut vertical) = (0.0f64, 0.0f64);
                let mut behind = false;
                for camera in cameras {
                    for p in edge_points(camera) {
                        let d = camera.ray(p);
                        behind |= d[2] <= 0.0;
                        horizontal = horizontal.max(d[0].atan2(d[2]).abs());
                        vertical = vertical.max(d[1].atan2(d[0].hypot(d[2])).abs());
                    }
                }
                if !behind && horizontal <= PERSPECTIVE_LIMIT && vertical <= PERSPECTIVE_LIMIT {
                    Projection::Perspective
                } else if vertical <= CYLINDRICAL_LIMIT {
                    Projection::Cylindrical
                } else {
                    Projection::Spherical
                }
            }
        }
    }
}

/// Pixel positions along a frame's border.
fn edge_points(camera: &Camera) -> impl Iterator<Item = [f64; 2]> {
    let (w, h) = camera.pinhole.size();
    let (w, h) = (w - 1.0, h - 1.0);
    (0..EDGE_SAMPLES).flat_map(move |k| {
        let t = k as f64 / EDGE_SAMPLES as f64;
        [[t * w, 0.0], [w, t * h], [w - t * w, h], [0.0, h - t * h]]
    })
}

/// An axis-aligned rectangle on the panorama plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Extent {
    pub(super) min: [f64; 2],
    pub(super) max: [f64; 2],
}

impl Extent {
    const EMPTY: Self = Self {
        min: [f64::INFINITY; 2],
        max: [f64::NEG_INFINITY; 2],
    };

    fn include(&mut self, p: [f64; 2]) {
        for ((min, max), value) in self.min.iter_mut().zip(&mut self.max).zip(p) {
            *min = min.min(value);
            *max = max.max(value);
        }
    }

    pub(super) fn union(self, other: Self) -> Self {
        Self {
            min: [self.min[0].min(other.min[0]), self.min[1].min(other.min[1])],
            max: [self.max[0].max(other.max[0]), self.max[1].max(other.max[1])],
        }
    }

    pub(super) fn size(&self) -> [f64; 2] {
        [self.max[0] - self.min[0], self.max[1] - self.min[1]]
    }
}

/// The plane region frame `camera` covers. Fails when `projection` cannot show it.
pub(super) fn frame_extent(projection: Projection, camera: &Camera, label: &str) -> Result<Extent> {
    let mut extent = Extent::EMPTY;
    for p in edge_points(camera) {
        let uv = projection.forward(camera.ray(p));
        ensure!(
            uv.is_some(),
            "{label} is outside what the {} projection can show; choose another projection",
            projection_name(projection)
        );
        if let Some(uv) = uv {
            extent.include(uv);
        }
    }
    if projection != Projection::Perspective {
        // A frame across the back of a full circle has edge longitudes on both sides of
        // ±π: it spans the whole width.
        if extent.size()[0] > PI {
            extent.min[0] = -PI;
            extent.max[0] = PI;
        }
        for (pole, latitude) in [([0.0, -1.0, 0.0], -FRAC_PI_2), ([0.0, 1.0, 0.0], FRAC_PI_2)] {
            let sees_pole = camera
                .pixel(pole)
                .is_some_and(|p| camera.pinhole.contains(p));
            if sees_pole {
                ensure!(
                    projection == Projection::Spherical,
                    "{label} looks straight up or down, which only the Spherical projection can show"
                );
                extent.min[0] = -PI;
                extent.max[0] = PI;
                extent.include([0.0, latitude]);
            }
        }
    }
    if projection == Projection::Cylindrical {
        let limit = CYLINDRICAL_MAX_LATITUDE.tan();
        ensure!(
            extent.min[1] >= -limit && extent.max[1] <= limit,
            "The panorama is too tall for the Cylindrical projection; choose Spherical"
        );
    }
    Ok(extent)
}

fn projection_name(projection: Projection) -> &'static str {
    match projection {
        Projection::Spherical => "Spherical",
        Projection::Cylindrical => "Cylindrical",
        Projection::Perspective => "Perspective",
    }
}

/// A pixel grid on the panorama plane: pixel `(x, y)` (centres at integer coordinates)
/// shows plane position `origin + (x + ½, y + ½) / scale`.
#[derive(Clone, Copy, Debug)]
pub(super) struct Grid {
    pub(super) projection: Projection,
    pub(super) origin: [f64; 2],
    /// Pixels per plane unit.
    pub(super) scale: f64,
    pub(super) width: usize,
    pub(super) height: usize,
}

impl Grid {
    /// The grid of `scale` pixels per unit covering `extent`.
    pub(super) fn covering(projection: Projection, extent: Extent, scale: f64) -> Self {
        let size = extent.size();
        Self {
            projection,
            origin: extent.min,
            scale,
            width: ((size[0] * scale).ceil() as usize).max(1),
            height: ((size[1] * scale).ceil() as usize).max(1),
        }
    }

    pub(super) fn plane(&self, x: f64, y: f64) -> [f64; 2] {
        [
            self.origin[0] + (x + 0.5) / self.scale,
            self.origin[1] + (y + 0.5) / self.scale,
        ]
    }

    /// Fractional pixel position of plane position `p`.
    pub(super) fn position(&self, p: [f64; 2]) -> [f64; 2] {
        [
            (p[0] - self.origin[0]) * self.scale - 0.5,
            (p[1] - self.origin[1]) * self.scale - 0.5,
        ]
    }

    /// The world direction pixel `(x, y)` shows, or `None` outside the projection.
    pub(super) fn ray(&self, x: f64, y: f64) -> Option<Vec3> {
        let p = self.plane(x, y);
        self.projection.valid(p).then(|| self.projection.inverse(p))
    }

    /// The pixel rectangle `[x0, x1) × [y0, y1)` covering plane extent `extent`, clipped
    /// to the grid, or `None` when they do not meet.
    pub(super) fn pixels(&self, extent: Extent) -> Option<[usize; 4]> {
        let lo = self.position(extent.min);
        let hi = self.position(extent.max);
        let x0 = lo[0].floor().max(0.0) as usize;
        let y0 = lo[1].floor().max(0.0) as usize;
        let x1 = ((hi[0].ceil() + 1.0).max(0.0) as usize).min(self.width);
        let y1 = ((hi[1].ceil() + 1.0).max(0.0) as usize).min(self.height);
        (x0 < x1 && y0 < y1).then_some([x0, y0, x1, y1])
    }
}
