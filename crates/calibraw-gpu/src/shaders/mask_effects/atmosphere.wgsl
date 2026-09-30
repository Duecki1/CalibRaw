
fn atmosphere_hash(position: vec2<f32>) -> f32 {
    let signed_cell = vec2<i32>(position);
    let cell = bitcast<vec2<u32>>(signed_cell);
    var state = cell.x * 1597334677u ^ cell.y * 3812015801u;
    state = (state ^ (state >> 16u)) * 2246822519u;
    state = (state ^ (state >> 13u)) * 3266489917u;
    state = state ^ (state >> 16u);
    return f32(state & 0x00ffffffu) / 16777215.0;
}

fn atmosphere_noise(position: vec2<f32>) -> f32 {
    let cell = floor(position);
    let fraction = fract(position);
    let smooth_fraction = fraction * fraction * (vec2<f32>(3.0) - 2.0 * fraction);
    let bottom = mix(
        atmosphere_hash(cell),
        atmosphere_hash(cell + vec2<f32>(1.0, 0.0)),
        smooth_fraction.x,
    );
    let top = mix(
        atmosphere_hash(cell + vec2<f32>(0.0, 1.0)),
        atmosphere_hash(cell + vec2<f32>(1.0, 1.0)),
        smooth_fraction.x,
    );
    return mix(bottom, top, smooth_fraction.y);
}

fn atmosphere_fbm(position: vec2<f32>) -> f32 {
    var point = position;
    var amplitude = 0.5;
    var value = 0.0;
    var normalization = 0.0;
    for (var octave = 0u; octave < 5u; octave = octave + 1u) {
        value = value + atmosphere_noise(point) * amplitude;
        normalization = normalization + amplitude;
        point = vec2<f32>(
            point.x * 1.62 + point.y * 1.17,
            point.x * -1.17 + point.y * 1.62,
        ) + vec2<f32>(13.7, 9.2);
        amplitude = amplitude * 0.52;
    }
    return value / max(normalization, 1e-6);
}

fn atmosphere_image_point(pos: vec2<i32>) -> vec2<f32> {
    let full_size = max(
        vec2<f32>(
            f32(Common::camera_uniforms.full_width),
            f32(Common::camera_uniforms.full_height),
        ),
        vec2<f32>(1.0),
    );
    var point = full_image_uv(pos) - vec2<f32>(0.5);
    point.x = point.x * full_size.x / full_size.y;
    return point;
}

// Full-image depth is shared by all fog components; mask coverage remains a
// separate final blend. DA3 is relative distance (near=0, far=1), not disparity.
@group(0) @binding(35) var scene_depth_tex: texture_2d<f32>;

fn fog_depth_at(pos: vec2<i32>) -> f32 {
    let size = vec2<i32>(textureDimensions(scene_depth_tex));
    let p = full_image_uv(pos) * vec2<f32>(size) - vec2<f32>(0.5);
    let base = vec2<i32>(floor(p));
    let f = fract(p);
    let full_size = vec2<f32>(
        f32(Common::camera_uniforms.full_width),
        f32(Common::camera_uniforms.full_height),
    );
    let center = sqrt(max(SceneAdjustments::local_effects_at(pos), vec3<f32>(0.0)));
    var total = 0.0;
    var weights = 0.0;
    // Joint upsampling rejects samples across image edges instead of blurring
    // background depth into foreground silhouettes. No depth-range mask curve
    // is applied here: a selection is not a measurement of distance.
    for (var y = 0; y < 2; y = y + 1) {
        for (var x = 0; x < 2; x = x + 1) {
            let cell = clamp(base + vec2<i32>(x, y), vec2<i32>(0), size - vec2<i32>(1));
            let uv = (vec2<f32>(cell) + vec2<f32>(0.5)) / vec2<f32>(size);
            let guide_pos = uv * full_size - vec2<f32>(0.5) - vec2<f32>(Common::tile_origin());
            let guide = sqrt(max(mask_effect_source_linear_at(guide_pos), vec3<f32>(0.0)));
            let delta = (guide - center) / max(length(center), 0.15);
            let spatial = select(1.0 - f.x, f.x, x == 1) * select(1.0 - f.y, f.y, y == 1);
            let weight = spatial * max(exp(-dot(delta, delta) * 24.0), 0.0001);
            total += textureLoad(scene_depth_tex, cell, 0).x * weight;
            weights += weight;
        }
    }
    return clamp(total / max(weights, 1e-6), 0.0, 1.0);
}

