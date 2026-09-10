#import calibraw::common as Common
#import calibraw::color as Color
#import calibraw::profile as Profile
#import calibraw::basic_adjustments as BasicAdjustments
#import calibraw::tone_common as ToneCommon


struct ToneHistogram {
    bins: array<atomic<u32>, 256>,
    air_count: array<atomic<u32>, 256>,
    air_r: array<atomic<u32>, 256>,
    air_g: array<atomic<u32>, 256>,
    air_b: array<atomic<u32>, 256>,
    air_r_carry: array<atomic<u32>, 256>,
    air_g_carry: array<atomic<u32>, 256>,
    air_b_carry: array<atomic<u32>, 256>,
}

@group(0) @binding(11) var tone_scene_tex: texture_2d<f32>;
@group(0) @binding(15) var<storage, read_write> tone_histogram: ToneHistogram;
@group(0) @binding(16) var<storage, read_write> tone_stats_out: ToneCommon::ToneStats;
@group(0) @binding(17) var tone_guide_read: texture_2d<f32>;
@group(0) @binding(18) var tone_guide_write: texture_storage_2d<r32float, write>;

fn tone_raster_ca_warped_pos(pos: vec2<i32>, amount: f32) -> vec2<f32> {
    let local_extent = vec2<f32>(
        f32(Common::camera_uniforms.width - 1u),
        f32(Common::camera_uniforms.height - 1u),
    );
    let origin = vec2<f32>(
        f32(Common::camera_uniforms.tile_origin_x),
        f32(Common::camera_uniforms.tile_origin_y),
    );
    let full_extent = vec2<f32>(
        f32(Common::camera_uniforms.full_width - 1u),
        f32(Common::camera_uniforms.full_height - 1u),
    );
    let center = 0.5 * full_extent;
    let global_pos = vec2<f32>(pos) + origin;
    let rel = global_pos - center;
    let norm = rel / max(center, vec2<f32>(1.0));
    let scale = 1.0 + amount * 0.001 * dot(norm, norm);
    let warped_global = clamp(center + rel * scale, vec2<f32>(0.0), full_extent);
    return clamp(warped_global - origin, vec2<f32>(0.0), local_extent);
}

