// Lens Blur: a gather over an aperture-shaped kernel in tile pixels, from
// the first mask Blur stage (blur.wgsl `apply_mask_focus_blurs`), on
// scene-linear Rec.2020 that may be negative or above 1.
//
// Without Focus by depth (LENS_BLUR_DEPTH_OPTION) the whole mask blurs by the
// Radius. With it, and once scene depth exists, each of the pixel's surfaces
// (scene/geometry.wgsl, up to two) blurs by its own radius, which grows with
// its distance from the focus range in inverse distance, less the blur the
// photo's lens already gave it (bake_focus.wgsl). A surface gathers only taps on its
// own surface, so a sharp subject never smears into the background behind
// it, nor the background into the subject; the two results blend by the
// pixel's coverage. Until depth exists the mask blurs evenly.

const MASK_LENS_BLUR_PI: f32 = 3.14159265;

// Match presence_reference_scale, retaining fractional radii and a true zero.
// The bilinear footprint is contained in the existing 144-pixel blur halo.
// `maximum` is in native pixels, so previews limit at the same share of the photo.
fn mask_focus_blur_radius(reference_pixels: f32, maximum: f32) -> f32 {
    let scale = Common::resolution_scale(0.55, 3.0);
    return clamp(
        reference_pixels * scale,
        0.0,
        maximum * Common::processing_per_source_pixel(),
    );
}

fn mask_lens_blur_at(pos: vec2<i32>, params: MaskEffectParams, mask: MaskBlurCoverage) -> vec3<f32> {
    let radius = mask_focus_blur_radius(mask_effect_lane(params, LENS_BLUR_RADIUS_LANE), 144.0);
    let center = SceneAdjustments::local_effects_at(pos);
    if radius <= 1e-6 { return center; }
    if params.options[LENS_BLUR_DEPTH_OPTION] > 0.5 && Geometry::scene_geometry_present() {
        return mask_lens_blur_by_depth(pos, params, mask, radius, center);
    }
    return mask_lens_blur_even(pos, params, mask, radius, center);
}

// --- The aperture ---

// The aperture polygon, from the slot's blades and rotation.
struct LensBlurAperture {
    rotation: f32,
    // The angle each blade spans.
    sector: f32,
    // The polygon's inradius relative to its circumradius.
    apothem: f32,
}

fn lens_blur_aperture(params: MaskEffectParams) -> LensBlurAperture {
    let blades = clamp(round(mask_effect_lane(params, LENS_BLUR_BLADES_LANE)), 3.0, 12.0);
    return LensBlurAperture(
        radians(mask_effect_lane(params, LENS_BLUR_ROTATION_LANE)),
        2.0 * MASK_LENS_BLUR_PI / blades,
        cos(MASK_LENS_BLUR_PI / blades),
    );
}

// Concentric equal-area strata replace the sparse 28-64 point spiral.
// Antipodal pairs preserve the centroid and cancel first-order sampling bias.
// Count changes add rings, rather than redistributing all samples abruptly.
// Ring r of a gather holds 2r + 2 pairs.
fn lens_blur_rings(radius: f32) -> u32 {
    return u32(clamp(ceil(radius), 2.0, 16.0));
}

// Tap `side` (0 or 1) of pair `pair` on ring `ring` of `rings`, for a gather
// of `radius` pixels: its offset in pixels (xy) and its weight (z), the share
// of the aperture's area it stands for with the polar Jacobian, so polygon
// corners receive their proper area instead of concentrating energy at the
// center.
fn lens_blur_tap(
    aperture: LensBlurAperture,
    radius: f32,
    rings: u32,
    ring: u32,
    pair: u32,
    side: u32,
) -> vec3<f32> {
    let ring_inner = f32(ring) / f32(rings);
    let ring_outer = f32(ring + 1u) / f32(rings);
    let radial = sqrt((ring_inner * ring_inner + ring_outer * ring_outer) * 0.5);
    let pairs = 2u * ring + 2u;
    let area = (ring_outer * ring_outer - ring_inner * ring_inner) / f32(pairs * 2u);
    let angle = (f32(pair) + fract(f32(ring) * 0.381966))
        * MASK_LENS_BLUR_PI / f32(pairs) + aperture.rotation;
    let theta = angle + f32(side) * MASK_LENS_BLUR_PI;
    let aperture_angle = (fract((theta - aperture.rotation) / aperture.sector + 0.5) - 0.5) * aperture.sector;
    let polygon_radius = aperture.apothem / max(cos(aperture_angle), 0.25);
    let offset = vec2<f32>(cos(theta), sin(theta)) * (radius * radial * polygon_radius);
    return vec3<f32>(offset, area * polygon_radius * polygon_radius);
}