fn fog_hash3(cell: vec3<i32>) -> f32 {
    let p = bitcast<vec3<u32>>(cell);
    var h = p.x * 1597334677u ^ p.y * 3812015801u ^ p.z * 2798796415u;
    h = (h ^ (h >> 16u)) * 2246822519u;
    h = (h ^ (h >> 13u)) * 3266489917u;
    h = h ^ (h >> 16u);
    return f32(h & 0x00ffffffu) / 16777215.0;
}

fn fog_noise3(point: vec3<f32>) -> f32 {
    let cell = vec3<i32>(floor(point));
    let f = fract(point);
    let u = f * f * f * (f * (f * 6.0 - vec3<f32>(15.0)) + vec3<f32>(10.0));
    let a = mix(
        mix(fog_hash3(cell), fog_hash3(cell + vec3<i32>(1, 0, 0)), u.x),
        mix(fog_hash3(cell + vec3<i32>(0, 1, 0)), fog_hash3(cell + vec3<i32>(1, 1, 0)), u.x), u.y,
    );
    let b = mix(
        mix(fog_hash3(cell + vec3<i32>(0, 0, 1)), fog_hash3(cell + vec3<i32>(1, 0, 1)), u.x),
        mix(fog_hash3(cell + vec3<i32>(0, 1, 1)), fog_hash3(cell + vec3<i32>(1, 1, 1)), u.x), u.y,
    );
    return mix(a, b, u.z);
}

fn fog_onset_integral(distance: f32, start: f32, width: f32) -> f32 {
    let travel = max(distance - start, 0.0);
    let u = clamp(travel / width, 0.0, 1.0);
    return width * (u * u * u - 0.5 * u * u * u * u) + max(travel - width, 0.0);
}

fn apply_fog(
    pos: vec2<i32>,
    input_rgb: vec3<f32>,
    primary: vec4<f32>,
    secondary: vec4<f32>,
    tertiary: vec4<f32>,
) -> vec3<f32> {
    let amount = clamp(primary.x / 100.0, 0.0, 1.0);
    let density = clamp(primary.y / 100.0, 0.0, 1.0);
    if amount <= 1e-6 || density <= 1e-6 {
        return input_rgb;
    }
    let influence = clamp(tertiary.z / 100.0, 0.0, 1.0);
    // Until depth is generated, use a restrained constant-distance preview.
    // No screen-height or luminance heuristic pretends to know scene geometry.
    var distance = 0.35;
    if Common::scene_tone_uniforms.scene_depth_present != 0u && influence > 1e-6 {
        distance = fog_depth_at(pos);
    }
    distance = mix(1.0, distance, influence);
    let start = clamp(tertiary.y / 100.0, 0.0, 0.95);
    if distance <= start { return input_rgb; }

    let scale = clamp(primary.z / 100.0, 0.01, 1.0);
    let softness = clamp(primary.w / 100.0, 0.0, 1.0);
    let variation = clamp(secondary.w / 100.0, 0.0, 1.0);
    let seed = clamp(tertiary.x, 0.0, 1000.0);
    let offset = vec3<f32>(seed * 0.071 + 19.3, seed * -0.113 + 47.1, seed * 0.053 + 11.7);
    let frequency = mix(7.0, 1.6, scale);
    let image_point = atmosphere_image_point(pos);
    // A perspective volume in relative scene units. Integrate only to the
    // visible surface; foreground objects truncate the same volume as the
    // background. Fixed world-space intervals preserve shared ray prefixes.
    let ray = vec3<f32>(image_point * 1.25, 1.0);
    let step = 1.0 / 12.0;
    let onset_width = mix(0.025, 0.18, softness);
    let extinction = 6.0 * density * density * amount;
    let has_lights = light_beams_present(pos);
    var direct_scattering = vec3<f32>(0.0);
    var optical_length = fog_onset_integral(distance, start, onset_width);
    if variation > 1e-6 || has_lights {
        optical_length = 0.0;
        for (var i = 0u; i < 12u; i = i + 1u) {
            let lo = max(f32(i) * step, start);
            let cell_end = f32(i + 1u) * step;
            let hi = min(cell_end, distance);
            if hi <= lo { continue; }
            // A partial last interval uses the same density as the full interval,
            // so increasing surface distance can never remove accumulated fog.
            let t = 0.5 * (lo + cell_end);
            var bank_density = 1.0;
            if variation > 1e-6 {
                let point = ray * t * frequency + offset;
                let broad = fog_noise3(point);
                let detail = fog_noise3(point * 2.03 + vec3<f32>(7.1, -3.4, 13.8));
                let field = mix(broad, detail, mix(0.28, 0.08, softness));
                bank_density = exp2((field - 0.5) * variation * mix(5.0, 2.5, softness));
            }
            let segment = fog_onset_integral(hi, start, onset_width)
                - fog_onset_integral(lo, start, onset_width);
            if has_lights {
                // Resolve narrow cones inside each density interval while
                // preserving Fog's existing field and shared ray prefixes.
                for (var sample = 0u; sample < 6u; sample += 1u) {
                    let sub_lo = max(f32(i) * step + f32(sample) * step / 6.0, lo);
                    let sub_end = f32(i) * step + f32(sample + 1u) * step / 6.0;
                    let sub_hi = min(sub_end, hi);
                    if sub_hi <= sub_lo { continue; }
                    let prefix = optical_length + bank_density * (
                        fog_onset_integral(sub_lo, start, onset_width) - fog_onset_integral(lo, start, onset_width));
                    let sub_length = bank_density * (fog_onset_integral(sub_hi, start, onset_width)
                        - fog_onset_integral(sub_lo, start, onset_width));
                    let camera_transmission = exp(-extinction * prefix * length(ray));
                    let scattered = 1.0 - exp(-extinction * sub_length * length(ray));
                    let sample_t = 0.5 * (sub_lo + sub_hi);
                    direct_scattering += light_beams_incident(pos, ray * sample_t, extinction * bank_density)
                        * camera_transmission * scattered;
                }
            }
            optical_length += segment * bank_density;
        }
    }
    // Beer-Lambert extinction and constant-environment single scattering in
    // scene-linear Rec.2020. Amount changes concentration, not a screen overlay.
    // https://pbr-book.org/4ed/Volume_Scattering/Transmittance
    let optical_depth = extinction * optical_length * length(ray);
    let transmission = exp(-optical_depth);
    // Global tone statistics are shared by export tiles. Match airlight to
    // scene illumination, avoiding white self-luminous fog in dark photographs.
    let ambient_ev = Tonemap::tone_stats.percentiles_0_field.w + Common::scene_tone_uniforms.exposure;
    let ambient = ToneCommon::SCENE_MIDDLE_GREY * exp2(clamp(ambient_ev, -12.0, 6.0)) * 1.15;
    let airlight = mask_effect_picker_color_to_working(secondary.xyz) * ambient;
    return input_rgb * transmission + airlight * (1.0 - transmission) + direct_scattering;
}

