// SPDX-License-Identifier: GPL-3.0-or-later
// Adapted from darktable 5.6.1 VNG linear interpolation
// (src/iop/demosaicing/vng.c, `_vng_lininterpolate`).
// Copyright (C) 2010-2026 darktable developers.
// Copyright (C) 2026 CalibRaw contributors (WGSL adaptation).
//
// The smooth interpolation darktable uses for the flat areas of dual
// demosaicing and for the image border that Markesteijn leaves out. Colours
// are white-balanced camera RGB with sensor white 1. The tile edge is treated
// as the image border; inside an export halo that only touches discarded
// pixels.

#import calibraw::common as Common
#import calibraw::raw_sampling as RawSampling

fn cfa_in_bounds(pos: vec2<i32>) -> bool {
    return all(pos >= vec2<i32>(0)) && all(pos <= Common::image_max());
}

fn cfa_on_border(pos: vec2<i32>) -> bool {
    return any(pos <= vec2<i32>(0)) || any(pos >= Common::image_max());
}

// The interpolation class of the photosite at `pos`: R 0, G 1, B 2, and with
// `split_greens` (Bayer) the greens of odd rows 3. darktable's four-colour VNG
// interpolates each green class from the other so that green imbalance
// between the rows cancels; which rows count as odd does not matter because
// the two classes are averaged.
fn cfa_class(pos: vec2<i32>, split_greens: bool) -> u32 {
    let color = RawSampling::color_at(pos);
    if split_greens && color == 1u && (pos.y & 1) == 1 {
        return 3u;
    }
    return color;
}

fn cfa_class_mask(class_index: u32) -> vec4<f32> {
    return select(
        vec4<f32>(0.0),
        vec4<f32>(1.0),
        vec4<u32>(0u, 1u, 2u, 3u) == vec4<u32>(class_index),
    );
}

// darktable's VNG linear interpolation (`_vng_lininterpolate`) of camera RGB at
// tile-local `pos`. Each missing class is the weighted mean of its
// samples among the 8 neighbours, edge neighbours weighing 2 and corners 1;
// on the image border it is the plain mean of the neighbours inside the
// image. The photosite keeps its own sample, and samples below black count
// as 0.
fn cfa_linear_rgb(pos: vec2<i32>, split_greens: bool) -> vec3<f32> {
    let own = cfa_class(pos, split_greens);
    let center = max(RawSampling::raw_cfa_at(pos), 0.0);
    let border = cfa_on_border(pos);
    var sum = vec4<f32>(0.0);
    var weight = vec4<f32>(0.0);
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let q = pos + vec2<i32>(dx, dy);
            if !cfa_in_bounds(q) { continue; }
            let neighbour = cfa_class(q, split_greens);
            if neighbour == own { continue; }
            let w = select(select(1.0, 2.0, dx == 0 || dy == 0), 1.0, border);
            let mask = cfa_class_mask(neighbour);
            sum += mask * (w * max(RawSampling::raw_cfa_at(q), 0.0));
            weight += mask * w;
        }
    }
    // A class without a neighbour keeps the photosite's own sample, as
    // darktable's border interpolation does.
    let interpolated = select(
        vec4<f32>(center),
        sum / max(weight, vec4<f32>(1e-30)),
        weight > vec4<f32>(0.0),
    );
    let classes = mix(interpolated, vec4<f32>(center), cfa_class_mask(own));
    let green = select(classes.y, 0.5 * (classes.y + classes.w), split_greens);
    return max(vec3<f32>(classes.x, green, classes.z), vec3<f32>(0.0));
}
