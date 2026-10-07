
const MASK_LENS_BLUR_PI: f32 = 3.14159265;

// Match presence_reference_scale, retaining fractional radii and a true zero.
// The bilinear footprint is contained in the existing 144-pixel blur halo.
fn mask_focus_blur_radius(reference_pixels: f32, maximum: f32) -> f32 {
    let scale = clamp(
        f32(min(Common::camera_uniforms.full_width, Common::camera_uniforms.full_height)) / 1080.0,
        0.55, 3.0,
    );
    return clamp(reference_pixels * scale, 0.0, maximum);
}

fn mask_lens_blur_at(
    pos: vec2<i32>,
    primary: vec4<f32>,
    secondary: vec4<f32>,
    mask: MaskBlurCoverage,
) -> vec3<f32> {
    let radius = mask_focus_blur_radius(primary.y, 144.0);
    let center = SceneAdjustments::local_effects_at(pos);
    if radius <= 1e-6 { return center; }
    let blades = clamp(round(primary.z), 3.0, 12.0);
    let rotation = radians(primary.w);
    let sector = 2.0 * MASK_LENS_BLUR_PI / blades;
    let apothem = cos(MASK_LENS_BLUR_PI / blades);
    let highlight_boost = clamp(secondary.x / 100.0, 0.0, 1.0);
    // Concentric equal-area strata replace the sparse 28-64 point spiral.
    // Antipodal pairs preserve the centroid and cancel first-order sampling bias.
    // Count changes add rings, rather than redistributing all samples abruptly.
    let rings = u32(clamp(ceil(radius), 2.0, 16.0));
    var sum = vec3<f32>(0.0);
    var total_weight = 0.0;
    var kernel_weight = 0.0;
    for (var ring = 0u; ring < 16u; ring += 1u) {
        if ring >= rings { break; }
        let ring_inner = f32(ring) / f32(rings);
        let ring_outer = f32(ring + 1u) / f32(rings);
        let radial = sqrt((ring_inner * ring_inner + ring_outer * ring_outer) * 0.5);
        let pairs = 2u * ring + 2u;
        let area = (ring_outer * ring_outer - ring_inner * ring_inner) / f32(pairs * 2u);
        for (var pair = 0u; pair < 32u; pair += 1u) {
            if pair >= pairs { break; }
            let angle = (f32(pair) + fract(f32(ring) * 0.381966))
                * MASK_LENS_BLUR_PI / f32(pairs) + rotation;
            for (var side = 0u; side < 2u; side += 1u) {
                let theta = angle + f32(side) * MASK_LENS_BLUR_PI;
                let aperture_angle = (fract((theta - rotation) / sector + 0.5) - 0.5) * sector;
                let polygon_radius = apothem / max(cos(aperture_angle), 0.25);
                let offset = vec2<f32>(cos(theta), sin(theta)) * (radius * radial * polygon_radius);
                let tap = vec2<f32>(pos) + offset;
                let sample = mask_effect_source_linear_at(tap);
                let bright = smoothstep(0.25, 2.0, Common::safe_luma(sample));
                // Include the polar Jacobian so polygon corners receive their
                // proper area instead of concentrating energy at the center.
                let weight = area * polygon_radius * polygon_radius
                    * (1.0 + highlight_boost * 1.5 * bright);
                let masked = weight * mask_blur_tap_weight(mask, tap);
                sum += sample * masked;
                total_weight += masked;
                kernel_weight += weight;
            }
        }
    }
    return mask_blur_masked_mean(sum, total_weight, kernel_weight, center);
}
