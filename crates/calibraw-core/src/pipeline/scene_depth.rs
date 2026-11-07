//! Scene depth: the photo's geometry as the depth job measured it, kept with
//! the edit, and the camera that every effect reading depth shares.
//!
//! A result ([`SceneDepth`]) holds four parts, each at the size its model
//! predicted it at and all in full-image coordinates (the whole photo before
//! any crop), so they need not share a size:
//! - disparity ([`DisparityMap`]), 16 bits, high for near;
//! - the probability that a pixel is sky, soft, as a [`MaskImage`];
//! - surface normals ([`SceneNormals`]);
//! - the camera ([`SceneCamera`]), which turns disparity into distance.
//!
//! Depth Range masks select on [`SceneDepth::selection_map`], derived from
//! the disparity, the camera's anchors and the sky.

use super::{MaskImage, SceneNormals};
use rayon::prelude::*;
use std::sync::Arc;

/// Disparity per pixel, by row: the depth model's output normalized by its own
/// minimum and maximum, so 0 is the farthest point it saw and
/// [`Self::NEAREST`] the nearest. Disparity goes as inverse distance up to an
/// offset, which [`SceneCamera::fit`] supplies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisparityMap {
    width: u32,
    height: u32,
    values: Arc<[u16]>,
}

impl DisparityMap {
    /// The stored value of disparity 1.
    pub const NEAREST: u16 = u16::MAX;

    /// `values` by row, one per pixel; `None` when their count does not match.
    pub fn new(width: u32, height: u32, values: Vec<u16>) -> Option<Self> {
        let count = usize::try_from(width)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?;
        (count > 0 && values.len() == count).then(|| Self {
            width,
            height,
            values: values.into(),
        })
    }

    /// The stored value of a disparity in [0, 1].
    pub fn encode(disparity: f32) -> u16 {
        (disparity.clamp(0.0, 1.0) * f32::from(Self::NEAREST)).round() as u16
    }

    /// The disparity, in [0, 1], that a stored `value` holds.
    pub fn decode(value: u16) -> f32 {
        f32::from(value) / f32::from(Self::NEAREST)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The stored values by row.
    pub fn values(&self) -> &Arc<[u16]> {
        &self.values
    }
}

/// The camera and distance scale of a depth result: how disparity becomes
/// distance and how an image point becomes a direction. Stored with the
/// result; keep the field names stable.
///
/// Image points are in shorter-edge units. With `i` a disparity:
///
/// ```text
/// D(i)  = 1 / (s·i + t)                              relative distance, (s, t) = fit
/// d(i)  = (D(i) − D_near) / (D_far − D_near)         linear depth, 0 nearest, 1 farthest
/// z(i)  = 1 + frame_relief · d(i)                    shading frame, nearest surface at z = 1
/// lateral position = 2 · focal_tan · (point − centre) · z
/// ```
///
/// where `D_near` and `D_far` are the distances of `anchors`. Distances are
/// defined for disparities from the farthest anchor up; callers clamp below.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct SceneCamera {
    /// Tangent of half the field of view across the shorter edge.
    pub focal_tan: f32,
    /// `(s, t)` of the relative distance `D(i) = 1 / (s·i + t)`, in an
    /// arbitrary unit: only ratios of distances are used.
    pub fit: [f32; 2],
    /// Disparity of the nearest and of the farthest surface that is not sky.
    pub anchors: [f32; 2],
    /// Depth range of the shading frame in units of the nearest surface's
    /// distance.
    pub frame_relief: f32,
}

impl SceneCamera {
    /// Whether the camera describes a scene: finite values, a positive field of
    /// view and relief, anchors in [0, 1] with the nearest above the farthest,
    /// and positive distances that grow from the nearest anchor to the farthest.
    pub fn is_valid(&self) -> bool {
        let [near, far] = self.anchors;
        let values = [
            self.focal_tan,
            self.fit[0],
            self.fit[1],
            near,
            far,
            self.frame_relief,
        ];
        values.iter().all(|value| value.is_finite())
            && self.focal_tan > 0.0
            && self.frame_relief > 0.0
            && (0.0..=1.0).contains(&far)
            && (0.0..=1.0).contains(&near)
            && far < near
            // Positive at both anchors and growing toward the farthest
            // (s > 0), so positive in between.
            && self.inverse_distance(far) > 0.0
            && self.fit[0] > 0.0
            && self.distance(far).is_finite()
    }

    fn inverse_distance(&self, disparity: f32) -> f32 {
        self.fit[0] * disparity + self.fit[1]
    }

