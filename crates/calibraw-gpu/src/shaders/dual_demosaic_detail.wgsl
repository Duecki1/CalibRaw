// SPDX-License-Identifier: GPL-3.0-or-later
// Adapted from darktable 5.6.1 dual demosaicing.
// Copyright (C) 2010-2026 darktable developers.
// Dual-demosaic credits: Ingo Weyrich and Hanno Schwalm.
// Copyright (C) 2026 CalibRaw contributors (WGSL adaptation).
//
// Pass 2 of dual demosaicing (dual_demosaic.wgsl): the first colour-smoothing
// pass of the linear interpolation and the unblurred detail mask.

#import calibraw::common as Common
#import calibraw::dual_demosaic as Dual

@group(0) @binding(22) var dual_low_write: texture_storage_2d<rgba16float /* CALIBRAW_WORK_FORMAT */, write>;

@compute @workgroup_size(8, 8, 1)
fn dual_smooth_detail(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    textureStore(
        dual_low_write,
        pos,
        vec4<f32>(Dual::dual_smoothed_prepared(pos), Dual::dual_detail_blend_at(pos)),
    );
}
