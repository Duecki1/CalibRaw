//! `GeometryInverseMap`: output pixel to source position through the geometry and lens maps.

use super::*;

#[derive(Clone, Copy, Debug)]
pub struct GeometryInverseMap<'a> {
    /// Output pixel to lens-corrected source pixel.
    source_from_output: Homography,
    projective: bool,
    lens_geometry: Option<&'a LensGeometryMap>,
    source_width: u32,
    source_height: u32,
    output_width: u32,
    output_height: u32,
}

impl<'a> GeometryInverseMap<'a> {
    pub fn new_with_lens(
        geometry: GeometryTransform,
        lens_geometry: Option<&'a LensGeometryMap>,
        source_width: u32,
        source_height: u32,
        output_width: u32,
        output_height: u32,
    ) -> Self {
        let geometry = geometry.sanitized();
        Self {
            source_from_output: GeometryMapping::new(geometry, source_width, source_height)
                .source_pixel_from_output_pixel(output_width, output_height),
            projective: !geometry.perspective.is_identity(),
            lens_geometry,
            source_width: source_width.max(1),
            source_height: source_height.max(1),
            output_width: output_width.max(1),
            output_height: output_height.max(1),
        }
    }

    /// Whether one Jacobian holds for every output pixel: without lens correction and
    /// perspective the map is affine.
    pub fn is_affine(self) -> bool {
        self.lens_geometry.is_none() && !self.projective
    }

    pub fn source_position(self, output_x: f32, output_y: f32) -> [f32; 2] {
        let corrected = self.source_from_output.apply([output_x, output_y]);
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
