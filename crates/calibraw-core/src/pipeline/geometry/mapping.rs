//! `GeometryMapping`: where source positions land in the corrected frame, the crop workspace and
//! the output, and `Homography`, the plane projective maps it is made of.

use super::*;

/// `w` below which a point counts as on or behind a homography's vanishing line. Valid points
/// have `w` near 1 (see [`PerspectiveCorrection`]).
const HORIZON_EPSILON: f64 = 1e-9;

/// A projective map of the plane: `[x, y, 1]` is multiplied by the rows and divided by the
/// third coordinate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Homography([[f64; 3]; 3]);

impl Homography {
    pub const IDENTITY: Self = Self([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);

    pub(super) const fn from_rows(rows: [[f64; 3]; 3]) -> Self {
        Self(rows)
    }

    /// `point ↦ linear · point + translation`.
    pub(super) const fn affine(linear: [[f64; 2]; 2], translation: [f64; 2]) -> Self {
        Self([
            [linear[0][0], linear[0][1], translation[0]],
            [linear[1][0], linear[1][1], translation[1]],
            [0.0, 0.0, 1.0],
        ])
    }

    pub(super) const fn translation(offset: [f64; 2]) -> Self {
        Self::affine([[1.0, 0.0], [0.0, 1.0]], offset)
    }

    pub(super) const fn scale(factor: [f64; 2]) -> Self {
        Self::affine([[factor[0], 0.0], [0.0, factor[1]]], [0.0, 0.0])
    }

    /// This map followed by `next`.
    pub(super) fn then(self, next: Self) -> Self {
        let mut rows = [[0.0; 3]; 3];
        for (row, out) in rows.iter_mut().enumerate() {
            for (column, value) in out.iter_mut().enumerate() {
                *value = (0..3).map(|k| next.0[row][k] * self.0[k][column]).sum();
            }
        }
        Self(rows)
    }

    /// The inverse map. Every homography built here is invertible: perspective corrections are
    /// rotations and the affine parts have a determinant of at least `1 - tan²(30°)`.
    pub fn inverse(self) -> Self {
        let m = self.0;
        let cofactor = |r0: usize, r1: usize, c0: usize, c1: usize| {
            m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0]
        };
        let adjugate = [
            [
                cofactor(1, 2, 1, 2),
                -cofactor(0, 2, 1, 2),
                cofactor(0, 1, 1, 2),
            ],
            [
                -cofactor(1, 2, 0, 2),
                cofactor(0, 2, 0, 2),
                -cofactor(0, 1, 0, 2),
            ],
            [
                cofactor(1, 2, 0, 1),
                -cofactor(0, 2, 0, 1),
                cofactor(0, 1, 0, 1),
            ],
        ];
        let determinant =
            m[0][0] * adjugate[0][0] + m[0][1] * adjugate[1][0] + m[0][2] * adjugate[2][0];
        Self(adjugate.map(|row| row.map(|value| value / determinant)))
    }

    pub fn apply(&self, [x, y]: [f32; 2]) -> [f32; 2] {
        let [x, y] = self.apply_f64([f64::from(x), f64::from(y)]);
        [x as f32, y as f32]
    }

    /// Maps `point`; a point on or behind the vanishing line maps to infinite coordinates, which
    /// every caller treats as outside the image.
    pub(super) fn apply_f64(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        let m = &self.0;
        let w = m[2][0] * x + m[2][1] * y + m[2][2];
        if w.is_nan() || w <= HORIZON_EPSILON {
            return [f64::INFINITY; 2];
        }
        [
            (m[0][0] * x + m[0][1] * y + m[0][2]) / w,
            (m[1][0] * x + m[1][1] * y + m[1][2]) / w,
        ]
    }

    pub(super) const fn rows_f64(&self) -> [[f64; 3]; 3] {
        self.0
    }

    /// Whether the map is affine: parallel lines stay parallel, so a textured triangle shows it
    /// exactly.
    pub fn is_affine(&self) -> bool {
        self.0[2] == [0.0, 0.0, 1.0]
    }

    /// The rows in single precision, for shaders.
    pub fn rows(&self) -> [[f32; 3]; 3] {
        self.0.map(|row| row.map(|value| value as f32))
    }
}

