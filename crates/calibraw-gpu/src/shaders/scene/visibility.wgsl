#import calibraw::scene::camera as Camera
#import calibraw::scene::bake_common as Bake
#import calibraw::scene::lights_common as Lights

// L2: how much of each light reaches each surface of the scene bake
// (lights_common.wgsl), at the working size in the full-image frame.
//
// The visibility array (rgba8unorm) holds two layers per group of four
// lights: the near surface's visibility of the group's lights in its
// channels, then the far surface's. A texel at no edge holds one surface,
// written to both layers. Sky is no surface and reads 1; it receives no
// light anyway. Each surface is lit at its own depth and normal, so the near
// and the far surface of a silhouette's texels are shadowed apart.

// The group this dispatch writes (x).
@group(0) @binding(111) var<uniform> scene_visibility_group: vec4<u32>;
@group(0) @binding(109) var scene_visibility_near_out: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(110) var scene_visibility_far_out: texture_storage_2d<rgba8unorm, write>;

// Sky probability from which a surface is sky.
const SCENE_VISIBILITY_SKY: f32 = 0.5;

struct SceneVisibilitySurface {
    // Linear depth; below 0 for sky.
    d: f32,
    normal: vec3<f32>,
}

// Visibility of the group's lights at a surface.
fn scene_group_visibility(texel: vec2<i32>, surface: SceneVisibilitySurface) -> vec4<f32> {
    var visibility = vec4<f32>(1.0);
    if surface.d < 0.0 {
        return visibility;
    }
    let camera = Bake::scene_bake.camera;
    let working = vec2<f32>(Bake::scene_bake_size());
    let point = Camera::scene_image_point(Bake::scene_bake_uv(texel), working / min(working.x, working.y));
    let taps = Lights::scene_shadow_lights.taps;
    for (var index = 0u; index < 4u; index = index + 1u) {
        let light = Lights::scene_shadow_light(4u * scene_visibility_group.x + index);
        if !Lights::scene_shadow_light_active(light) {
            continue;
        }
        let relief_scale = light.position.w;
        let z = Camera::scene_frame_depth(camera, surface.d, relief_scale);
        let position = Camera::scene_frame_position(camera, point, z);
        let normal = Camera::scene_frame_normal(camera, surface.normal, point, z, relief_scale);
        visibility[index] = Lights::scene_cube_visibility(light, position, normal, taps.x, taps.y);
    }
    return visibility;
}

@compute @workgroup_size(8, 8, 1)
fn bake_scene_visibility(@builtin(global_invocation_id) gid: vec3<u32>) {
    let texel = vec2<i32>(gid.xy);
    if any(texel >= Bake::scene_bake_size()) { return; }
    let geometry = textureLoad(Bake::scene_geometry_tex, texel, 0);
    let normals = textureLoad(Bake::scene_normals_layer_tex, texel, 0);
    let edge = textureLoad(Bake::scene_near_tex, texel, 0).w > 0.5;
    var near = SceneVisibilitySurface(geometry.z, Bake::scene_from_octahedral(normals.xy));
    if !edge {
        if geometry.y >= SCENE_VISIBILITY_SKY {
            near.d = -1.0;
        }
        let visibility = scene_group_visibility(texel, near);
        textureStore(scene_visibility_near_out, texel, visibility);
        textureStore(scene_visibility_far_out, texel, visibility);
        return;
    }
    var far = SceneVisibilitySurface(geometry.w, Bake::scene_from_octahedral(normals.zw));
    if geometry.y >= SCENE_VISIBILITY_SKY {
        far.d = -1.0;
    }
    textureStore(scene_visibility_near_out, texel, scene_group_visibility(texel, near));
    textureStore(scene_visibility_far_out, texel, scene_group_visibility(texel, far));
}
