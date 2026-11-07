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

// Control depth c, which sliders use, is even in the log of distance from the
// nearest anchor (0) to the farthest (1): c = ln(D / D_near) / ln Q with
// Q = D_far / D_near. Linear depth of control depth: d_of_c(c) = (Q^c − 1) / (Q − 1).
fn scene_depth_of_control(camera: SceneCamera, control: f32) -> f32 {
    let ratio = camera.anchors.w / camera.anchors.z;
    // A flat scene: control and linear depth agree.
    if ratio < 1.0001 {
        return control;
    }
    return (pow(ratio, control) - 1.0) / (ratio - 1.0);
}

// Control depth of linear depth `d`: c = ln(1 + (Q − 1) · d) / ln Q, the
// inverse of scene_depth_of_control.
fn scene_control_of_depth(camera: SceneCamera, d: f32) -> f32 {
    let ratio = camera.anchors.w / camera.anchors.z;
    if ratio < 1.0001 {
        return d;
    }
    return log(1.0 + (ratio - 1.0) * d) / log(ratio);
}

// d(d_of_c) / dc at control depth `control`: ln Q · Q^c / (Q − 1).
fn scene_depth_per_control(camera: SceneCamera, control: f32) -> f32 {
    let ratio = camera.anchors.w / camera.anchors.z;
    if ratio < 1.0001 {
        return 1.0;
    }
    return log(ratio) * pow(ratio, control) / (ratio - 1.0);
}

// Frame depth z of a light at control depth `control` (Relight's Light
// depth / 100) with relief scale E: on the scene's depth range from 0 on,
// and before the nearest surface, toward the camera, z = 1 + 0.9 · control
// (the camera at -1).
fn scene_light_frame_depth(camera: SceneCamera, control: f32, relief_scale: f32) -> f32 {
    if control < 0.0 {
        return 1.0 + 0.9 * max(control, -1.0);
    }
    return scene_frame_depth(camera, scene_depth_of_control(camera, min(control, 1.0)), relief_scale);
}

// Image point of full-image `uv`: in shorter edges from the image's centre,
// x right and y down. `aspect` is the image's width and height in shorter
// edges.
fn scene_image_point(uv: vec2<f32>, aspect: vec2<f32>) -> vec2<f32> {
    return (uv - vec2<f32>(0.5)) * aspect;
}

// Position in the shading frame of image point `point` at frame depth `z`:
// lateral position 2 · focal_tan · point · z, x right, y down, z away from
// the camera, which lies at the origin.
fn scene_frame_position(camera: SceneCamera, point: vec2<f32>, z: f32) -> vec3<f32> {
    return vec3<f32>(2.0 * camera.lens.x * point * z, z);
}

// A unit normal at image point `point` and frame depth `z` (relief scale E)
// in the frame of relief scale E, from the depth result's normal `normal`,
// which belongs to the frame of relief scale 1. The frame of scale E maps a
// point at depth z1 on its view ray to depth zE = 1 + E · (z1 − 1) on the same
// ray; normals take the inverse transpose of that map's Jacobian, which in
// the scaled frame gives n' ∝ (n.x, n.y, (s · n.z − (E − 1) · (2 · focal_tan
// · point) · n.xy / z1) / E) with s = zE / z1.
fn scene_frame_normal(
    camera: SceneCamera,
    normal: vec3<f32>,
    point: vec2<f32>,
    z: f32,
    relief_scale: f32,
) -> vec3<f32> {
    if abs(relief_scale - 1.0) < 1e-4 {
        return normal;
    }
    let unscaled = 1.0 + (z - 1.0) / relief_scale;
    let ray = 2.0 * camera.lens.x * point;
    let along = (z / unscaled * normal.z - (relief_scale - 1.0) * dot(ray, normal.xy) / unscaled)
        / relief_scale;
    return normalize(vec3<f32>(normal.xy, along));
}