// How much more a tap of colour `sample` weighs as a bright highlight, for a
// Highlight boost of `boost` (0 to 1).
fn lens_blur_highlight(sample: vec3<f32>, boost: f32) -> f32 {
    let bright = smoothstep(0.25, 2.0, Common::safe_luma(sample));
    return 1.0 + boost * 1.5 * bright;
}

// --- Without depth ---

fn mask_lens_blur_even(
    pos: vec2<i32>,
    params: MaskEffectParams,
    mask: MaskBlurCoverage,
    radius: f32,
    center: vec3<f32>,
) -> vec3<f32> {
    let aperture = lens_blur_aperture(params);
    let highlight_boost = mask_effect_percent(params, LENS_BLUR_HIGHLIGHTS_LANE);
    let rings = lens_blur_rings(radius);
    var sum = vec3<f32>(0.0);
    var total_weight = 0.0;
    var kernel_weight = 0.0;
    for (var ring = 0u; ring < 16u; ring += 1u) {
        if ring >= rings { break; }
        let pairs = 2u * ring + 2u;
        for (var pair = 0u; pair < 32u; pair += 1u) {
            if pair >= pairs { break; }
            for (var side = 0u; side < 2u; side += 1u) {
                let footprint = lens_blur_tap(aperture, radius, rings, ring, pair, side);
                let tap = vec2<f32>(pos) + footprint.xy;
                let sample = mask_effect_source_linear_at(tap);
                let weight = footprint.z * lens_blur_highlight(sample, highlight_boost);
                let masked = weight * mask_blur_tap_weight(mask, tap);
                sum += sample * masked;
                total_weight += masked;
                kernel_weight += weight;
            }
        }
    }
    return mask_blur_masked_mean(sum, total_weight, kernel_weight, center);
}

// --- By depth ---

// Radii below this many pixels leave a surface as it is.
const LENS_BLUR_SHARP_RADIUS: f32 = 0.25;

fn mask_lens_blur_by_depth(
    pos: vec2<i32>,
    params: MaskEffectParams,
    mask: MaskBlurCoverage,
    radius: f32,
    center: vec3<f32>,
) -> vec3<f32> {
    let scene_rgb = Geometry::scene_unedited_at(pos);
    let pixel = Geometry::scene_pixel(full_image_uv(pos), scene_rgb, 1.0);
    let colors = Geometry::scene_split(pixel, scene_rgb, center);
    let focus = lens_blur_focus(params);
    let near = mask_lens_blur_surface(
        pos, params, mask, pixel.near, colors[0],
        lens_blur_added_radius(pixel.near, focus, radius),
    );
    if !pixel.in_band {
        return near;
    }
    let far = mask_lens_blur_surface(
        pos, params, mask, pixel.far, colors[1],
        lens_blur_added_radius(pixel.far, focus, radius),
    );
    return Geometry::scene_blend(pixel, near, far);
}

// The focus range in inverse distance relative to the nearest surface's,
// D_near / D (camera.wgsl), which a control depth c gives as Q^-c: of its
// near end (x) and its far end (y), and the greatest distance in it of any
// surface from the range (z), between the nearest surface (1) and the
// farthest (1 / Q). The ends are control depths (FOCUS_NEAR ≤ FOCUS_FAR). A
// flat scene (Q about 1) has no depth to defocus by: z is 0.
fn lens_blur_focus(params: MaskEffectParams) -> vec3<f32> {
    let ratio = lens_blur_depth_ratio();
    let near = pow(ratio, -mask_effect_lane(params, LENS_BLUR_FOCUS_NEAR_LANE));
    let far = pow(ratio, -mask_effect_lane(params, LENS_BLUR_FOCUS_FAR_LANE));
    return vec3<f32>(near, far, max(1.0 - near, far - 1.0 / ratio));
}

