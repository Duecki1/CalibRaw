// Chromatic aberration removal (Lens Corrections > Chromatic Aberration), in
// the RAW stage between the demosaic or raster finish and colour denoise.
//
// 1. chromatic_aberration_prepare (scene -> tex1) moves red and blue back onto
//    green by the photo's lateral model (`LateralChromaticAberration` in
//    calibraw-core), scaled by the amount on the CPU, and stores the
//    Laplacian of Gaussian of green in alpha.
// 2. chromatic_aberration_fringes (tex1 -> scene) removes the axial fringes no
//    displacement explains. Over a Gaussian window it fits each of red and
//    blue as a·G + c·∇²G + b: on an edge between two colours a channel
//    follows green affinely, and the ∇²G term absorbs a channel imaged
//    blurrier (c > 0) or sharper (c < 0) than green, which is what axial
//    aberration does. Where c is significant, the pixel is desaturated to the
//    share of its colour that the fit without that term, a·G + b, keeps. The
//    correction only removes colour: it never adds colour or changes a hue,
//    so colourful detail the fit misreads loses at most some saturation.
//    Warm hues are left alone (CA_WARM_HUE_*).
//
// Colour space: white-balanced camera RGB from the demosaic finish, whose
// channels are the sensor's, or a raster's scene-linear Rec.2020. A
// camera-linear raster's camera RGB is not balanced yet; the fringe pass
// balances it with the uniform's gains to tell colour from neutral. Green is
// never changed. Values may be negative (read noise) or above 1 (highlight
// reconstruction).
// Coordinates: tile-local pixels; neighbours clamp to the tile, whose export
// halo covers CHROMATIC_ABERRATION_SUPPORT (tiles.rs). The lateral model
// takes global positions in the full image, and every radius is set in native
// pixels and scaled to the processing resolution.

#import calibraw::common as Common

@group(0) @binding(11) var ca_read: texture_2d<f32>;
@group(0) @binding(10) var ca_write: texture_storage_2d<rgba16float /* CALIBRAW_WORK_FORMAT */, write>;

// Largest lateral shift, in native pixels (`MAX_LATERAL_SHIFT` in Rust).
const CA_MAX_LATERAL_SHIFT: u32 = 8u;
// Native pixels the fringe window reaches (`CA_WINDOW_RADIUS` in Rust).
const CA_WINDOW_RADIUS: u32 = 7u;
// Gaussian sigma of the fringe window and of the Laplacian's smoothing, in
// native pixels.
const CA_WINDOW_SIGMA: f32 = 3.5;
const CA_LAPLACIAN_SIGMA: f32 = 1.5;
// Gaussian taps a side of the Laplacian's smoothing at native resolution:
// ceil(3 sigma).
const CA_LAPLACIAN_TAPS: i32 = 5;
// The fit's regularization: green's variance is floored at the square of this
// share of the local green level, plus a dark floor, so noise in flat areas
// fits no edge. The ∇²G term's floor is a small fraction of it.
const CA_GREEN_FLOOR_SHARE: f32 = 0.02;
const CA_DARK_FLOOR: f32 = 0.002;
const CA_LAPLACIAN_RIDGE: f32 = 1e-4;
// t-statistic of c over which the fringe correction fades in.
const CA_SIGNIFICANCE_START: f32 = 3.0;
const CA_SIGNIFICANCE_FULL: f32 = 6.0;
// Axial fringes are purple, blue or green. Yellow through orange to red, in
// degrees of atan2(log B/G, log R/G), are the colours of lights, skin and
// foliage, whose glow and edges keep their colour; lateral red and yellow
// fringes are moved by the lateral model instead. The correction fades out
// over CA_WARM_HUE_RAMP degrees into this range.
const CA_WARM_HUE_START: f32 = -105.0;
const CA_WARM_HUE_END: f32 = 15.0;
const CA_WARM_HUE_RAMP: f32 = 20.0;

fn ca_texel(pos: vec2<i32>) -> vec4<f32> {
    return textureLoad(ca_read, Common::clamp_pos(pos), 0);
}

// The amount set by the edit, in [0, 1].
fn ca_amount() -> f32 {
    return clamp(Common::camera_uniforms.chromatic_aberration.x, 0.0, 1.0);
}

