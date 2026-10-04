// Blends cached AI Remove patches over the demosaiced scene texture. Inputs
// and output share the scene's colour space and the work texture format.
// `params` selects the pixel rectangle in scene texture coordinates; the patch
// alpha is its coverage, so uncovered pixels pass through unchanged.
struct RemoveCompositeParams {
    origin: vec2<u32>,
    extent: vec2<u32>,
};

@group(0) @binding(0) var scene_input: texture_2d<f32>;
@group(0) @binding(1) var patch_input: texture_2d<f32>;
@group(0) @binding(2) var scene_output: texture_storage_2d<rgba16float /* CALIBRAW_WORK_FORMAT */, write>;
@group(0) @binding(3) var<uniform> params: RemoveCompositeParams;

@compute @workgroup_size(8, 8)
fn composite_remove_patch(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.extent.x || id.y >= params.extent.y) {
        return;
    }
    let location = params.origin + id.xy;
    let coordinates = vec2<i32>(location);
    let base = textureLoad(scene_input, coordinates, 0);
    let cached = textureLoad(patch_input, coordinates, 0);
    let blended = base.rgb + (cached.rgb - base.rgb) * cached.a;
    textureStore(scene_output, coordinates, vec4<f32>(blended, 1.0));
}
