//! Projecting source-image positions to the screen through lens and geometry maps.

use super::*;

pub(in crate::ui::preview) fn shortest_angle_delta(from: f32, to: f32) -> f32 {
    let mut delta = to - from;
    while delta > std::f32::consts::PI {
        delta -= std::f32::consts::TAU;
    }
    while delta < -std::f32::consts::PI {
        delta += std::f32::consts::TAU;
    }
    delta
}

pub(in crate::ui::preview) fn source_angle_from(
    center: [f32; 2],
    point: [f32; 2],
    source_width: u32,
    source_height: u32,
) -> f32 {
    let dx = (point[0] - center[0]) * source_width.max(1) as f32;
    let dy = (point[1] - center[1]) * source_height.max(1) as f32;
    dy.atan2(dx)
}

/// Projects normalized source-image coordinates onto the screen: optional lens
/// correction, then the output geometry (crop, rotation, perspective), then
/// the on-screen image rectangle.
#[derive(Clone, Copy)]
pub(crate) struct SourceProjection<'a> {
    pub(in crate::ui::preview) image_rect: Rect,
    pub(in crate::ui::preview) geometry: GeometryTransform,
    /// Maps native sensor coordinates to lens-corrected ones. `None` when the
    /// projected coordinates are already corrected.
    pub(in crate::ui::preview) lens: Option<&'a LensGeometryMap>,
    pub(in crate::ui::preview) source_width: u32,
    pub(in crate::ui::preview) source_height: u32,
}

impl<'a> SourceProjection<'a> {
    pub(crate) fn new(
        image_rect: Rect,
        geometry: GeometryTransform,
        lens: Option<&'a LensGeometryMap>,
        source_width: u32,
        source_height: u32,
    ) -> Self {
        Self {
            image_rect,
            geometry,
            lens,
            source_width,
            source_height,
        }
    }

    /// The same projection for coordinates that are already lens-corrected.
    pub(in crate::ui::preview) fn without_lens(self) -> Self {
        Self { lens: None, ..self }
    }

    pub(crate) fn to_screen(self, source_uv: [f32; 2]) -> Pos2 {
        let corrected_uv = self.lens.map_or(source_uv, |lens| {
            native_source_to_corrected_uv(lens, self.source_width, self.source_height, source_uv)
        });
        final_geometry_source_to_screen(
            self.image_rect,
            self.geometry,
            self.source_width,
            self.source_height,
            corrected_uv,
        )
    }

    pub(crate) fn to_source(self, screen: Pos2) -> [f32; 2] {
        let corrected_uv = final_geometry_screen_to_source(
            self.image_rect,
            self.geometry,
            self.source_width,
            self.source_height,
            screen,
        );
        corrected_uv_to_native_source(
            corrected_uv,
            self.lens,
            self.source_width,
            self.source_height,
        )
    }

    /// Source-space bounding box of the on-screen `visible_rect`, sampled
    /// densely when lens correction makes the mapping nonlinear.
    pub(in crate::ui::preview) fn visible_source_uv(
        self,
        visible_rect: Rect,
    ) -> crate::app::PreviewUvRect {
        source_uv_bbox(
            visible_rect_sample_points(visible_rect, self.lens.is_some())
                .into_iter()
                .map(|point| self.to_source(point)),
        )
    }

    /// Screen radius of a brush of relative `size` (a fraction of the shorter
    /// source edge), measured along the more stretched axis.
    pub(in crate::ui::preview) fn brush_radius(self, center: [f32; 2], size: f32) -> f32 {
        let radius_source_pixels =
            size.max(0.0) * self.source_width.min(self.source_height).max(1) as f32;
        let center_screen = self.to_screen(center);
        let x_screen = self.to_screen([
            center[0] + radius_source_pixels / self.source_width.max(1) as f32,
            center[1],
        ]);
        let y_screen = self.to_screen([
            center[0],
            center[1] + radius_source_pixels / self.source_height.max(1) as f32,
        ]);
        center_screen
            .distance(x_screen)
            .max(center_screen.distance(y_screen))
    }