/// How a [`GeometryTransform`] places a source image of a given size.
///
/// Positions are continuous pixels of the lens-corrected source (pixel `i` covers `[i, i + 1)`).
/// The perspective correction `G` maps them into perspective space, where the corrected frame is
/// the bounding box of the corrected source. Frame pixels are perspective-space positions with
/// the frame's top-left corner at 0. A source position `p` lands in the output at
/// `Q · Σ · A · (G(p) - c)` from its centre: `A = R · S · F` rotates, skews and flips, `Σ`
/// stretches for the aspect, `Q` applies the quarter turns and `c` is the crop centre in frame
/// pixels. Without perspective `G` is the identity and the frame is the source.
///
/// The stored crop is normalized to the frame: its centre is `c` and its extent is the output
/// size before `Σ` and `Q`. The crop workspace shows the frame with `A` applied about the frame
/// centre, so the image stays put while the crop changes; the crop is axis-aligned there.
/// Workspace coordinates are normalized to the frame, before quarter turns; the canvas shows the
/// workspace after them.
///
/// `Σ` is diagonal, so it only changes sizes in pixels: positions normalized to the output, the
/// workspace or the canvas do not depend on it, and neither does where the crop fits.
#[derive(Clone, Copy, Debug)]
pub struct GeometryMapping {
    quarter_turns: u8,
    /// `G` followed by the move of the frame's corner to 0.
    frame_from_source: Homography,
    source_from_frame: Homography,
    /// `A`.
    linear: [[f64; 2]; 2],
    linear_inverse: [[f64; 2]; 2],
    /// `Σ`, as its scale of the output's width and height.
    stretch: [f64; 2],
    source_size: [f64; 2],
    frame_size: [f64; 2],
    /// Crop centre in frame pixels.
    crop_center: [f64; 2],
    /// Crop size in output pixels before `Σ` and quarter turns.
    crop_size: [f64; 2],
}

impl GeometryMapping {
    pub fn new(geometry: GeometryTransform, source_width: u32, source_height: u32) -> Self {
        let geometry = geometry.sanitized();
        let source_size = [
            f64::from(source_width.max(1)),
            f64::from(source_height.max(1)),
        ];
        let perspective = geometry.perspective.homography(source_size);
        let [min, max] =
            bounds(&rectangle_corners(source_size).map(|corner| perspective.apply_f64(corner)));
        let frame_from_source = perspective.then(Homography::translation([-min[0], -min[1]]));
        let linear = linear_part(geometry);
        let mut mapping = Self {
            quarter_turns: geometry.quarter_turns % 4,
            frame_from_source,
            source_from_frame: frame_from_source.inverse(),
            linear,
            linear_inverse: inverse_2x2(linear),
            stretch: geometry.aspect_stretch_factors(),
            source_size,
            frame_size: [max[0] - min[0], max[1] - min[1]],
            crop_center: [0.0; 2],
            crop_size: [0.0; 2],
        };
        mapping.set_crop(geometry.crop);
        mapping
    }

    /// The same mapping with another stored crop (normalized to the frame).
    pub fn with_crop(mut self, crop: [f32; 4]) -> Self {
        self.set_crop(crop);
        self
    }

    fn set_crop(&mut self, crop: [f32; 4]) {
        let crop = crop.map(f64::from);
        self.crop_center = [
            (crop[0] + crop[2]) * 0.5 * self.frame_size[0],
            (crop[1] + crop[3]) * 0.5 * self.frame_size[1],
        ];
        self.crop_size = [
            (crop[2] - crop[0]) * self.frame_size[0],
            (crop[3] - crop[1]) * self.frame_size[1],
        ];
    }

    /// Whether the mapping is affine, which it is without perspective correction.
    pub fn is_affine(&self) -> bool {
        self.frame_from_source.is_affine()
    }

    /// Size of the corrected frame in pixels, before quarter turns.
    pub fn frame_size(&self) -> [f32; 2] {
        self.frame_size.map(|value| value as f32)
    }

    /// Size of the output in pixels, after quarter turns.
    pub fn output_size(&self) -> [f32; 2] {
        self.turned(self.stretched(self.crop_size))
            .map(|value| value as f32)
    }

    /// Size of the crop canvas in pixels, after quarter turns: the frame, stretched like the
    /// output.
    pub fn canvas_size(&self) -> [f32; 2] {
        self.turned(self.stretched(self.frame_size))
            .map(|value| value as f32)
    }

    /// Normalized source position to output position, 0–1 across the cropped output after
    /// quarter turns.
    pub fn output_uv_from_source_uv(&self) -> Homography {
        Homography::scale(self.source_size)
            .then(self.frame_from_source)
            .then(self.view_from_frame(self.crop_center, self.turned(self.crop_size)))
    }

