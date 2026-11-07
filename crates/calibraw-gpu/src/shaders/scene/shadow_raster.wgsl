#import calibraw::scene::camera as Camera
#import calibraw::scene::bake_common as Bake
#import calibraw::scene::lights_common as Lights

// L1: the shadow cubes of the light bake (lights_common.wgsl). Each face draws
// the scene's visible surface as one mesh, built in the vertex shader from
// `vertex_index`, with no vertex buffer.
//
// The mesh. Its vertices are the texel centres of a mesh level: the working
// size, or half of it (the Draft tier, `SceneShadowLight.shape.w`). Each holds
// the linear depth of the visible surface there (`prepare_scene_shadow_mesh`):
// the near surface where it covers at least half the texel, else the far
// one, so no vertex lies between two surfaces. Sky is no surface: a triangle
// with a sky corner is not drawn, so a light behind the scene is not blocked
// by it. Every quad of four texels is two triangles.
//
// Walls. A triangle whose corners lie more than a wall's depth apart
// (SCENE_WALL_DEPTH) spans a silhouette: drawn, it would be a sheet from the
// near surface down to the far one, shading everything behind the outline.
// Such a triangle is not drawn. Where two of its corners lie on the near side,
// the edge between them is part of the near surface's outline, and a wall
// hangs behind it instead: both corners pushed back along their own view rays
// by the depth to the far corner, at most a wall's depth. That stands for the
// near surface's unseen back, about as deep as objects are wide, so light
// does not pass behind a silhouette as if the object were a sheet; and since
// it lies straight behind the outline, a light behind and beside the object
// still reaches its rim. Each triangle therefore has six vertices: its own
// three and three spare, or the wall's two triangles; what is not drawn is
// collapsed to a point outside the clip volume.
//
// Instances. Instance 6 · channel + face draws face `face` of the light in
// `channel`, into a render pass whose target is that face's layer.

// The mesh's visible depth: linear depth d at a texel, or -1 for sky.
// r32float, two levels: the working size and half of it.
@group(0) @binding(107) var scene_mesh_tex: texture_2d<f32>;
@group(0) @binding(108) var scene_mesh_out: texture_storage_2d<r32float, write>;

// How deep a wall reaches behind an outline, as a share of the outline's depth
// in the frame of relief scale 1.
const SCENE_WALL_DEPTH: f32 = 0.1;
// Vertices per triangle of the grid: its own three and three spare, or a wall.
const SCENE_VERTICES_PER_TRIANGLE: u32 = 6u;
// Sky probability from which a surface is sky.
const SCENE_MESH_SKY: f32 = 0.5;

// Level 0 of the mesh, from the bake.
@compute @workgroup_size(8, 8, 1)
fn prepare_scene_shadow_mesh(@builtin(global_invocation_id) gid: vec3<u32>) {
    let texel = vec2<i32>(gid.xy);
    if any(texel >= Bake::scene_bake_size()) { return; }
    let geometry = textureLoad(Bake::scene_geometry_tex, texel, 0);
    let coefficients = textureLoad(Bake::scene_coverage_tex, texel, 0);
    var depth = geometry.z;
    if Bake::scene_coverage_constant(coefficients) {
        // One surface, the texel's own.
        if geometry.y >= SCENE_MESH_SKY {
            depth = -1.0;
        }
    } else {
        let rgb = textureLoad(Bake::scene_working_tex, texel, 0).rgb;
        let coverage = Bake::scene_coverage_of(coefficients, Bake::scene_bake_guide(rgb));
        if coverage < 0.5 {
            depth = geometry.w;
            if geometry.y >= SCENE_MESH_SKY {
                depth = -1.0;
            }
        }
    }
    textureStore(scene_mesh_out, texel, vec4<f32>(depth, 0.0, 0.0, 0.0));
}

// Level 1 of the mesh: per texel the nearest surface below it, sky only where
// all four are. The level is half the working size, rounded down.
@compute @workgroup_size(8, 8, 1)
fn reduce_scene_shadow_mesh(@builtin(global_invocation_id) gid: vec3<u32>) {
    let texel = vec2<i32>(gid.xy);
    let size = vec2<i32>(textureDimensions(scene_mesh_out));
    if any(texel >= size) { return; }
    let last = Bake::scene_bake_size() - vec2<i32>(1);
    var depth = 2.0;
    for (var corner = 0; corner < 4; corner = corner + 1) {
        let source = min(2 * texel + vec2<i32>(corner % 2, corner / 2), last);
        let d = textureLoad(scene_mesh_tex, source, 0).x;
        if d >= 0.0 {
            depth = min(depth, d);
        }
    }
    textureStore(scene_mesh_out, texel, vec4<f32>(select(depth, -1.0, depth > 1.5), 0.0, 0.0, 0.0));
}

