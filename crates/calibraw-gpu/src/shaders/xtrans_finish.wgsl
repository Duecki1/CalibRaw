// SPDX-License-Identifier: GPL-3.0-or-later
// Adapted from darktable 5.6.0 Markesteijn and 5.6.1 dual demosaicing.
// Copyright (C) 2010-2026 darktable developers.
// Credits include Frank Markesteijn, Ingo Weyrich, and Hanno Schwalm.
// Copyright (C) 2026 CalibRaw contributors (WGSL adaptation).

#import calibraw::common as Common
#import calibraw::noise_ca_finish as NoiseCaFinish
#import calibraw::dual_demosaic as Dual

@group(0) @binding(26) var mark_high_read: texture_2d<f32>;
@group(0) @binding(10) var xtrans_scene_write: texture_storage_2d<rgba16float /* CALIBRAW_WORK_FORMAT */, write>;

fn xt_high(pos: vec2<i32>) -> vec3<f32> {
    return textureLoad(mark_high_read, Common::clamp_pos(pos), 0).rgb;
}

fn xt_uv(rgb: vec3<f32>) -> vec2<f32> {
    let y = 0.2627 * rgb.r + 0.6780 * rgb.g + 0.0593 * rgb.b;
    return vec2<f32>(0.56433 * (rgb.b - y), 0.67815 * (rgb.r - y));
}

fn xt_phase6(offset: i32) -> vec2<f32> {
    let phase = ((offset % 6) + 6) % 6;
    switch phase {
        case 0: { return vec2<f32>( 1.0,  0.0); }
        case 1: { return vec2<f32>( 0.5,  0.8660254); }
        case 2: { return vec2<f32>(-0.5,  0.8660254); }
        case 3: { return vec2<f32>(-1.0,  0.0); }
        case 4: { return vec2<f32>(-0.5, -0.8660254); }
        default: { return vec2<f32>(0.5, -0.8660254); }
    }
}

fn xt_complex_mul(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(a.x * b.x - a.y * b.y, a.x * b.y + a.y * b.x);
}

fn xt_complex_conj(a: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(a.x, -a.y);
}

fn xt_carrier_term(delta: vec2<f32>, phase: vec2<f32>, weight: f32) -> vec4<f32> {
    return weight * vec4<f32>(
        delta.x * phase.x,
        delta.x * phase.y,
        delta.y * phase.x,
        delta.y * phase.y,
    );
}

fn xt_frequency_uv(pos: vec2<i32>) -> vec2<f32> {
    let center_rgb = xt_high(pos);
    let center_opponents = xt_uv(center_rgb);
    var low_sum = vec2<f32>(0.0);
    var low_weight = 0.0;
    var carrier_x = vec4<f32>(0.0);
    var carrier_y = vec4<f32>(0.0);
    var carrier_diag = vec4<f32>(0.0);
    var carrier_antidiag = vec4<f32>(0.0);
    var carrier_weight = 0.0;

    for (var dy = -6; dy <= 6; dy = dy + 1) {
        let wy = f32(7 - abs(dy));
        let py = xt_phase6(dy);
        for (var dx = -6; dx <= 6; dx = dx + 1) {
            let wx = f32(7 - abs(dx));
            let weight = wx * wy;
            let uv = xt_uv(xt_high(pos + vec2<i32>(dx, dy)));
            let delta = uv - center_opponents;
            let px = xt_phase6(dx);
            let pdiag = xt_complex_mul(px, py);
            let panti = xt_complex_mul(px, xt_complex_conj(py));
            low_sum += uv * weight;
            low_weight += weight;
            carrier_x += xt_carrier_term(delta, px, weight);
            carrier_y += xt_carrier_term(delta, py, weight);
            carrier_diag += xt_carrier_term(delta, pdiag, weight);
            carrier_antidiag += xt_carrier_term(delta, panti, weight);
            carrier_weight += weight;
        }
    }

    let low = low_sum / max(low_weight, 1e-6);
    let inv = 1.0 / max(carrier_weight, 1e-6);
    let carrier_alias = 0.25 * inv * vec2<f32>(
        carrier_x.x + carrier_y.x + carrier_diag.x + carrier_antidiag.x,
        carrier_x.z + carrier_y.z + carrier_diag.z + carrier_antidiag.z,
    );
    let center_signal = dot(center_rgb, vec3<f32>(0.2627, 0.6780, 0.0593));
    let low_rgb = NoiseCaFinish::finish_from_yuv(center_signal, low);
    let luma_support = abs(center_signal - dot(low_rgb, vec3<f32>(0.2627, 0.6780, 0.0593)));
    let spectral_energy = length(carrier_alias);
    let reject = smoothstep(0.0015, 0.030, max(spectral_energy - 0.35 * luma_support, 0.0));
    let corrected = center_opponents - reject * carrier_alias;
    return mix(center_opponents, corrected, clamp(Common::camera_uniforms.frequency_chroma, 0.0, 1.0));
}