fn tone_raster_scene_bilinear(pos: vec2<f32>) -> vec3<f32> {
    let base = floor(pos);
    let p0 = vec2<i32>(i32(base.x), i32(base.y));
    let p1 = p0 + vec2<i32>(1, 1);
    let f = fract(pos);
    let a = textureLoad(tone_scene_tex, Common::clamp_pos(p0), 0).xyz;
    let b = textureLoad(tone_scene_tex, Common::clamp_pos(vec2<i32>(p1.x, p0.y)), 0).xyz;
    let c = textureLoad(tone_scene_tex, Common::clamp_pos(vec2<i32>(p0.x, p1.y)), 0).xyz;
    let d = textureLoad(tone_scene_tex, Common::clamp_pos(p1), 0).xyz;
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

fn tone_source_scene_at(pos: vec2<i32>) -> vec3<f32> {
    var rgb = textureLoad(tone_scene_tex, Common::clamp_pos(pos), 0).xyz;
    if Common::camera_uniforms._pad_0_field <= 0.5 {
        return rgb;
    }
    if abs(Common::camera_uniforms.ca_red) > 1e-6 {
        rgb.r = tone_raster_scene_bilinear(
            tone_raster_ca_warped_pos(pos, Common::camera_uniforms.ca_red),
        ).r;
    }
    if abs(Common::camera_uniforms.ca_blue) > 1e-6 {
        rgb.b = tone_raster_scene_bilinear(
            tone_raster_ca_warped_pos(pos, Common::camera_uniforms.ca_blue),
        ).b;
    }
    return rgb;
}

fn tone_unexposed_working_at(pos: vec2<i32>) -> vec3<f32> {
    let camera_rgb = tone_source_scene_at(pos);

    var working = Color::cam_to_working(camera_rgb);
    if Common::camera_uniforms._pad_0_field > 0.5 {
        working = BasicAdjustments::apply_temperature_tint_values(
            working,
            Common::camera_uniforms.temperature,
            Common::camera_uniforms.tint,
        );
    }
    let characterized = Profile::apply_camera_characterization(working);
    let profile_exposure_ev = bitcast<f32>(Common::camera_uniforms.profile_flags.z);
    let exposed = characterized * exp2(profile_exposure_ev);
    return Color::map_negative_gamut(exposed);
}

fn tone_guide_cell_size() -> i32 {
    return max(i32(round(Common::camera_uniforms.tone_analysis_scale)), 1);
}

fn tone_guide_origin_cell() -> vec2<i32> {
    let scale = f32(tone_guide_cell_size());
    return vec2<i32>(floor(vec2<f32>(Common::tile_origin()) / scale));
}

@compute @workgroup_size(8, 8, 1)
fn tone_guide_prepare(@builtin(global_invocation_id) gid: vec3<u32>) {
    let guide_size = textureDimensions(tone_guide_write);
    if gid.x >= guide_size.x || gid.y >= guide_size.y { return; }

    let source_size = vec2<i32>(
        i32(Common::camera_uniforms.width),
        i32(Common::camera_uniforms.height),
    );
    let cell_size = tone_guide_cell_size();
    let tile_origin = Common::tile_origin();
    let global_cell = tone_guide_origin_cell() + vec2<i32>(gid.xy);
    let global_cell_min = global_cell * cell_size;
    let global_cell_max = global_cell_min + vec2<i32>(cell_size);
    let cell_min = clamp(global_cell_min - tile_origin, vec2<i32>(0), source_size);
    let cell_max = clamp(global_cell_max - tile_origin, vec2<i32>(0), source_size);

    var air_sum = vec3<f32>(0.0);
    var air_dark = 1e20;
    var air_count = 0.0;
    var log_sum = 0.0;
    var count = 0.0;
    var brightest = vec4<f32>(ToneCommon::TONE_EV_MIN);
    var y = cell_min.y;
    loop {
        if y >= cell_max.y { break; }
        var x = cell_min.x;
        loop {
            if x >= cell_max.x { break; }
            let local_pos = vec2<i32>(x, y);
            let rgb = tone_unexposed_working_at(local_pos);
            let ev = clamp(
                log2(Common::safe_luma(rgb) / ToneCommon::SCENE_MIDDLE_GREY),
                ToneCommon::TONE_EV_MIN,
                ToneCommon::TONE_EV_MAX,
            );
            log_sum = log_sum + ev;
            if ev > brightest.x {
                brightest = vec4<f32>(ev, brightest.x, brightest.y, brightest.z);
            } else if ev > brightest.y {
                brightest = vec4<f32>(brightest.x, ev, brightest.y, brightest.z);
            } else if ev > brightest.z {
                brightest = vec4<f32>(brightest.x, brightest.y, ev, brightest.z);
            } else if ev > brightest.w {
                brightest = vec4<f32>(brightest.x, brightest.y, brightest.z, ev);
            }
            count = count + 1.0;

            let global_pos = local_pos + tile_origin;
            let histogram_min = vec2<i32>(Common::camera_uniforms.tone_histogram_bounds.xy);
            let histogram_max = vec2<i32>(Common::camera_uniforms.tone_histogram_bounds.zw);
            if all(global_pos >= histogram_min) && all(global_pos < histogram_max) {
                atomicAdd(&tone_histogram.bins[ToneCommon::tone_ev_to_bin(ev)], 1u);
                let positive = max(rgb, vec3<f32>(0.0));
                air_sum += positive;
                air_dark = min(air_dark, min(positive.r, min(positive.g, positive.b)));
                air_count += 1.0;
            }
            x = x + 1;
        }
        y = y + 1;
    }

    // Only complete globally aligned cells contribute. Tile halos cannot add
    // duplicate candidates; bounds select the core in tiled export.
    if air_count > 0.0 && air_count == f32(cell_size * cell_size) {
        let bin = ToneCommon::tone_ev_to_bin(log2(max(air_dark, 1e-6) / ToneCommon::SCENE_MIDDLE_GREY));
        let scale = ToneCommon::SCENE_MIDDLE_GREY * exp2(ToneCommon::tone_bin_to_ev(bin));
        let encoded = vec3<u32>(round(clamp(air_sum / air_count / scale, vec3<f32>(0.0), vec3<f32>(64.0)) * 4096.0));
        atomicAdd(&tone_histogram.air_count[bin], 1u);
        let r = atomicAdd(&tone_histogram.air_r[bin], encoded.r);
        let g = atomicAdd(&tone_histogram.air_g[bin], encoded.g);
        let b = atomicAdd(&tone_histogram.air_b[bin], encoded.b);
        if r > 0xffffffffu - encoded.r { atomicAdd(&tone_histogram.air_r_carry[bin], 1u); }
        if g > 0xffffffffu - encoded.g { atomicAdd(&tone_histogram.air_g_carry[bin], 1u); }
        if b > 0xffffffffu - encoded.b { atomicAdd(&tone_histogram.air_b_carry[bin], 1u); }
    }

    let average_ev = log_sum / max(count, 1.0);

    let bright_count = min(count, 4.0);
    var bright_sum = brightest.x;
    if bright_count > 1.0 { bright_sum = bright_sum + brightest.y; }
    if bright_count > 2.0 { bright_sum = bright_sum + brightest.z; }
    if bright_count > 3.0 { bright_sum = bright_sum + brightest.w; }
    let robust_bright_ev = bright_sum / max(bright_count, 1.0);
    let guide_ev = max(average_ev, robust_bright_ev - 1.5);
    textureStore(tone_guide_write, vec2<i32>(gid.xy), vec4<f32>(guide_ev, 0.0, 0.0, 1.0));
}

fn tone_bilateral_guide(pos: vec2<i32>, axis: vec2<i32>) -> f32 {
    let guide_max = vec2<i32>(textureDimensions(tone_guide_read)) - vec2<i32>(1);
    let center_pos = clamp(pos, vec2<i32>(0), guide_max);
    let center = textureLoad(tone_guide_read, center_pos, 0).x;
    let radius = clamp(i32(round(Common::camera_uniforms.tone_guide_radius)), 1, 6);
    let sigma = max(f32(radius) * 0.65, 1.0);

    var weighted_sum = 0.0;
    var weight_sum = 0.0;
    for (var offset = -6; offset <= 6; offset = offset + 1) {
        if abs(offset) > radius { continue; }
        let sample_pos = clamp(center_pos + axis * offset, vec2<i32>(0), guide_max);
        let sample_ev = textureLoad(tone_guide_read, sample_pos, 0).x;
        let distance = f32(offset);
        let spatial_weight = exp(-0.5 * distance * distance / (sigma * sigma));
        let range_weight = exp(-1.35 * abs(sample_ev - center));
        let weight = spatial_weight * range_weight;
        weighted_sum = weighted_sum + sample_ev * weight;
        weight_sum = weight_sum + weight;
    }

    return weighted_sum / max(weight_sum, 1e-6);
}

@compute @workgroup_size(8, 8, 1)
fn tone_guide_horizontal(@builtin(global_invocation_id) gid: vec3<u32>) {
    let guide_size = textureDimensions(tone_guide_write);
    if gid.x >= guide_size.x || gid.y >= guide_size.y { return; }
    let pos = vec2<i32>(gid.xy);
    let value = tone_bilateral_guide(pos, vec2<i32>(1, 0));
    textureStore(tone_guide_write, pos, vec4<f32>(value, 0.0, 0.0, 1.0));
}

@compute @workgroup_size(8, 8, 1)
fn tone_guide_vertical(@builtin(global_invocation_id) gid: vec3<u32>) {
    let guide_size = textureDimensions(tone_guide_write);
    if gid.x >= guide_size.x || gid.y >= guide_size.y { return; }
    let pos = vec2<i32>(gid.xy);
    let value = tone_bilateral_guide(pos, vec2<i32>(0, 1));
    textureStore(tone_guide_write, pos, vec4<f32>(value, 0.0, 0.0, 1.0));
}

fn estimate_airlight() -> vec4<f32> {
    var total = 0u;
    for (var bin = 0u; bin < 256u; bin++) { total += atomicLoad(&tone_histogram.air_count[bin]); }
    if total == 0u { return vec4<f32>(1.0, 1.0, 1.0, 0.0); }
    let candidate_target = max(1u, u32(ceil(f32(total) * 0.001)));
    var count = 0u;
    var sum = vec3<f32>(0.0);
    for (var bin = 255i; bin >= 0; bin--) {
        let n = atomicLoad(&tone_histogram.air_count[bin]);
        let low = vec3<f32>(f32(atomicLoad(&tone_histogram.air_r[bin])), f32(atomicLoad(&tone_histogram.air_g[bin])), f32(atomicLoad(&tone_histogram.air_b[bin])));
        let high = vec3<f32>(f32(atomicLoad(&tone_histogram.air_r_carry[bin])), f32(atomicLoad(&tone_histogram.air_g_carry[bin])), f32(atomicLoad(&tone_histogram.air_b_carry[bin])));
        let scale = ToneCommon::SCENE_MIDDLE_GREY * exp2(ToneCommon::tone_bin_to_ev(u32(bin)));
        sum += (low + high * 4294967296.0) * (scale / 4096.0);
        count += n;
        if count >= candidate_target { break; }
    }
    return vec4<f32>(max(sum / f32(count), vec3<f32>(1e-6)), f32(count));
}

@compute @workgroup_size(1, 1, 1)
fn tone_reduce_histogram(@builtin(global_invocation_id) gid: vec3<u32>) {
    if any(gid != vec3<u32>(0u)) { return; }

    tone_stats_out.airlight = estimate_airlight();
    var total = 0u;
    for (var index = 0u; index < ToneCommon::TONE_HISTOGRAM_BIN_COUNT; index = index + 1u) {
        total = total + atomicLoad(&tone_histogram.bins[index]);
    }

    if total == 0u {
        tone_stats_out.percentiles_0_field = vec4<f32>(-8.0, -5.0, 0.0, 2.5);
        tone_stats_out.percentiles_1_field = vec4<f32>(4.0, 12.0, 0.0, 0.0);
        return;
    }

    let target_005 = max(1u, u32(ceil(f32(total) * 0.005)));
    let target_05 = max(1u, u32(ceil(f32(total) * 0.05)));
    let target_50 = max(1u, u32(ceil(f32(total) * 0.50)));
    let target_95 = max(1u, u32(ceil(f32(total) * 0.95)));
    let target_995 = max(1u, u32(ceil(f32(total) * 0.995)));

    var cumulative = 0u;
    var p005_field = ToneCommon::TONE_EV_MIN;
    var p05_field = ToneCommon::TONE_EV_MIN;
    var p50_field = 0.0;
    var p95_field = ToneCommon::TONE_EV_MAX;
    var p995_field = ToneCommon::TONE_EV_MAX;
    var found_005 = false;
    var found_05 = false;
    var found_50 = false;
    var found_95 = false;
    var found_995 = false;

    for (var index = 0u; index < ToneCommon::TONE_HISTOGRAM_BIN_COUNT; index = index + 1u) {
        cumulative = cumulative + atomicLoad(&tone_histogram.bins[index]);
        let ev = ToneCommon::tone_bin_to_ev(index);
        if !found_005 && cumulative >= target_005 { p005_field = ev; found_005 = true; }
        if !found_05 && cumulative >= target_05 { p05_field = ev; found_05 = true; }
        if !found_50 && cumulative >= target_50 { p50_field = ev; found_50 = true; }
        if !found_95 && cumulative >= target_95 { p95_field = ev; found_95 = true; }
        if !found_995 && cumulative >= target_995 { p995_field = ev; found_995 = true; }
    }

    tone_stats_out.percentiles_0_field = vec4<f32>(p005_field, p05_field, p50_field, p95_field);
    tone_stats_out.percentiles_1_field = vec4<f32>(p995_field, max(p995_field - p005_field, 1.0), f32(total), 0.0);
}