// The size of mesh level `level`, from level 0's (higher levels' sizes are
// misreported by some drivers).
fn scene_mesh_size(level: u32) -> vec2<i32> {
    return max(vec2<i32>(textureDimensions(scene_mesh_tex, 0)) >> vec2<u32>(level), vec2<i32>(1));
}

// Outside every clip volume: a corner of a triangle that is not drawn.
const SCENE_NOT_DRAWN: vec4<f32> = vec4<f32>(2.0, 2.0, 2.0, 1.0);

// The clip position of the mesh vertex at `texel` of a mesh level of `size`, at
// frame depth `z`, seen from `light` through face `face`.
fn scene_mesh_clip(light: Lights::SceneShadowLight, face: u32, texel: vec2<i32>, size: vec2<i32>, z: f32) -> vec4<f32> {
    let uv = (vec2<f32>(texel) + vec2<f32>(0.5)) / vec2<f32>(size);
    let working = vec2<f32>(Bake::scene_bake_size());
    let point = Camera::scene_image_point(uv, working / min(working.x, working.y));
    let position = Camera::scene_frame_position(Bake::scene_bake.camera, point, z);
    let c = Lights::scene_cube_coordinates(position - light.position.xyz, face);
    return Lights::scene_cube_clip(c);
}

@vertex
fn scene_shadow_vertex(
    @builtin(vertex_index) vertex: u32,
    @builtin(instance_index) instance: u32,
) -> @builtin(position) vec4<f32> {
    let light = Lights::scene_shadow_light(instance / 6u);
    let face = instance % 6u;
    let level = u32(light.shape.w);
    let size = scene_mesh_size(level);
    let quads = max(size.x - 1, 1);
    let triangle = vertex / SCENE_VERTICES_PER_TRIANGLE;
    let slot = vertex % SCENE_VERTICES_PER_TRIANGLE;
    let quad = i32(triangle / 2u);
    let base = vec2<i32>(quad % quads, quad / quads);
    // Corners of the quad's two triangles, counter-clockwise in the image.
    var offsets = array<vec2<i32>, 6>(
        vec2<i32>(0, 0), vec2<i32>(1, 0), vec2<i32>(0, 1),
        vec2<i32>(1, 0), vec2<i32>(1, 1), vec2<i32>(0, 1),
    );
    let first = 3u * (triangle % 2u);
    let relief_scale = light.position.w;
    let camera = Bake::scene_bake.camera;
    var texels: array<vec2<i32>, 3>;
    var depths: array<f32, 3>;
    var nearest = 1e9;
    var farthest = 0.0;
    for (var corner = 0u; corner < 3u; corner = corner + 1u) {
        let texel = base + offsets[first + corner];
        let d = textureLoad(scene_mesh_tex, texel, i32(level)).x;
        if d < 0.0 {
            return SCENE_NOT_DRAWN;
        }
        let z = Camera::scene_frame_depth(camera, d, relief_scale);
        texels[corner] = texel;
        depths[corner] = z;
        nearest = min(nearest, z);
        farthest = max(farthest, z);
    }
    // The nearest corner's depth in the frame of relief scale 1 is
    // 1 + (z − 1) / E, so a wall reaching T of it reaches T · (E + z − 1) here.
    let wall = SCENE_WALL_DEPTH * (relief_scale + nearest - 1.0);
    if farthest - nearest <= wall {
        // A continuous surface: the triangle itself.
        if slot >= 3u {
            return SCENE_NOT_DRAWN;
        }
        return scene_mesh_clip(light, face, texels[slot], size, depths[slot]);
    }
    // Across a silhouette: a wall behind the edge of its two near corners.
    var near = array<u32, 2>(0u, 0u);
    var count = 0u;
    for (var corner = 0u; corner < 3u; corner = corner + 1u) {
        if depths[corner] - nearest <= wall {
            if count < 2u {
                near[count] = corner;
            }
            count = count + 1u;
        }
    }
    if count != 2u {
        return SCENE_NOT_DRAWN;
    }
    // The wall's quad (a, b, b', a') as triangles (a, b, b') and (a, b', a').
    var corners = array<u32, 6>(near[0], near[1], near[1], near[0], near[1], near[0]);
    var pushed = array<bool, 6>(false, false, true, false, true, true);
    let corner = corners[slot];
    var z = depths[corner];
    if pushed[slot] {
        z = z + min(farthest - z, wall);
    }
    return scene_mesh_clip(light, face, texels[corner], size, z);
}