    /// Normalized source position to position on the crop canvas, 0–1 across the frame after
    /// quarter turns.
    pub fn canvas_uv_from_source_uv(&self) -> Homography {
        let center = self.frame_size.map(|value| value * 0.5);
        Homography::scale(self.source_size)
            .then(self.frame_from_source)
            .then(self.view_from_frame(center, self.turned(self.frame_size)))
    }

    /// Output pixel (top-left at 0, centres at half pixels) to source pixel (centres at whole
    /// pixels) for an output raster of the given size.
    pub fn source_pixel_from_output_pixel(
        &self,
        output_width: u32,
        output_height: u32,
    ) -> Homography {
        let output = [
            f64::from(output_width.max(1)),
            f64::from(output_height.max(1)),
        ];
        Homography::affine(
            [[1.0 / output[0], 0.0], [0.0, 1.0 / output[1]]],
            [0.5 / output[0], 0.5 / output[1]],
        )
        .then(
            self.view_from_frame(self.crop_center, self.turned(self.crop_size))
                .inverse(),
        )
        .then(self.source_from_frame)
        .then(Homography::translation([-0.5, -0.5]))
    }

    /// The crop workspace position (normalized, before quarter turns) of the canvas position
    /// `canvas_uv`.
    pub fn workspace_from_canvas(&self, canvas_uv: [f32; 2]) -> [f32; 2] {
        let [u, v] = canvas_uv.map(f64::from);
        let [x, y] = match self.quarter_turns {
            0 => [u, v],
            1 => [v, 1.0 - u],
            2 => [1.0 - u, 1.0 - v],
            _ => [1.0 - v, u],
        };
        [x as f32, y as f32]
    }

    /// The canvas position of the workspace position `workspace`.
    pub fn canvas_from_workspace(&self, workspace: [f32; 2]) -> [f32; 2] {
        let [x, y] = workspace.map(f64::from);
        let [u, v] = match self.quarter_turns {
            0 => [x, y],
            1 => [1.0 - y, x],
            2 => [1.0 - x, 1.0 - y],
            _ => [y, 1.0 - x],
        };
        [u as f32, v as f32]
    }

    /// `crop` (stored, normalized to the frame) as an axis-aligned rectangle in the crop
    /// workspace. The two differ only in where the centre lands.
    pub fn workspace_crop(&self, crop: [f32; 4]) -> [f32; 4] {
        self.move_crop_center(crop, self.linear)
    }

    /// The inverse of [`Self::workspace_crop`].
    pub fn stored_crop(&self, workspace_crop: [f32; 4]) -> [f32; 4] {
        self.move_crop_center(workspace_crop, self.linear_inverse)
    }

    fn move_crop_center(&self, crop: [f32; 4], linear: [[f64; 2]; 2]) -> [f32; 4] {
        let crop = crop.map(f64::from);
        let offset =
            [0, 1].map(|axis| ((crop[axis] + crop[axis + 2]) * 0.5 - 0.5) * self.frame_size[axis]);
        let moved = apply_2x2(linear, offset);
        let center = [0, 1].map(|axis| 0.5 + moved[axis] / self.frame_size[axis]);
        let half = [(crop[2] - crop[0]) * 0.5, (crop[3] - crop[1]) * 0.5];
        [
            center[0] - half[0],
            center[1] - half[1],
            center[0] + half[0],
            center[1] + half[1],
        ]
        .map(|value| value as f32)
    }

    /// Frame pixels to `0.5 + Q · A · (point - center) / size`, where `size` is measured after
    /// quarter turns.
    fn view_from_frame(&self, center: [f64; 2], size: [f64; 2]) -> Homography {
        let turned = multiply_2x2(quarter_turn_matrix(self.quarter_turns), self.linear);
        let scaled = [0, 1].map(|row| turned[row].map(|value| value / size[row]));
        let moved_center = apply_2x2(scaled, center);
        Homography::affine(scaled, [0.5 - moved_center[0], 0.5 - moved_center[1]])
    }

    /// `size` (before quarter turns) stretched by `Σ`.
    fn stretched(&self, size: [f64; 2]) -> [f64; 2] {
        [size[0] * self.stretch[0], size[1] * self.stretch[1]]
    }

    /// `size` after quarter turns.
    fn turned(&self, size: [f64; 2]) -> [f64; 2] {
        if self.quarter_turns.is_multiple_of(2) {
            size
        } else {
            [size[1], size[0]]
        }
    }

    pub(super) fn crop_center(&self) -> [f64; 2] {
        self.crop_center
    }

    pub(super) fn crop_size(&self) -> [f64; 2] {
        self.crop_size
    }

    pub(super) fn frame_size_f64(&self) -> [f64; 2] {
        self.frame_size
    }