fn xt_median5(a0: f32, a1: f32, a2: f32, a3: f32, a4: f32) -> f32 {
    var a = a0;
    var b = a1;
    var c = a2;
    var d = a3;
    var e = a4;
    var t = 0.0;
    if a > b { t = a; a = b; b = t; }
    if d > e { t = d; d = e; e = t; }
    if a > d { t = a; a = d; d = t; }
    if b > e { t = b; b = e; e = t; }
    if b > c { t = b; b = c; c = t; }
    if c > d { t = c; c = d; d = t; }
    if b > c { t = b; b = c; c = t; }
    return c;
}

fn xt_reference_false_color_guard(pos: vec2<i32>, rgb: vec3<f32>) -> vec3<f32> {
    let uv0 = xt_uv(rgb);
    let uvn = xt_uv(xt_high(pos + vec2<i32>(0, -1)));
    let uvs = xt_uv(xt_high(pos + vec2<i32>(0,  1)));
    let uvw = xt_uv(xt_high(pos + vec2<i32>(-1, 0)));
    let uve = xt_uv(xt_high(pos + vec2<i32>( 1, 0)));
    let median = vec2<f32>(
        xt_median5(uv0.x, uvn.x, uvs.x, uvw.x, uve.x),
        xt_median5(uv0.y, uvn.y, uvs.y, uvw.y, uve.y),
    );
    let disagreement = length(uv0 - median);
    let strength = NoiseCaFinish::finish_false_color_strength(rgb, disagreement, 0.50);
    if strength <= 1e-6 { return rgb; }
    let y = dot(rgb, vec3<f32>(0.2627, 0.6780, 0.0593));
    return NoiseCaFinish::finish_from_yuv(y, mix(uv0, median, strength));
}

fn xt_frequency_chroma(pos: vec2<i32>, rgb: vec3<f32>) -> vec3<f32> {
    let uv0 = xt_frequency_uv(pos);
    let uvn = xt_uv(xt_high(pos + vec2<i32>(0, -1)));
    let uvs = xt_uv(xt_high(pos + vec2<i32>(0,  1)));
    let uvw = xt_uv(xt_high(pos + vec2<i32>(-1, 0)));
    let uve = xt_uv(xt_high(pos + vec2<i32>( 1, 0)));
    let median = vec2<f32>(
        xt_median5(uv0.x, uvn.x, uvs.x, uvw.x, uve.x),
        xt_median5(uv0.y, uvn.y, uvs.y, uvw.y, uve.y),
    );
    let strength = clamp(Common::camera_uniforms.frequency_chroma, 0.0, 1.0);
    let uv = mix(xt_uv(rgb), mix(uv0, median, 0.35), strength);
    let y = dot(rgb, vec3<f32>(0.2627, 0.6780, 0.0593));
    return NoiseCaFinish::finish_from_yuv(y, uv);
}

override fn NoiseCaFinish::finish_reference_at(pos: vec2<i32>) -> vec3<f32> {
    return xt_high(pos);
}

// Pass 1 of dual demosaicing (dual_demosaic.wgsl), with Markesteijn as the
// reference.
@compute @workgroup_size(8, 8, 1)
fn xtrans_dual_prepare(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    Dual::dual_store_prepared(pos, xt_high(pos), false);
}

@compute @workgroup_size(8, 8, 1)
fn xtrans_demosaic_finish(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    let reference = xt_high(pos);
    var camera_rgb = reference;
    if Common::camera_uniforms.demosaic_mode >= 1.5 {
        camera_rgb = Dual::dual_blend(pos, reference);
    } else if Common::camera_uniforms.demosaic_mode >= 0.5 {
        camera_rgb = xt_frequency_chroma(pos, reference);
    } else {
        camera_rgb = xt_reference_false_color_guard(pos, reference);
    }
    camera_rgb = NoiseCaFinish::finish_apply_sensor_denoise(pos, camera_rgb);
    textureStore(xtrans_scene_write, pos, vec4<f32>(camera_rgb, 1.0));
}