    pub(in crate::ui::preview) fn brush_outline(
        self,
        center: [f32; 2],
        size: f32,
        segments: usize,
    ) -> Vec<Pos2> {
        let width = self.source_width.max(1) as f32;
        let height = self.source_height.max(1) as f32;
        let radius = size.max(0.0) * self.source_width.min(self.source_height).max(1) as f32;
        let center_px = [center[0] * width, center[1] * height];
        let segments = segments.max(16);
        (0..=segments)
            .map(|index| {
                let angle = std::f32::consts::TAU * index as f32 / segments as f32;
                self.to_screen([
                    (center_px[0] + radius * angle.cos()) / width,
                    (center_px[1] + radius * angle.sin()) / height,
                ])
            })
            .collect()
    }

    /// The straight source segment from `start` to `end`, sampled so that
    /// lens correction can bend it on screen.
    pub(in crate::ui::preview) fn linear_axis(
        self,
        start: [f32; 2],
        end: [f32; 2],
        segments: usize,
    ) -> Vec<Pos2> {
        let segments = segments.max(2);
        (0..=segments)
            .map(|index| {
                let t = index as f32 / segments as f32;
                self.to_screen([
                    start[0] + (end[0] - start[0]) * t,
                    start[1] + (end[1] - start[1]) * t,
                ])
            })
            .collect()
    }

    /// The line perpendicular to `start`..`end` at fraction `t` along it,
    /// clipped to the source image.
    pub(in crate::ui::preview) fn linear_isoline(
        self,
        start: [f32; 2],
        end: [f32; 2],
        t: f32,
        segments: usize,
    ) -> Vec<Pos2> {
        let width = self.source_width.max(1) as f32;
        let height = self.source_height.max(1) as f32;
        let start_px = [start[0] * width, start[1] * height];
        let delta = [(end[0] - start[0]) * width, (end[1] - start[1]) * height];
        let center = [start_px[0] + delta[0] * t, start_px[1] + delta[1] * t];
        let perpendicular = [-delta[1], delta[0]];
        if perpendicular[0].abs().max(perpendicular[1].abs()) <= 1e-6 {
            return vec![self.to_screen(start)];
        }
        let Some((q0, q1)) =
            clip_infinite_source_line(center, perpendicular, self.source_width, self.source_height)
        else {
            return Vec::new();
        };
        let segments = segments.max(2);
        (0..=segments)
            .map(|index| {
                let fraction = index as f32 / segments as f32;
                let q = q0 + (q1 - q0) * fraction;
                self.to_screen([
                    (center[0] + perpendicular[0] * q) / width,
                    (center[1] + perpendicular[1] * q) / height,
                ])
            })
            .collect()
    }

    /// The midpoint of `start`..`end` and the rotation handle offset
    /// perpendicular to it on screen.
    pub(in crate::ui::preview) fn linear_rotation_handle(
        self,
        start: [f32; 2],
        end: [f32; 2],
    ) -> (Pos2, Pos2) {
        let along = |t: f32| {
            [
                start[0] + (end[0] - start[0]) * t,
                start[1] + (end[1] - start[1]) * t,
            ]
        };
        let midpoint = self.to_screen(along(0.5));
        let tangent = self.to_screen(along(0.52)) - self.to_screen(along(0.48));
        let normal = if tangent.length_sq() > 1e-6 {
            egui::vec2(-tangent.y, tangent.x) / tangent.length()
        } else {
            egui::vec2(0.0, -1.0)
        };
        (midpoint, midpoint + normal * 34.0)
    }