    /// Relative distance `D(i)` of `disparity`, at least the farthest anchor's
    /// disparity.
    pub fn distance(&self, disparity: f32) -> f32 {
        1.0 / self.inverse_distance(disparity)
    }

    /// Linear depth `d(i)`: 0 at the nearest anchor, 1 at the farthest, clamped.
    pub fn linear_depth(&self, disparity: f32) -> f32 {
        let [near, far] = self.anchors.map(|anchor| self.distance(anchor));
        let disparity = disparity.max(self.anchors[1]);
        ((self.distance(disparity) - near) / (far - near)).clamp(0.0, 1.0)
    }

    /// Depth `z(i)` in the shading frame: 1 at the nearest surface,
    /// `1 + frame_relief` at the farthest.
    pub fn frame_depth(&self, disparity: f32) -> f32 {
        1.0 + self.frame_relief * self.linear_depth(disparity)
    }

    /// What Depth Range masks select on, before the sky: 0 at the nearest
    /// anchor and 1 at the farthest, linear in disparity and clamped.
    pub fn selection(&self, disparity: f32) -> f32 {
        let [near, far] = self.anchors;
        (1.0 - (disparity - far) / (near - far)).clamp(0.0, 1.0)
    }
}

/// One depth result: disparity, sky, normals and camera (module docs).
#[derive(Clone, Debug, PartialEq)]
pub struct SceneDepth {
    disparity: DisparityMap,
    sky: MaskImage,
    normals: SceneNormals,
    camera: SceneCamera,
    /// Derived from the others when built (`selection_map`).
    selection: MaskImage,
}

impl SceneDepth {
    /// A result from its parts; `None` when the camera is not valid
    /// ([`SceneCamera::is_valid`]) or the sky is empty.
    pub fn new(
        disparity: DisparityMap,
        sky: MaskImage,
        normals: SceneNormals,
        camera: SceneCamera,
    ) -> Option<Self> {
        if !camera.is_valid() || sky.width == 0 || sky.height == 0 {
            return None;
        }
        let selection = selection_map(&disparity, &sky, &camera);
        Some(Self {
            disparity,
            sky,
            normals,
            camera,
            selection,
        })
    }

    pub fn disparity(&self) -> &DisparityMap {
        &self.disparity
    }

    /// Probability that a pixel is sky, 0 to 255.
    pub fn sky(&self) -> &MaskImage {
        &self.sky
    }

    pub fn normals(&self) -> &SceneNormals {
        &self.normals
    }

    pub fn camera(&self) -> &SceneCamera {
        &self.camera
    }

    /// What Depth Range masks select on, at the disparity's size: 0 nearest,
    /// 255 farthest. It is [`SceneCamera::selection`], moved toward the
    /// farthest by the sky's probability, in 255 levels as Depth Range has
    /// always selected. Shared by every clone of this result.
    pub fn selection_map(&self) -> &MaskImage {
        &self.selection
    }
}

/// [`SceneDepth::selection_map`] of `disparity`, with `sky` sampled bilinearly
/// at each disparity pixel's centre: exactly the sky's own pixel when both
/// have the same size.
fn selection_map(disparity: &DisparityMap, sky: &MaskImage, camera: &SceneCamera) -> MaskImage {
    let width = disparity.width as usize;
    let mut pixels = vec![0u8; disparity.values.len()];
    let scale_x = sky.width as f32 / disparity.width as f32;
    let scale_y = sky.height as f32 / disparity.height as f32;
    let axis = |position: usize, scale: f32, extent: u32| {
        let p = ((position as f32 + 0.5) * scale - 0.5).clamp(0.0, (extent - 1) as f32);
        let low = p.floor() as usize;
        (low, (low + 1).min(extent as usize - 1), p - low as f32)
    };
    pixels
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            let (y0, y1, fy) = axis(y, scale_y, sky.height);
            for (x, pixel) in row.iter_mut().enumerate() {
                let (x0, x1, fx) = axis(x, scale_x, sky.width);
                let at = |x: usize, y: usize| f32::from(sky.pixels[y * sky.width as usize + x]);
                let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * fx;
                let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * fx;
                let sky = (top + (bottom - top) * fy) / 255.0;
                let near_far =
                    camera.selection(DisparityMap::decode(disparity.values[y * width + x]));
                *pixel = ((near_far + (1.0 - near_far) * sky) * 255.0).round() as u8;
            }
        });
    MaskImage::new(disparity.width, disparity.height, pixels)
        .expect("the selection map has one pixel per disparity value")
}
