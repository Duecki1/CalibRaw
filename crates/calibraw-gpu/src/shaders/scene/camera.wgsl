// The scene camera of a depth result (calibraw-core `SceneCamera`, uploaded by
// pipeline/gpu/scene/camera.rs): how disparity becomes distance, and the
// shading frame every depth effect shares.
//
// With i a disparity (0 the farthest point the depth model saw, 1 the nearest):
//
//   D(i) = 1 / (s·i + t)                                  relative distance
//   d(i) = clamp((D(i) − D_near) / (D_far − D_near), 0, 1)  linear depth
//   z(d) = 1 + E · R · d                                  shading frame depth
//
// D_near and D_far are the distances of the nearest and the farthest surface
// that is not sky (the anchors), R is the frame relief and E Relight's relief
// scale (1 for every other effect), so the nearest surface lies at z = 1.
// Disparities below the farthest anchor (sky, haze beyond the scenery) count
// as the farthest surface, d = 1. D is in an arbitrary unit: only ratios of
// distances mean anything. Everything is f32.

struct SceneCamera {
    // Tangent of half the field of view across the shorter edge (x), the fit
    // (s, t) of D(i) (y, z) and the frame relief R (w).
    lens: vec4<f32>,
    // Disparity of the nearest (x) and the farthest (y) anchor, and their
    // distances D_near (z) and D_far (w).
    anchors: vec4<f32>,
}

// Relative distance D(i) of a disparity at least the farthest anchor's.
fn scene_distance(camera: SceneCamera, disparity: f32) -> f32 {
    return 1.0 / (camera.lens.y * disparity + camera.lens.z);
}

// Linear depth d of a disparity: 0 at the nearest surface, 1 at the farthest.
fn scene_linear_depth(camera: SceneCamera, disparity: f32) -> f32 {
    let distance = scene_distance(camera, max(disparity, camera.anchors.y));
    let range = camera.anchors.w - camera.anchors.z;
    return clamp((distance - camera.anchors.z) / range, 0.0, 1.0);
}

// Depth z in the shading frame of linear depth `d`, with relief scale E.
fn scene_frame_depth(camera: SceneCamera, d: f32, relief_scale: f32) -> f32 {
    return 1.0 + relief_scale * camera.lens.w * d;
}

// Linear depth of frame depth `z` at relief scale 1.
fn scene_depth_of_frame(camera: SceneCamera, z: f32) -> f32 {
    return (z - 1.0) / camera.lens.w;
}
