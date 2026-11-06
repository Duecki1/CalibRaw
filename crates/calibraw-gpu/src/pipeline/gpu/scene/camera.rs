//! The scene camera as the shaders read it (`shaders/scene/camera.wgsl`).

use calibraw_core::pipeline::SceneCamera;

/// `SceneCamera` in `camera.wgsl`: the depth result's camera with the anchors'
/// distances, which every reader would otherwise derive again.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(in crate::pipeline::gpu) struct SceneCameraUniform {
    /// `focal_tan`, the fit's `s` and `t`, and `frame_relief`.
    pub(in crate::pipeline::gpu) lens: [f32; 4],
    /// The nearest and farthest anchor's disparity, then their distances.
    pub(in crate::pipeline::gpu) anchors: [f32; 4],
}

impl SceneCameraUniform {
    pub(super) fn new(camera: &SceneCamera) -> Self {
        let [s, t] = camera.fit;
        let [near, far] = camera.anchors;
        Self {
            lens: [camera.focal_tan, s, t, camera.frame_relief],
            anchors: [near, far, camera.distance(near), camera.distance(far)],
        }
    }
}

/// The frame position of a light placed as Relight places it, as
/// `relight_light` in relight.wgsl computes it from the same values: at
/// `source` (percent of the full image) and control depth `light_depth`
/// (percent) with relief scale `relief_scale`, in an image of `aspect`
/// (width and height in shorter edges). See `scene_light_frame_depth` and
/// `scene_frame_position` in camera.wgsl.
pub(super) fn light_position(
    camera: &SceneCamera,
    aspect: [f32; 2],
    source: [f32; 2],
    light_depth: f32,
    relief_scale: f32,
) -> [f32; 3] {
    let control = (light_depth / 100.0).clamp(-1.0, 1.0);
    let z = if control < 0.0 {
        1.0 + 0.9 * control
    } else {
        let [near, far] = camera.anchors.map(|anchor| camera.distance(anchor));
        let ratio = far / near;
        let d = if ratio < 1.0001 {
            control
        } else {
            (ratio.powf(control) - 1.0) / (ratio - 1.0)
        };
        1.0 + relief_scale * camera.frame_relief * d
    };
    let point = [
        (source[0] / 100.0 - 0.5) * aspect[0],
        (source[1] / 100.0 - 0.5) * aspect[1],
    ];
    let lateral = 2.0 * camera.focal_tan * z;
    [point[0] * lateral, point[1] * lateral, z]
}
