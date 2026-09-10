#import calibraw::common as Common
#import calibraw::tonemap as Tonemap
#import calibraw::scene_adjustments as SceneAdjustments

// Dark-channel transmission with a scalar guided filter (He et al., 2009/2010).
// All estimation and inversion happens before tone curves in linear Rec.2020.
// See docs/dehaze-research.md for equations, departures, and limitations.
@group(0) @binding(22) var source: texture_2d<f32>;
@group(0) @binding(30) var auxiliary: texture_2d<f32>;
@group(0) @binding(23) var destination: texture_storage_2d<rgba16float /* CALIBRAW_WORK_FORMAT */, write>;

fn airlight() -> vec3<f32> {
    return max(Tonemap::tone_stats.airlight.xyz * exp2(Common::scene_tone_uniforms.exposure), vec3<f32>(1e-6));
}

fn source_at(pos: vec2<i32>) -> vec3<f32> {
    return textureLoad(source, Common::clamp_pos(pos), 0).xyz;
}

fn auxiliary_at(pos: vec2<i32>) -> vec4<f32> {
    return textureLoad(auxiliary, Common::clamp_pos(pos), 0);
}

fn dark_ratio(rgb: vec3<f32>, ambient: vec3<f32>) -> f32 {
    let ratio = max(rgb, vec3<f32>(0.0)) / ambient;
    return clamp(min(ratio.r, min(ratio.g, ratio.b)), 0.0, 1.0);
}

fn guide(rgb: vec3<f32>, ambient: vec3<f32>) -> f32 {
    // Bounded, exposure-invariant guide; highlights above airlight remain valid.
    let y = Common::safe_luma(rgb) / Common::safe_luma(ambient);
    return y / (1.0 + y);
}

fn dark_radius() -> i32 {
    return SceneAdjustments::presence_step(7.0, 14);
}

fn guide_radius() -> i32 {
    return SceneAdjustments::presence_step(4.0, 8);
}

@compute @workgroup_size(8, 8, 1)
fn dark_horizontal(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(gid.xy);
    let ambient = airlight();
    var dark = 1.0;
    let radius = dark_radius();
    for (var x = -radius; x <= radius; x++) {
        dark = min(dark, dark_ratio(source_at(pos + vec2<i32>(x, 0)), ambient));
    }
    textureStore(destination, pos, vec4<f32>(dark, guide(source_at(pos), ambient), 0.0, 1.0));
}

@compute @workgroup_size(8, 8, 1)
fn dark_vertical(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(gid.xy);
    var dark = 1.0;
    let radius = dark_radius();
    for (var y = -radius; y <= radius; y++) {
        dark = min(dark, auxiliary_at(pos + vec2<i32>(0, y)).x);
    }
    textureStore(destination, pos, vec4<f32>(dark, auxiliary_at(pos).y, 0.0, 1.0));
}

@compute @workgroup_size(8, 8, 1)
fn guided_coefficients(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(gid.xy);
    let radius = guide_radius();
    var moments = vec4<f32>(0.0);
    for (var y = -radius; y <= radius; y++) {
        for (var x = -radius; x <= radius; x++) {
            let sample = auxiliary_at(pos + vec2<i32>(x, y));
            let p = sample.x;
            let i = sample.y;
            moments += vec4<f32>(i, p, i * i, i * p);
        }
    }
    moments /= f32((2 * radius + 1) * (2 * radius + 1));
    let variance = max(moments.z - moments.x * moments.x, 0.0);
    let covariance = moments.w - moments.x * moments.y;
    let a = covariance / (variance + 0.0001);
    let b = moments.y - a * moments.x;
    textureStore(destination, pos, vec4<f32>(a, b, 0.0, 1.0));
}

fn apply_amount(rgb: vec3<f32>, ambient: vec3<f32>, dark: f32, value: f32) -> vec3<f32> {
    let amount = clamp(value / 100.0, -1.0, 1.0);
    if abs(amount) <= 1e-6 { return rgb; }
    // A constant white/sky region is ambiguous under DCP. Gradually reduce its
    // inferred veil as it approaches airlight instead of turning it grey.
    let confidence = 1.0 - smoothstep(0.95, 1.0, dark);
    let estimated_t = max(1.0 - 0.95 * dark * confidence, 0.15);
    if amount < 0.0 {
        let t = pow(min(estimated_t, 0.65), -amount);
        return mix(ambient, rgb, t);
    }
    // Sub-cell images have no reliable global atmospheric estimate.
    if Tonemap::tone_stats.airlight.w <= 0.0 { return rgb; }
    let requested_veil = 1.0 - pow(estimated_t, amount);
    // Bound subtraction by the available signal in every channel. This avoids
    // colored clipping at depth edges and in saturated objects without a
    // separate saturation boost or per-channel transmission.
    let veil = min(requested_veil, 0.98 * dark_ratio(rgb, ambient));
    return (rgb - ambient * veil) / max(1.0 - veil, 0.15);
}

@compute @workgroup_size(8, 8, 1)
fn restore_haze(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(gid.xy);
    let radius = guide_radius();
    var coefficients = vec2<f32>(0.0);
    for (var y = -radius; y <= radius; y++) {
        for (var x = -radius; x <= radius; x++) {
            coefficients += auxiliary_at(pos + vec2<i32>(x, y)).xy;
        }
    }
    coefficients /= f32((2 * radius + 1) * (2 * radius + 1));
    let ambient = airlight();
    var rgb = source_at(pos);
    let dark = clamp(coefficients.x * guide(rgb, ambient) + coefficients.y, 0.0, 1.0);
    rgb = apply_amount(rgb, ambient, dark, Common::effects_uniforms.presence.z);
    let count = min(Common::scene_tone_uniforms.mask_counts.x, 32u);
    for (var index = 0u; index < count; index++) {
        let state = Common::mask_data[index].metadata;
        if state.x == 0u || state.y == 0u || Common::mask_effect_id(state) != 0u { continue; }
        let value = Common::mask_data[index].adjust_2_field.w;
        if abs(value) <= 1e-6 { continue; }
        let weight = SceneAdjustments::local_mask_weight(pos, index);
        if weight <= 1e-5 { continue; }
        rgb = mix(rgb, apply_amount(rgb, ambient, dark, value), weight);
    }
    textureStore(destination, pos, vec4<f32>(rgb, 1.0));
}

@compute @workgroup_size(8, 8, 1)
fn copy_dehaze(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(gid.xy);
    textureStore(destination, pos, vec4<f32>(SceneAdjustments::apply_local_exposure_nodes(pos, source_at(pos)), 1.0));
}