    /// Screen positions of the ellipse's +major, -major, +minor and -minor
    /// axis handles.
    pub(in crate::ui::preview) fn radial_handles(
        self,
        center: [f32; 2],
        radius: [f32; 2],
        rotation: f32,
    ) -> [Pos2; 4] {
        [
            0.0,
            std::f32::consts::PI,
            std::f32::consts::FRAC_PI_2,
            -std::f32::consts::FRAC_PI_2,
        ]
        .map(|angle| self.to_screen(self.radial_point(center, radius, rotation, angle)))
    }

    pub(in crate::ui::preview) fn radial_rotation_handle(
        self,
        center: [f32; 2],
        radius: [f32; 2],
        rotation: f32,
    ) -> Pos2 {
        let center_screen = self.to_screen(center);
        let major_screen = self.radial_handles(center, radius, rotation)[0];
        let direction = (major_screen - center_screen).normalized();
        major_screen + direction * 30.0
    }

    pub(in crate::ui::preview) fn radial_outline(
        self,
        center: [f32; 2],
        radius: [f32; 2],
        rotation: f32,
        segments: usize,
    ) -> Vec<Pos2> {
        let segments = segments.max(12);
        (0..=segments)
            .map(|index| {
                let angle = std::f32::consts::TAU * index as f32 / segments as f32;
                self.to_screen(self.radial_point(center, radius, rotation, angle))
            })
            .collect()
    }

    fn radial_point(
        self,
        center: [f32; 2],
        radius: [f32; 2],
        rotation: f32,
        angle: f32,
    ) -> [f32; 2] {
        radial_source_uv_at(
            center,
            radius,
            rotation,
            angle,
            self.source_width,
            self.source_height,
        )
    }
}

pub(in crate::ui::preview) fn clip_infinite_source_line(
    point: [f32; 2],
    direction: [f32; 2],
    source_width: u32,
    source_height: u32,
) -> Option<(f32, f32)> {
    let bounds = [source_width.max(1) as f32, source_height.max(1) as f32];
    let mut lo = f32::NEG_INFINITY;
    let mut hi = f32::INFINITY;
    for axis in 0..2 {
        let p = point[axis];
        let d = direction[axis];
        if d.abs() <= 1e-8 {
            if p < 0.0 || p > bounds[axis] {
                return None;
            }
            continue;
        }
        let a = (0.0 - p) / d;
        let b = (bounds[axis] - p) / d;
        lo = lo.max(a.min(b));
        hi = hi.min(a.max(b));
        if lo > hi {
            return None;
        }
    }
    (lo.is_finite() && hi.is_finite()).then_some((lo, hi))
}

pub(in crate::ui::preview) fn radial_source_uv_at(
    center: [f32; 2],
    radius: [f32; 2],
    rotation: f32,
    angle: f32,
    source_width: u32,
    source_height: u32,
) -> [f32; 2] {
    let width = source_width.max(1) as f32;
    let height = source_height.max(1) as f32;
    let local_x = radius[0] * width * angle.cos();
    let local_y = radius[1] * height * angle.sin();
    let cos_r = rotation.cos();
    let sin_r = rotation.sin();
    let dx = cos_r * local_x - sin_r * local_y;
    let dy = sin_r * local_x + cos_r * local_y;
    [center[0] + dx / width, center[1] + dy / height]
}

pub(in crate::ui::preview) fn distance_to_segment(point: Pos2, start: Pos2, end: Pos2) -> f32 {
    let segment = end - start;
    let length_sq = segment.length_sq();
    if length_sq <= f32::EPSILON {
        return point.distance(start);
    }
    let t = ((point - start).dot(segment) / length_sq).clamp(0.0, 1.0);
    point.distance(start + segment * t)
}

pub(in crate::ui::preview) fn distance_to_polyline(point: Pos2, points: &[Pos2]) -> f32 {
    match points {
        [] => f32::INFINITY,
        [only] => point.distance(*only),
        _ => points
            .windows(2)
            .map(|pair| distance_to_segment(point, pair[0], pair[1]))
            .fold(f32::INFINITY, f32::min),
    }
}