// Gains that white-balance the scene's red, green and blue.
fn ca_neutral_gains() -> vec3<f32> {
    return max(Common::camera_uniforms.chromatic_aberration.yzw, vec3<f32>(1e-6));
}

// Where `model` ([k1, k2, tx, ty], see calibraw-core) says its channel shows
// the green content of pixel `pos`, as an offset in processing pixels.
fn ca_lateral_offset(pos: vec2<i32>, model: vec4<f32>) -> vec2<f32> {
    let full = vec2<f32>(
        f32(max(Common::camera_uniforms.full_width, 1u) - 1u),
        f32(max(Common::camera_uniforms.full_height, 1u) - 1u),
    );
    let center = 0.5 * full;
    let half_diagonal = max(length(center), 1.0);
    let u = (vec2<f32>(pos + Common::tile_origin()) - center) / half_diagonal;
    let shift = ((model.x + model.y * dot(u, u)) * u + model.zw) * half_diagonal;
    let limit = f32(CA_MAX_LATERAL_SHIFT) * Common::processing_per_source_pixel();
    let length_px = length(shift);
    return select(shift, shift * (limit / length_px), length_px > limit);
}

fn ca_catmull_rom(t: f32) -> vec4<f32> {
    let t2 = t * t;
    let t3 = t2 * t;
    return vec4<f32>(
        -0.5 * t3 + t2 - 0.5 * t,
        1.5 * t3 - 2.5 * t2 + 1.0,
        -1.5 * t3 + 2.0 * t2 + 0.5 * t,
        0.5 * t3 - 0.5 * t2,
    );
}

// Channel `channel` of the scene at fractional position `position`, by
// Catmull-Rom interpolation clamped to its four nearest samples so edges do
// not ring.
fn ca_cubic_channel(position: vec2<f32>, channel: u32) -> f32 {
    let base = floor(position);
    let fraction = position - base;
    let origin = vec2<i32>(base) - vec2<i32>(1);
    let wx = ca_catmull_rom(fraction.x);
    let wy = ca_catmull_rom(fraction.y);
    var value = 0.0;
    var low = 3.0e38;
    var high = -3.0e38;
    for (var j = 0; j < 4; j = j + 1) {
        var row = 0.0;
        for (var i = 0; i < 4; i = i + 1) {
            let sample = ca_texel(origin + vec2<i32>(i, j))[channel];
            row += wx[i] * sample;
            if (i == 1 || i == 2) && (j == 1 || j == 2) {
                low = min(low, sample);
                high = max(high, sample);
            }
        }
        value += wy[j] * row;
    }
    return clamp(value, low, high);
}

// Laplacian of green smoothed by a Gaussian of CA_LAPLACIAN_SIGMA native
// pixels: the discrete Laplacian of the discrete, normalized Gaussian, applied
// as one kernel.
fn ca_green_laplacian(pos: vec2<i32>) -> f32 {
    let sigma = max(CA_LAPLACIAN_SIGMA * Common::processing_per_source_pixel(), 0.3);
    let taps = clamp(i32(ceil(3.0 * sigma)), 1, CA_LAPLACIAN_TAPS);
    // One-dimensional Gaussian weights of offsets -7 ..= 7 at index
    // offset + 7, zero past the smoothing's own taps, so the kernel can read
    // one offset beyond them.
    var gauss: array<f32, 15>;
    var total = 0.0;
    for (var offset = -taps; offset <= taps; offset = offset + 1) {
        let weight = exp(-0.5 * f32(offset * offset) / (sigma * sigma));
        gauss[offset + 7] = weight;
        total += weight;
    }
    for (var offset = -taps; offset <= taps; offset = offset + 1) {
        gauss[offset + 7] /= total;
    }
    var laplacian = 0.0;
    let reach = taps + 1;
    for (var y = -reach; y <= reach; y = y + 1) {
        for (var x = -reach; x <= reach; x = x + 1) {
            let kernel = gauss[x + 6] * gauss[y + 7] + gauss[x + 8] * gauss[y + 7]
                + gauss[x + 7] * gauss[y + 6] + gauss[x + 7] * gauss[y + 8]
                - 4.0 * gauss[x + 7] * gauss[y + 7];
            laplacian += kernel * ca_texel(pos + vec2<i32>(x, y)).g;
        }
    }
    return laplacian;
}

