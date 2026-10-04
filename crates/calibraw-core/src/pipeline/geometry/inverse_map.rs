//! `GeometryInverseMap`: output pixel to source position through crop, rotation and lens maps.

use super::*;

#[derive(Clone, Copy, Debug)]
pub struct GeometryInverseMap<'a> {
    pub(super) geometry: GeometryTransform,
    lens_geometry: Option<&'a LensGeometryMap>,
    source_width: u32,
    source_height: u32,
    output_width: u32,
    output_height: u32,
}

impl<'a> GeometryInverseMap<'a> {
    #[cfg(test)]
    pub fn new(
        geometry: GeometryTransform,
        source_width: u32,
        source_height: u32,
        output_width: u32,
        output_height: u32,
    ) -> Self {
        Self::new_with_lens(
            geometry,
            None,
            source_width,
            source_height,
            output_width,
            output_height,
        )
    }

    pub fn new_with_lens(
        geometry: GeometryTransform,
        lens_geometry: Option<&'a LensGeometryMap>,
        source_width: u32,
        source_height: u32,
        output_width: u32,
        output_height: u32,
    ) -> Self {
        Self {
            geometry: geometry.sanitized(),
            lens_geometry,
            source_width: source_width.max(1),
            source_height: source_height.max(1),
            output_width: output_width.max(1),
            output_height: output_height.max(1),
        }
    }

    pub fn source_position(self, output_x: f32, output_y: f32) -> [f32; 2] {
        let output_u = (output_x + 0.5) / self.output_width as f32;
        let output_v = (output_y + 0.5) / self.output_height as f32;
        let corrected = self.corrected_position_normalized(output_u, output_v);
        if corrected[0] < -0.5
            || corrected[1] < -0.5
            || corrected[0] > self.source_width as f32 - 0.5
            || corrected[1] > self.source_height as f32 - 0.5
        {
            return corrected;
        }
        self.lens_geometry.map_or(corrected, |lens| {
            lens.source_position_for_raster(
                corrected[0],
                corrected[1],
                self.source_width,
                self.source_height,
            )
        })
    }

    pub fn pixel_jacobian(self) -> [[f32; 2]; 2] {
        self.pixel_jacobian_at(0.0, 0.0)
    }

    pub fn pixel_jacobian_at(self, output_x: f32, output_y: f32) -> [[f32; 2]; 2] {
        let x = output_x.clamp(0.0, self.output_width.saturating_sub(1) as f32);
        let y = output_y.clamp(0.0, self.output_height.saturating_sub(1) as f32);
        let origin = self.source_position(x, y);

        let jx = if self.output_width == 1 {
            let left = self.source_position(-0.5, y);
            let right = self.source_position(0.5, y);
            [right[0] - left[0], right[1] - left[1]]
        } else {
            let (sample_x, sign) = if x + 1.0 < self.output_width as f32 {
                (x + 1.0, 1.0)
            } else {
                (x - 1.0, -1.0)
            };
            let step = self.source_position(sample_x, y);
            [(step[0] - origin[0]) * sign, (step[1] - origin[1]) * sign]
        };
        let jy = if self.output_height == 1 {
            let top = self.source_position(x, -0.5);
            let bottom = self.source_position(x, 0.5);
            [bottom[0] - top[0], bottom[1] - top[1]]
        } else {
            let (sample_y, sign) = if y + 1.0 < self.output_height as f32 {
                (y + 1.0, 1.0)
            } else {
                (y - 1.0, -1.0)
            };
            let step = self.source_position(x, sample_y);
            [(step[0] - origin[0]) * sign, (step[1] - origin[1]) * sign]
        };
        [jx, jy]
    }

    fn corrected_position_normalized(self, output_u: f32, output_v: f32) -> [f32; 2] {
        let geometry = self.geometry;
        let crop = geometry.crop;
        let source_width = self.source_width as f32;
        let source_height = self.source_height as f32;
        let crop_width = (crop[2] - crop[0]) * source_width;
        let crop_height = (crop[3] - crop[1]) * source_height;
        let center_x = (crop[0] + crop[2]) * 0.5 * source_width;
        let center_y = (crop[1] + crop[3]) * 0.5 * source_height;

        let (u, v) = match geometry.quarter_turns % 4 {
            0 => (output_u, output_v),
            1 => (output_v, 1.0 - output_u),
            2 => (1.0 - output_u, 1.0 - output_v),
            _ => (1.0 - output_v, output_u),
        };
        let dx = (u - 0.5) * crop_width;
        let dy = (v - 0.5) * crop_height;

        let fx = if geometry.flip_horizontal { -1.0 } else { 1.0 };
        let fy = if geometry.flip_vertical { -1.0 } else { 1.0 };
        let shx = geometry.horizontal_transform.to_radians().tan();
        let shy = geometry.vertical_transform.to_radians().tan();
        let angle = geometry.rotation_degrees.to_radians();
        let c = angle.cos();
        let s = angle.sin();

        let a = c * fx - s * shy * fx;
        let b = c * shx * fy - s * fy;
        let c2 = s * fx + c * shy * fx;
        let d = s * shx * fy + c * fy;
        let determinant = a * d - b * c2;
        if determinant.abs() < 1e-6 {
            return [center_x - 0.5, center_y - 0.5];
        }
        let source_dx = (d * dx - b * dy) / determinant;
        let source_dy = (-c2 * dx + a * dy) / determinant;

        [center_x + source_dx - 0.5, center_y + source_dy - 0.5]
    }
}

pub(super) fn sanitized_crop(crop: [f32; 4]) -> [f32; 4] {
    let geometry = GeometryTransform {
        crop,
        ..Default::default()
    };
    geometry.sanitized().crop
}

pub(super) fn lerp_crop(start: [f32; 4], end: [f32; 4], t: f32) -> [f32; 4] {
    [
        start[0] + (end[0] - start[0]) * t,
        start[1] + (end[1] - start[1]) * t,
        start[2] + (end[2] - start[2]) * t,
        start[3] + (end[3] - start[3]) * t,
    ]
}