fn apply_smoke(
    pos: vec2<i32>,
    input_rgb: vec3<f32>,
    primary: vec4<f32>,
    secondary: vec4<f32>,
    tertiary: vec4<f32>,
) -> vec3<f32> {
    let amount = clamp(primary.x / 100.0, 0.0, 1.0);
    let density = clamp(primary.y / 100.0, 0.0, 1.0);
    if amount <= 1e-6 || density <= 1e-6 {
        return input_rgb;
    }

    let angle = radians(clamp(secondary.w, -180.0, 180.0));
    let cosine = cos(angle);
    let sine = sin(angle);
    let image_point = atmosphere_image_point(pos);
    var point = vec2<f32>(
        cosine * image_point.x - sine * image_point.y,
        sine * image_point.x + cosine * image_point.y,
    );
    point = point * vec2<f32>(0.78, 1.18);

    let scale = clamp(primary.z / 100.0, 0.01, 1.0);
    let frequency = mix(11.0, 2.4, scale);
    let turbulence = clamp(primary.w / 100.0, 0.0, 1.0);
    let softness = clamp(tertiary.x / 100.0, 0.0, 1.0);
    let seed = clamp(tertiary.y, 0.0, 1000.0);
    let offset = vec2<f32>(seed * 0.097 + 31.6, seed * -0.067 + 8.9);

    let warp_point = point * frequency * 0.58 + offset;
    let warp = vec2<f32>(
        atmosphere_fbm(warp_point + vec2<f32>(0.0, 17.2)),
        atmosphere_fbm(warp_point + vec2<f32>(23.4, 0.0)),
    ) - vec2<f32>(0.5);
    let warped = point * frequency + offset
        + warp * mix(0.10, 2.35, turbulence);
    let body = atmosphere_fbm(warped);
    let ridge_noise = atmosphere_noise(warped * 2.15 + vec2<f32>(5.7, 12.9));
    let ridges = 1.0 - abs(ridge_noise * 2.0 - 1.0);
    let field = mix(body, body * 0.78 + ridges * 0.22, turbulence);
    let threshold = mix(0.68, 0.39, density);
    let transition = mix(0.025, 0.20, softness);
    let plume = smoothstep(threshold - transition, threshold + transition, field);
    let opacity = clamp(
        amount * mix(0.35, 1.0, density) * plume * 1.08,
        0.0,
        0.94,
    );
    let color = mask_effect_picker_color_to_working(secondary.xyz);
    return mix(input_rgb, color, opacity);
}
