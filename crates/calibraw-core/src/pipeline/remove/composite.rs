//! Compositing remove patches into linear regions.

use super::*;

pub fn composite_remove_edits_into_linear_region(
    edits: &RemoveEditState,
    region: NativeRect,
    rgb: &mut [f32],
) {
    if rgb.len() != region.width as usize * region.height as usize * 3 {
        return;
    }
    for stroke in &edits.strokes {
        for patch in &stroke.patches {
            composite_patch_into_linear_region_with_opacity(
                patch,
                region,
                rgb,
                stroke.composite_opacity(),
                stroke.retouch.is_some(),
            );
        }
    }
}

pub fn composite_patch_into_linear_region(
    patch: &RemovePatch,
    region: NativeRect,
    rgb: &mut [f32],
) {
    composite_patch_into_linear_region_with_opacity(patch, region, rgb, 1.0, false);
}

fn composite_patch_into_linear_region_with_opacity(
    patch: &RemovePatch,
    region: NativeRect,
    rgb: &mut [f32],
    opacity: f32,
    retouch_coverage: bool,
) {
    if !patch.has_scene_pixels() {
        return;
    }
    let Some(intersection) = patch.bounds.intersect(region) else {
        return;
    };
    for y in intersection.y..intersection.bottom() {
        let patch_y = (y - patch.bounds.y) as usize;
        let region_y = (y - region.y) as usize;
        for x in intersection.x..intersection.right() {
            let patch_x = (x - patch.bounds.x) as usize;
            let region_x = (x - region.x) as usize;
            let patch_index = patch_y * patch.bounds.width as usize + patch_x;
            let coverage = patch.alpha[patch_index] as f32 / 255.0;
            let alpha = if retouch_coverage {
                if coverage > 0.0 {
                    opacity
                } else {
                    0.0
                }
            } else {
                coverage * opacity
            };
            if alpha <= 0.0 {
                continue;
            }
            let rgb_index = patch_index * 3;
            let repaired = [
                half::f16::from_bits(patch.rgb_scene16f[rgb_index]).to_f32(),
                half::f16::from_bits(patch.rgb_scene16f[rgb_index + 1]).to_f32(),
                half::f16::from_bits(patch.rgb_scene16f[rgb_index + 2]).to_f32(),
            ];
            let out_index = (region_y * region.width as usize + region_x) * 3;
            for channel in 0..3 {
                rgb[out_index + channel] =
                    rgb[out_index + channel] * (1.0 - alpha) + repaired[channel] * alpha;
            }
        }
    }
}

pub fn display_linear_rec2020_to_model_srgb(rgb: [f32; 3]) -> [f32; 3] {
    display_linear_rec2020_to_srgb(rgb)
}

pub fn model_srgb_to_display_linear_rec2020(rgb: [f32; 3]) -> [f32; 3] {
    linear_srgb_to_rec2020(rgb.map(srgb_decode))
}
