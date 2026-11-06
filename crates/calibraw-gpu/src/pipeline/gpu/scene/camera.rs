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
