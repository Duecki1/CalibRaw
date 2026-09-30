// Light components are sampled by Fog's volume integrator, independent of
// component order. Positions and ranges use the full image, including tiles.
fn light_beam_active(index: u32) -> bool {
    let state = Common::mask_data[index].metadata;
    return state.x != 0u && state.y != 0u
        && Common::mask_effect_id(state) == MASK_EFFECT_LIGHT_BEAMS_ID;
}

fn light_beams_present(pos: vec2<i32>) -> bool {
    let count = min(Common::scene_tone_uniforms.mask_counts.x, Common::MAX_RENDER_MASK_SLOTS);
    for (var index = 0u; index < count; index += 1u) {
        if light_beam_active(index) && SceneAdjustments::local_mask_weight(pos, index) > 1e-5 {
            return true;
        }
    }
    return false;
}

fn light_beam_incident(point: vec3<f32>, index: u32, extinction: f32) -> vec3<f32> {
    let primary = Common::mask_data[index].adjust_0_field;
    let secondary = Common::mask_data[index].adjust_1_field;
    let tertiary = Common::mask_data[index].adjust_2_field;
    let depth = max(tertiary.z / 100.0, 0.02);
    let aspect = f32(Common::camera_uniforms.full_width) / max(f32(Common::camera_uniforms.full_height), 1.0);
    let source = vec3<f32>((primary.zw / 100.0 - vec2<f32>(0.5)) * vec2<f32>(aspect, 1.0) * 1.25 * depth, depth);
    let range = max(primary.y / 100.0 * 1.25 * depth, 0.001);
    let angle = radians(secondary.w);
    let axis = vec3<f32>(cos(angle), sin(angle), 0.0);
    let delta = point - source;
    let along = dot(delta, axis);
    let travel = length(delta);
    let radius = max(along, 0.0) * tan(radians(tertiary.x * 0.5));
    let across = length(delta - along * axis);
    let softness = clamp(tertiary.y / 100.0, 0.0, 1.0);
    let edge = max(radius * mix(0.05, 0.9, softness), range * 0.008);
    let cone = (1.0 - smoothstep(max(radius - edge, 0.0), max(radius, edge), across))
        * smoothstep(0.0, range * 0.015, along)
        * (1.0 - smoothstep(range * 0.65, range, travel));
    // A spherical pool spreads on every side of a lamp, including above and
    // behind it. It is separate from the directional cone width.
    let pool_radius = range * mix(0.08, 0.5, tertiary.w / 100.0);
    let pool = tertiary.w / 100.0 * exp(-2.0 * travel * travel / (pool_radius * pool_radius))
        * (1.0 - smoothstep(pool_radius, pool_radius * 2.0, travel));
    let attenuation = exp(-extinction * travel) / (1.0 + 6.0 * travel * travel / (range * range));
    let radiance = 2.5 * primary.x / 100.0 * (cone + pool) * attenuation;
    return mask_effect_picker_color_to_working(secondary.xyz) * radiance;
}

fn light_beams_incident(pos: vec2<i32>, point: vec3<f32>, extinction: f32) -> vec3<f32> {
    var light = vec3<f32>(0.0);
    let count = min(Common::scene_tone_uniforms.mask_counts.x, Common::MAX_RENDER_MASK_SLOTS);
    for (var index = 0u; index < count; index += 1u) {
        if !light_beam_active(index) { continue; }
        let weight = SceneAdjustments::local_mask_weight(pos, index);
        if weight <= 1e-5 { continue; }
        light += light_beam_incident(point, index, extinction) * weight;
    }
    return light;
}

fn fog_coverage_at(pos: vec2<i32>) -> f32 {
    var coverage = 0.0;
    let count = min(Common::scene_tone_uniforms.mask_counts.x, Common::MAX_RENDER_MASK_SLOTS);
    for (var index = 0u; index < count; index += 1u) {
        let state = Common::mask_data[index].metadata;
        if state.x == 0u || state.y == 0u || Common::mask_effect_id(state) != MASK_EFFECT_FOG_ID { continue; }
        coverage = max(coverage, SceneAdjustments::local_mask_weight(pos, index));
    }
    return coverage;
}

fn apply_light_beam_fallback(pos: vec2<i32>, rgb: vec3<f32>, index: u32) -> vec3<f32> {
    let uncovered = 1.0 - fog_coverage_at(pos);
    if uncovered <= 1e-5 { return rgb; }
    var distance = 0.35;
    if Common::scene_tone_uniforms.scene_depth_present != 0u { distance = fog_depth_at(pos); }
    let ray = vec3<f32>(atmosphere_image_point(pos) * 1.25, 1.0);
    let step = 1.0 / 72.0;
    let extinction = 0.65;
    var light = vec3<f32>(0.0);
    for (var i = 0u; i < 72u; i += 1u) {
        let lo = f32(i) * step;
        let hi = min(lo + step, distance);
        if hi <= lo { break; }
        let t = 0.5 * (lo + hi);
        let segment_depth = extinction * (hi - lo) * length(ray);
        light += light_beam_incident(ray * t, index, extinction)
            * exp(-extinction * lo * length(ray)) * (1.0 - exp(-segment_depth));
    }
    // The calling node blends this component's mask once; Fog uses the same
    // mask when sampling the light inside its own density field.
    return rgb + light * uncovered;
}
