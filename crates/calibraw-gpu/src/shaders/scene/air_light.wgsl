#import calibraw::scene::camera as Camera
#import calibraw::scene::bake_common as Bake
#import calibraw::scene::lights_common as Lights

// L3, the air light (docs/rework/DEPTH_EFFECTS_PLAN.md §4.4): for every view
// ray of a pipeline's region and every light of the light bake, the share of
// the light the ray receives in the air that the scene does not shadow,
//     share = Σ visibility · w / Σ w,
// with w the light's falloff along the ray. Media (Fog, Smoke, Rain's mist)
// multiply their own analytic, unshadowed in-scattering by it, so it holds
// nothing of their density, phase or colour, and moving one of their sliders
// rebuilds nothing.
//
// Grid. The grid of lights_common.wgsl `SceneAirLight`: one ray per grid
// texel, through the centre of the full-image pixels it covers, `march.x` of
// them on an edge (2 in the Final tier, 4 in Draft, of the pipeline's own
// pixels). Every ray depends only on its grid position, so export tiles and
// crops of any size compute the same rays where they overlap.
//
// March. A ray runs from the nearest surface's depth (d = 0; media hold nothing
// nearer) to the surface it meets: the far surface of a pixel that shows two,
// the sky at d = 1. It takes `march.y` steps even in control depth c, each
// moved by one jitter hashed from the grid position (deterministic, the same
// for every render), and a step's weight is the light's falloff at it times
// the length of ray the step stands for. Visibility at a step is the light's
// shadow cube read there (`scene_cube_visibility` with no normal), filtered
// over `march.z` taps. The share to the near surface sums the steps before
// it; to the far surface, all. The scene's frame is the light's (its relief
// scale E): the cube was drawn in it.
//
// Precision: f32 throughout; the shares are stored as half floats, depths
// likewise (within 1/2048 of linear depth).

// The air light's texture, written: layer 0 depths, then two per group.
@group(0) @binding(116) var scene_air_out: texture_storage_2d_array<rgba16float, write>;

// Sky probability from which the far surface is sky.
const SCENE_AIR_SKY: f32 = 0.5;

// A hash of a grid position into [0, 1).
fn scene_air_jitter(grid: vec2<i32>) -> f32 {
    var h = bitcast<u32>(grid.x) * 1597334677u ^ bitcast<u32>(grid.y) * 3812015801u;
    h = (h ^ (h >> 16u)) * 2246822519u;
    h = (h ^ (h >> 13u)) * 3266489917u;
    h = h ^ (h >> 16u);
    return f32(h & 0x00ffffffu) / 16777216.0;
}

@compute @workgroup_size(8, 8, 1)
fn bake_scene_air_light(@builtin(global_invocation_id) gid: vec3<u32>) {
    let local = vec2<i32>(gid.xy);
    let air = Lights::scene_air;
    if any(local >= air.region.zw) { return; }
    let grid = air.region.xy + local;
    let image = vec2<f32>(air.image.xy);
    let uv = (vec2<f32>(grid) + vec2<f32>(0.5)) * f32(air.march.x) / image;

    // The surfaces the ray meets: the bake's texel there.
    let size = Bake::scene_bake_size();
    let texel = Bake::scene_bake_clamp(vec2<i32>(floor(clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)) * vec2<f32>(size))));
    let geometry = textureLoad(Bake::scene_geometry_tex, texel, 0);
    let one_surface = Bake::scene_coverage_constant(textureLoad(Bake::scene_coverage_tex, texel, 0));
    var near_depth = geometry.z;
    var far_depth = geometry.w;
    if geometry.y >= SCENE_AIR_SKY {
        far_depth = 1.0;
        if one_surface {
            near_depth = 1.0;
        }
    }
    textureStore(scene_air_out, local, 0, vec4<f32>(near_depth, far_depth, 0.0, 1.0));

    let camera = Bake::scene_bake.camera;
    let working = vec2<f32>(size);
    let point = Camera::scene_image_point(uv, working / min(working.x, working.y));
    let far_control = Camera::scene_control_of_depth(camera, far_depth);
    let steps = max(air.march.y, 1u);
    let step = far_control / f32(steps);
    let jitter = scene_air_jitter(grid);
    for (var group = 0u; group < air.march.w; group = group + 1u) {
        var near_share = vec4<f32>(1.0);
        var far_share = vec4<f32>(1.0);
        for (var index = 0u; index < 4u; index = index + 1u) {
            let light = Lights::scene_shadow_light(4u * group + index);
            if !Lights::scene_shadow_light_active(light) {
                continue;
            }
            let relief_scale = light.position.w;
            let reach = max(light.air.x, 1e-6);
            var sums = vec4<f32>(0.0); // near: Σ v·w, Σ w; far: Σ v·w, Σ w
            for (var k = 0u; k < steps; k = k + 1u) {
                let control = (f32(k) + jitter) * step;
                let d = Camera::scene_depth_of_control(camera, control);
                let z = Camera::scene_frame_depth(camera, d, relief_scale);
                let position = Camera::scene_frame_position(camera, point, z);
                let offset = position - light.position.xyz;
                let falloff = 1.0 / (1.0 + dot(offset, offset) / (reach * reach));
                let weight = falloff * Camera::scene_depth_per_control(camera, control);
                let visibility = Lights::scene_cube_visibility(light, position, vec3<f32>(0.0), 0u, air.march.z);
                let sample = vec2<f32>(visibility * weight, weight);
                sums = sums + vec4<f32>(select(vec2<f32>(0.0), sample, d <= near_depth), sample);
            }
            near_share[index] = select(1.0, sums.x / sums.y, sums.y > 0.0);
            far_share[index] = select(1.0, sums.z / sums.w, sums.w > 0.0);
        }
        textureStore(scene_air_out, local, 1u + 2u * group, near_share);
        textureStore(scene_air_out, local, 2u + 2u * group, far_share);
    }
}