// Q = D_far / D_near, the scene's depth ratio, at least 1.
fn lens_blur_depth_ratio() -> f32 {
    let camera = Bake::scene_bake.camera;
    return max(camera.anchors.w / camera.anchors.z, 1.0);
}

// The blur radius in pixels added to `surface`. The total it aims for follows
// the thin lens, linear in inverse distance: 0 inside the focus range,
// `radius` at the scene's most out-of-focus end. The photo's own defocus
// there (B7's fit, bake_focus.wgsl, in 1 / z of the frame at relief scale 1)
// is subtracted in quadrature, taking a disc of radius r as deviation r / 2,
// so a surface the lens already softened is topped up to the total rather
// than blurred twice, and the transition from sharp continues the lens's.
// The fit's flat term is left out: it is the working image's resampling, not
// the lens.
fn lens_blur_added_radius(surface: Geometry::SceneSurface, focus: vec3<f32>, radius: f32) -> f32 {
    if focus.z <= 1e-6 {
        return 0.0;
    }
    // D_near / D = 1 / (1 + (Q − 1) · d) for linear depth d.
    let inverse = 1.0 / (1.0 + (lens_blur_depth_ratio() - 1.0) * surface.d);
    let defocus = max(inverse - focus.x, 0.0) + max(focus.y - inverse, 0.0);
    let total = radius * min(defocus / focus.z, 1.0);
    // The fit's deviation in shorter edges, as pixels.
    let fit = Bake::scene_bake.focus_fit;
    let lens = select(0.0, fit.x * abs(1.0 / surface.position.z - fit.y), fit.w != 0.0)
        * effect_short_edge();
    let lens_radius = 2.0 * lens;
    return sqrt(max(total * total - lens_radius * lens_radius, 0.0));
}

// One of the pixel's surfaces blurred by `radius` pixels: `color` is its
// colour at the pixel, in the blur's domain. Each tap counts by how well it
// lies on the surface (`lens_blur_layer_at`), as normalized convolution, so
// taps of other surfaces leave no trace; with hardly any tap on it the
// surface keeps its own colour (`mask_blur_masked_mean`).
fn mask_lens_blur_surface(
    pos: vec2<i32>,
    params: MaskEffectParams,
    mask: MaskBlurCoverage,
    surface: Geometry::SceneSurface,
    color: vec3<f32>,
    radius: f32,
) -> vec3<f32> {
    if radius < LENS_BLUR_SHARP_RADIUS {
        return color;
    }
    let aperture = lens_blur_aperture(params);
    let highlight_boost = mask_effect_percent(params, LENS_BLUR_HIGHLIGHTS_LANE);
    let rings = lens_blur_rings(radius);
    let camera = Bake::scene_bake.camera;
    // Distance from the surface's tangent plane, relative to its depth, that
    // still counts as the surface: more for a wider gather, whose taps reach
    // where a curved surface leaves its plane (as `scene_soft_normal`).
    let tolerance = 0.01
        + 2.0 * Camera::scene_frame_position(camera, vec2<f32>(radius * scene_pixel_extent()), 1.0).x;
    var sum = vec3<f32>(0.0);
    var total_weight = 0.0;
    var kernel_weight = 0.0;
    for (var ring = 0u; ring < 16u; ring += 1u) {
        if ring >= rings { break; }
        let pairs = 2u * ring + 2u;
        for (var pair = 0u; pair < 32u; pair += 1u) {
            if pair >= pairs { break; }
            for (var side = 0u; side < 2u; side += 1u) {
                let footprint = lens_blur_tap(aperture, radius, rings, ring, pair, side);
                let tap = vec2<f32>(pos) + footprint.xy;
                let layer = lens_blur_layer_at(surface, tap, tolerance);
                let weight = footprint.z * lens_blur_highlight(layer.rgb, highlight_boost);
                let masked = weight * layer.w * mask_blur_tap_weight(mask, tap);
                sum += layer.rgb * masked;
                total_weight += masked;
                kernel_weight += weight;
            }
        }
    }
    return mask_blur_masked_mean(sum, total_weight, kernel_weight, color);
}