@compute @workgroup_size(8, 8, 1)
fn chromatic_aberration_prepare(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    var rgb = ca_texel(pos).rgb;
    let point = vec2<f32>(pos);
    let red_offset = ca_lateral_offset(pos, Common::camera_uniforms.lateral_ca_red);
    let blue_offset = ca_lateral_offset(pos, Common::camera_uniforms.lateral_ca_blue);
    // The channel at p shows green's content from p + offset, so green's
    // content at p is found at p - offset.
    if dot(red_offset, red_offset) > 1e-8 {
        rgb.r = ca_cubic_channel(point - red_offset, 0u);
    }
    if dot(blue_offset, blue_offset) > 1e-8 {
        rgb.b = ca_cubic_channel(point - blue_offset, 2u);
    }
    textureStore(ca_write, pos, vec4<f32>(rgb, ca_green_laplacian(pos)));
}

// Gaussian-weighted sums over the fringe window, of samples taken relative to
// the centre pixel so f32 keeps the variances' precision.
struct CaWindow {
    weight: f32,
    weight_squares: f32,
    // Sums of g, l, r, b.
    first: vec4<f32>,
    // Sums of g·g, l·l, g·l.
    green: vec3<f32>,
    // Sums of g·r, l·r, r·r.
    red: vec3<f32>,
    // Sums of g·b, l·b, b·b.
    blue: vec3<f32>,
}

fn ca_window(pos: vec2<i32>, center: vec4<f32>) -> CaWindow {
    let scale = Common::processing_per_source_pixel();
    let sigma = max(CA_WINDOW_SIGMA * scale, 0.75);
    let radius = clamp(i32(ceil(2.0 * sigma)), 1, i32(CA_WINDOW_RADIUS));
    var gauss: array<f32, 8>;
    for (var offset = 0; offset <= radius; offset = offset + 1) {
        gauss[offset] = exp(-0.5 * f32(offset * offset) / (sigma * sigma));
    }
    var window = CaWindow(0.0, 0.0, vec4<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0));
    for (var y = -radius; y <= radius; y = y + 1) {
        for (var x = -radius; x <= radius; x = x + 1) {
            let weight = gauss[abs(x)] * gauss[abs(y)];
            let sample = ca_texel(pos + vec2<i32>(x, y));
            // (g, l, r, b) relative to the centre.
            let d = vec4<f32>(sample.g, sample.a, sample.r, sample.b)
                - vec4<f32>(center.g, center.a, center.r, center.b);
            window.weight += weight;
            window.weight_squares += weight * weight;
            window.first += weight * d;
            window.green += weight * vec3<f32>(d.x * d.x, d.y * d.y, d.x * d.y);
            window.red += weight * vec3<f32>(d.x * d.z, d.y * d.z, d.z * d.z);
            window.blue += weight * vec3<f32>(d.x * d.w, d.y * d.w, d.w * d.w);
        }
    }
    return window;
}

// The fit of one channel: its value at the centre without the ∇²G term,
// and how strongly the window shows that term.
struct CaChannelFit {
    value: f32,
    significance: f32,
}

