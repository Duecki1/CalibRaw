#import calibraw::common as Common
#import calibraw::color as Color
#import calibraw::profile as Profile

// The edit's creative 3D LUT (calibraw_core::luts), applied by apply_view_node
// after the view transform and point colours and before grain and vignette.
//
// Colour spaces: the input and output are display-linear Rec.2020 (D65). The
// LUT is indexed by the sRGB-encoded display value, encoded exactly as for the
// screen (Profile::apply_output_encoding: out-of-gamut colours are compressed
// to the sRGB boundary, then the sRGB curve is applied and the result clamped
// to [0, 1]). That value is mapped from the LUT's domain onto its lattice;
// inputs outside the domain are clamped to it. The LUT's output is read as
// sRGB-encoded sRGB and decoded back to linear Rec.2020.
//
// Only the LUT's change is added, scaled by the amount:
//   out = in + amount * (decode(lut(encode(in))) - decode(encode(in)))
// An identity LUT therefore leaves every value unchanged, including HDR and
// out-of-gamut values that the encoding clamps, and at amounts below 1 those
// values keep what the encoding cut off.
//
// Interpolation is tetrahedral from four texel loads of an unfiltered
// Rgba32Float texture, so its precision does not depend on the GPU's texture
// filtering. Mirrors CubeLut::sample_tetrahedral in calibraw-core.

// size³ texels, red along x, green along y, blue along z; alpha unused.
@group(0) @binding(61) var look_lut_tex: texture_3d<f32>;

fn look_lut_texel(cell: vec3<u32>, corner: vec3<u32>) -> vec3<f32> {
    return textureLoad(look_lut_tex, cell + corner, 0).xyz;
}

// The LUT at `coordinate`, which is in [0, 1] on every axis.
fn sample_tetrahedral(coordinate: vec3<f32>, size: f32) -> vec3<f32> {
    let last = size - 1.0;
    let position = coordinate * last;
    // The upper cell corner must stay on the lattice, so 1.0 falls in the last
    // cell with a fraction of 1.
    let cell_origin = min(floor(position), vec3<f32>(last - 1.0));
    let fraction = position - cell_origin;
    let cell = vec3<u32>(cell_origin);
    let origin = look_lut_texel(cell, vec3<u32>(0u, 0u, 0u));
    let white = look_lut_texel(cell, vec3<u32>(1u, 1u, 1u));
    let fr = fraction.x;
    let fg = fraction.y;
    let fb = fraction.z;
    // Each ordering of the fractions selects one of the six tetrahedra that
    // split the cell along its black-to-white diagonal.
    var first: vec3<f32>;
    var second: vec3<f32>;
    var weights: vec3<f32>;
    if fr > fg {
        if fg > fb {
            first = look_lut_texel(cell, vec3<u32>(1u, 0u, 0u));
            second = look_lut_texel(cell, vec3<u32>(1u, 1u, 0u));
            weights = vec3<f32>(fr, fg, fb);
        } else if fr > fb {
            first = look_lut_texel(cell, vec3<u32>(1u, 0u, 0u));
            second = look_lut_texel(cell, vec3<u32>(1u, 0u, 1u));
            weights = vec3<f32>(fr, fb, fg);
        } else {
            first = look_lut_texel(cell, vec3<u32>(0u, 0u, 1u));
            second = look_lut_texel(cell, vec3<u32>(1u, 0u, 1u));
            weights = vec3<f32>(fb, fr, fg);
        }
    } else if fb > fg {
        first = look_lut_texel(cell, vec3<u32>(0u, 0u, 1u));
        second = look_lut_texel(cell, vec3<u32>(0u, 1u, 1u));
        weights = vec3<f32>(fb, fg, fr);
    } else if fb > fr {
        first = look_lut_texel(cell, vec3<u32>(0u, 1u, 0u));
        second = look_lut_texel(cell, vec3<u32>(0u, 1u, 1u));
        weights = vec3<f32>(fg, fb, fr);
    } else {
        first = look_lut_texel(cell, vec3<u32>(0u, 1u, 0u));
        second = look_lut_texel(cell, vec3<u32>(1u, 1u, 0u));
        weights = vec3<f32>(fg, fr, fb);
    }
    return origin
        + weights.x * (first - origin)
        + weights.y * (second - first)
        + weights.z * (white - second);
}

fn apply_look_lut(display_linear: vec3<f32>) -> vec3<f32> {
    let lut = Common::scene_tone_uniforms.look_lut;
    let amount = lut.x;
    if amount <= 0.0 {
        return display_linear;
    }
    let encoded = Profile::apply_output_encoding(display_linear);
    let domain_min = Common::scene_tone_uniforms.look_lut_domain_min.xyz;
    let domain_scale = Common::scene_tone_uniforms.look_lut_domain_scale.xyz;
    let coordinate = clamp((encoded - domain_min) * domain_scale, vec3<f32>(0.0), vec3<f32>(1.0));
    let looked = sample_tetrahedral(coordinate, lut.y);
    let change = Common::SRGB_TO_REC2020 * (Color::srgb_eotf(looked) - Color::srgb_eotf(encoded));
    return display_linear + amount * change;
}