// Full-image uv of a tap at `tap` tile pixels (pixel centres at integers).
fn lens_blur_tap_uv(tap: vec2<f32>) -> vec2<f32> {
    let full = max(
        vec2<f32>(f32(Common::camera_uniforms.full_width), f32(Common::camera_uniforms.full_height)),
        vec2<f32>(1.0),
    );
    return (tap + vec2<f32>(Common::tile_origin()) + vec2<f32>(0.5)) / full;
}

// What a tap at `tap` tile pixels gives `surface`: the colour of whichever of
// its working texel's surfaces lies on `surface`'s plane (rgb), and how much
// it counts (w): how close that surface lies to the plane, times its share of
// the texel. A texel at an edge shows both of its surfaces blended; the one
// taken is split from the photo's colour by the bake's layer colours, as
// `scene_split` splits a pixel but at the working texel (its unedited colour
// gives coverage and the ratio), so neither surface's colour leaks into the
// other's blur.
fn lens_blur_layer_at(surface: Geometry::SceneSurface, tap: vec2<f32>, tolerance: f32) -> vec4<f32> {
    let uv = lens_blur_tap_uv(tap);
    let texel = Bake::scene_bake_clamp(vec2<i32>(floor(uv * vec2<f32>(Bake::scene_bake_size()))));
    let point = Camera::scene_image_point(uv, Geometry::scene_aspect());
    let rgb = mask_effect_source_linear_at(tap);
    let g = Geometry::scene_layer(texel, Bake::SCENE_LAYER_GEOMETRY);
    let k = Geometry::scene_layer(texel, Bake::SCENE_LAYER_COVERAGE);
    if Bake::scene_coverage_constant(k) {
        // One surface, on the side its coverage says.
        let d = select(g.w, g.z, k.w > 0.5);
        return vec4<f32>(rgb, lens_blur_plane_share(surface, point, d, tolerance));
    }
    let near_share = lens_blur_plane_share(surface, point, g.z, tolerance);
    let far_share = lens_blur_plane_share(surface, point, g.w, tolerance);
    let near_side = near_share >= far_share;
    let working = textureLoad(Geometry::scene_working_image, texel, 0).rgb;
    let alpha = Bake::scene_coverage_of(k, Bake::scene_bake_guide(working));
    let near_color = Geometry::scene_layer(texel, Bake::SCENE_LAYER_NEAR).rgb;
    let far_color = Geometry::scene_layer(texel, Bake::SCENE_LAYER_FAR).rgb;
    let blend = alpha * near_color + (1.0 - alpha) * far_color;
    let taken = select(far_color, near_color, near_side);
    let ratio = select(
        vec3<f32>(1.0),
        taken / max(blend, vec3<f32>(Geometry::SCENE_SPLIT_LEAST_BLEND)),
        blend > vec3<f32>(Geometry::SCENE_SPLIT_LEAST_BLEND),
    );
    let share = select((1.0 - alpha) * far_share, alpha * near_share, near_side);
    return vec4<f32>(rgb * ratio, share);
}

// How well a surface of linear depth `d` seen at image point `point` lies on
// `surface`'s tangent plane: 1 on it, falling as a Gaussian of its distance
// from it relative to `surface`'s depth, in units of `tolerance`.
fn lens_blur_plane_share(surface: Geometry::SceneSurface, point: vec2<f32>, d: f32, tolerance: f32) -> f32 {
    let camera = Bake::scene_bake.camera;
    let z = Camera::scene_frame_depth(camera, d, 1.0);
    let offset = dot(surface.normal, Camera::scene_frame_position(camera, point, z) - surface.position);
    let residual = abs(offset) / surface.position.z / tolerance;
    return exp(-residual * residual);
}