    pub(super) fn source_size(&self) -> [f64; 2] {
        self.source_size
    }

    /// The corrected source's corners in frame pixels, in the order top-left, top-right,
    /// bottom-right, bottom-left, which keeps their orientation (the interior on the left of
    /// each edge in y-down coordinates).
    pub(super) fn frame_corners(&self) -> [[f64; 2]; 4] {
        rectangle_corners(self.source_size).map(|corner| self.frame_from_source.apply_f64(corner))
    }

    /// The crop's corners in source pixels, in the orientation of [`Self::frame_corners`].
    pub(super) fn crop_source_corners(&self) -> [[f64; 2]; 4] {
        self.crop_corner_offsets(self.crop_size).map(|offset| {
            self.source_from_frame.apply_f64([
                self.crop_center[0] + offset[0],
                self.crop_center[1] + offset[1],
            ])
        })
    }

    /// The corners of a crop of `size` (output pixels before `Σ` and quarter turns) relative to
    /// its centre, frame pixels: `A⁻¹` of the output rectangle's corners, in the orientation of
    /// [`Self::frame_corners`].
    pub(super) fn crop_corner_offsets(&self, size: [f64; 2]) -> [[f64; 2]; 4] {
        let [width, height] = size;
        rectangle_corners(size)
            .map(|[x, y]| apply_2x2(self.linear_inverse, [x - width * 0.5, y - height * 0.5]))
    }
}

/// `[min, max]` of `points` along each axis.
pub(super) fn bounds(points: &[[f64; 2]]) -> [[f64; 2]; 2] {
    let fold = |start: f64, pick: fn(f64, f64) -> f64| {
        [0, 1].map(|axis| points.iter().map(|point| point[axis]).fold(start, pick))
    };
    [
        fold(f64::INFINITY, f64::min),
        fold(f64::NEG_INFINITY, f64::max),
    ]
}

fn rectangle_corners(size: [f64; 2]) -> [[f64; 2]; 4] {
    [
        [0.0, 0.0],
        [size[0], 0.0],
        [size[0], size[1]],
        [0.0, size[1]],
    ]
}

/// `R · S · F`: the straighten rotation after the skews after the flips.
fn linear_part(geometry: GeometryTransform) -> [[f64; 2]; 2] {
    let (sin, cos) = f64::from(geometry.rotation_degrees).to_radians().sin_cos();
    multiply_2x2([[cos, -sin], [sin, cos]], skew_flip(geometry))
}

/// `S · F`: the skews of `geometry` after its flips, which act on edge angles before the
/// rotation does.
pub(super) fn skew_flip(geometry: GeometryTransform) -> [[f64; 2]; 2] {
    let fx = if geometry.flip_horizontal { -1.0 } else { 1.0 };
    let fy = if geometry.flip_vertical { -1.0 } else { 1.0 };
    let shx = f64::from(geometry.horizontal_skew_degrees)
        .to_radians()
        .tan();
    let shy = f64::from(geometry.vertical_skew_degrees).to_radians().tan();
    multiply_2x2([[1.0, shx], [shy, 1.0]], [[fx, 0.0], [0.0, fy]])
}

/// The quarter turns as a map of offsets: the first turns `(x, y)` into `(-y, x)`.
fn quarter_turn_matrix(quarter_turns: u8) -> [[f64; 2]; 2] {
    match quarter_turns % 4 {
        0 => [[1.0, 0.0], [0.0, 1.0]],
        1 => [[0.0, -1.0], [1.0, 0.0]],
        2 => [[-1.0, 0.0], [0.0, -1.0]],
        _ => [[0.0, 1.0], [-1.0, 0.0]],
    }
}

pub(super) fn multiply_2x2(a: [[f64; 2]; 2], b: [[f64; 2]; 2]) -> [[f64; 2]; 2] {
    [0, 1].map(|row| [0, 1].map(|column| a[row][0] * b[0][column] + a[row][1] * b[1][column]))
}

pub(super) fn apply_2x2(matrix: [[f64; 2]; 2], [x, y]: [f64; 2]) -> [f64; 2] {
    [
        matrix[0][0] * x + matrix[0][1] * y,
        matrix[1][0] * x + matrix[1][1] * y,
    ]
}

fn inverse_2x2(matrix: [[f64; 2]; 2]) -> [[f64; 2]; 2] {
    let determinant = matrix[0][0] * matrix[1][1] - matrix[0][1] * matrix[1][0];
    [
        [matrix[1][1] / determinant, -matrix[0][1] / determinant],
        [-matrix[1][0] / determinant, matrix[0][0] / determinant],
    ]
}