// Fits `channel ≈ a·G + c·∇²G + b` over the window. `sums` are the channel's
// sums of g·x, l·x and x·x, `mean` its windowed mean and `centre` its centre
// value; `laplacian` is ∇²G at the centre.
fn ca_fit_channel(
    window: CaWindow,
    sums: vec3<f32>,
    mean: f32,
    centre: f32,
    green_level: f32,
    laplacian: f32,
) -> CaChannelFit {
    let inverse_weight = 1.0 / max(window.weight, 1e-12);
    let mean_g = window.first.x * inverse_weight;
    let mean_l = window.first.y * inverse_weight;
    let var_gg = max(window.green.x * inverse_weight - mean_g * mean_g, 0.0);
    let var_ll = max(window.green.y * inverse_weight - mean_l * mean_l, 0.0);
    let cov_gl = window.green.z * inverse_weight - mean_g * mean_l;
    let cov_gx = sums.x * inverse_weight - mean_g * mean;
    let cov_lx = sums.y * inverse_weight - mean_l * mean;
    let var_xx = max(sums.z * inverse_weight - mean * mean, 0.0);

    let floor = CA_GREEN_FLOOR_SHARE * (max(green_level, 0.0) + CA_DARK_FLOOR);
    let eps = floor * floor;
    let a11 = var_gg + eps;
    let a22 = var_ll + CA_LAPLACIAN_RIDGE * eps;
    let det = max(a11 * a22 - cov_gl * cov_gl, 1e-30);
    let a = (cov_gx * a22 - cov_lx * cov_gl) / det;
    let c = (a11 * cov_lx - cov_gl * cov_gx) / det;
    // Intercept in centre-relative units; the fit at the centre is
    // centre + b, and dropping c·∇²G leaves centre + b - c·laplacian.
    let b = mean - a * mean_g - c * mean_l;

    // Standard error of c from the residual, over the window's effective
    // sample count at native resolution, so a reduced preview weighs the
    // same evidence as the full photo.
    let residual = max(var_xx - a * cov_gx - c * cov_lx, 1e-20);
    let scale = Common::processing_per_source_pixel();
    let samples = window.weight * window.weight / max(window.weight_squares, 1e-12) / (scale * scale);
    let standard_error = sqrt(residual * (a11 / det) / max(samples, 1.0));
    let t = abs(c) / max(standard_error, 1e-20);
    return CaChannelFit(
        centre + b - c * laplacian,
        smoothstep(CA_SIGNIFICANCE_START, CA_SIGNIFICANCE_FULL, t),
    );
}

// How much of the fringe correction a pixel of chroma `chroma` (log red and
// blue over green) takes: none for warm hues.
fn ca_fringe_hue_weight(chroma: vec2<f32>) -> f32 {
    let hue = degrees(atan2(chroma.y, chroma.x));
    let warm = smoothstep(CA_WARM_HUE_START - CA_WARM_HUE_RAMP, CA_WARM_HUE_START, hue)
        * (1.0 - smoothstep(CA_WARM_HUE_END, CA_WARM_HUE_END + CA_WARM_HUE_RAMP, hue));
    return 1.0 - warm;
}

// log2 of a channel over green, with the dark floor keeping both positive.
fn ca_log_ratio(value: f32, green: f32) -> f32 {
    return log2((max(value, 0.0) + CA_DARK_FLOOR) / (max(green, 0.0) + CA_DARK_FLOOR));
}

@compute @workgroup_size(8, 8, 1)
fn chromatic_aberration_fringes(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    let center = ca_texel(pos);
    let window = ca_window(pos, center);
    let inverse_weight = 1.0 / max(window.weight, 1e-12);
    let green_level = center.g + window.first.x * inverse_weight;
    let red = ca_fit_channel(
        window,
        window.red,
        window.first.z * inverse_weight,
        center.r,
        green_level,
        center.a,
    );
    let blue = ca_fit_channel(
        window,
        window.blue,
        window.first.w * inverse_weight,
        center.b,
        green_level,
        center.a,
    );

    // Scale the chroma (log red and blue over green) toward neutral, keeping
    // the share of it that the fit keeps along the same hue. A fit of another
    // hue keeps none of it.
    let gains = ca_neutral_gains();
    let balanced = center.rgb * gains;
    let observed = vec2<f32>(
        ca_log_ratio(balanced.r, balanced.g),
        ca_log_ratio(balanced.b, balanced.g),
    );
    let fitted = vec2<f32>(
        ca_log_ratio(red.value * gains.r, balanced.g),
        ca_log_ratio(blue.value * gains.b, balanced.g),
    );
    let kept = clamp(dot(observed, fitted) / max(dot(observed, observed), 1e-12), 0.0, 1.0);
    let strength = max(red.significance, blue.significance) * ca_fringe_hue_weight(observed)
        * ca_amount();
    let scale = 1.0 - strength * (1.0 - kept);
    var rgb = center.rgb;
    if scale < 1.0 {
        let chroma = observed * scale;
        let green = max(balanced.g, 0.0) + CA_DARK_FLOOR;
        rgb.r = (green * exp2(chroma.x) - CA_DARK_FLOOR) / gains.r;
        rgb.b = (green * exp2(chroma.y) - CA_DARK_FLOOR) / gains.b;
    }
    textureStore(ca_write, pos, vec4<f32>(rgb, 1.0));
}
