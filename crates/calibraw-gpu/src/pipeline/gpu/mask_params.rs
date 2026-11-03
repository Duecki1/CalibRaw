//! Packing of local adjustment masks and mask effects into `MaskData` slots.

use super::*;
use crate::pipeline::effect_lanes::{light_rays, options, rain, relight};
use crate::pipeline::{EffectLanes, PackContext};

/// An effect slot holding `lanes`.
fn effect_mask_data(effect: MaskEffect, active: bool, lanes: &EffectLanes) -> MaskData {
    let [adjust_0, adjust_1, adjust_2] = lanes.rows();
    MaskData {
        metadata: [
            u32::from(active),
            u32::from(active),
            0,
            effect.shader_id() << MASK_EFFECT_ID_SHIFT,
        ],
        adjust_0,
        adjust_1,
        adjust_2,
        effect_options: lanes.options(),
        ..MaskData::zeroed()
    }
}

/// Whether an active Rain slot throws splashes on the ground, which it finds in
/// scene depth.
pub(super) fn rain_reads_ground(data: &MaskData) -> bool {
    data.applies(MaskEffect::Rain)
        && data.option(options::RAIN_DEPTH) > 0.5
        && data.lane(rain::SPLASHES) > 0.0
}

/// Whether an active Fog, Smoke or Rain slot scatters the photograph's own lights.
pub(super) fn medium_uses_image_lights(data: &MaskData) -> bool {
    (data.applies(MaskEffect::Fog)
        || data.applies(MaskEffect::Smoke)
        || data.applies(MaskEffect::Rain))
        && data.option(options::MEDIUM_IMAGE_LIGHTS) > 0.5
}

/// Lights whose shadows the scene shadow map holds, one per RGBA channel.
pub(super) const SCENE_SHADOW_MAP_CHANNELS: usize = 4;

/// Whether an active Relight slot casts shadows on its own shading. Mirrors
/// the amount and shadow-strength checks of `apply_relight`.
pub(super) fn relight_casts_shadows(data: &MaskData) -> bool {
    emits_scene_light(data)
        && data.applies(MaskEffect::Relight)
        && (data.lane(relight::SHADOWS) / 100.0).clamp(0.0, 1.0) > 1e-6
}

/// Whether an active slot places a light in the scene. Mirrors `emits` of
/// `scene_light_at` in scene_lights.wgsl.
fn emits_scene_light(data: &MaskData) -> bool {
    let percent = |lane| (data.lane(lane) / 100.0).clamp(0.0, 1.0);
    if data.applies(MaskEffect::Relight) {
        return percent(relight::AMOUNT) > 1e-6;
    }
    data.applies(MaskEffect::LightRays)
        && percent(light_rays::AMOUNT) > 1e-6
        && (data.lane(light_rays::LENGTH) / 100.0).clamp(0.0, 2.0) > 1e-6
}

/// Whether an active slot receives scene lights, shadowed by the scene shadow
/// map (scene_shadows.wgsl).
pub(super) fn receives_scene_light(data: &MaskData) -> bool {
    [
        MaskEffect::Fog,
        MaskEffect::Smoke,
        MaskEffect::Rain,
        MaskEffect::WetSurfaces,
    ]
    .into_iter()
    .any(|effect| data.applies(effect))
}

/// The scene shadow map's channel of a slot's light, if it has one.
pub(super) fn scene_light_channel(data: &MaskData) -> Option<usize> {
    let lane = data.option(options::SCENE_LIGHT_CHANNEL);
    (lane > 0.5).then(|| lane.round() as usize - 1)
}

/// Gives the first slots that place a light in the scene, in slot order, a
/// channel of the scene shadow map each.
fn assign_scene_light_channels(packed: &mut [MaskData]) {
    let lights = packed
        .iter_mut()
        .filter(|data| emits_scene_light(data))
        .take(SCENE_SHADOW_MAP_CHANNELS);
    for (channel, data) in lights.enumerate() {
        data.set_option(options::SCENE_LIGHT_CHANNEL, (channel + 1) as f32);
    }
}

/// The direction in image points that `output_direction`, a unit vector in the
/// output frame, points in, for a projection from source to output
/// coordinates. Rain and the ground follow the frame the photo is seen in, so
/// straightening or flipping it does not tilt them.
pub(super) fn source_direction(
    projection: [[f32; 3]; 3],
    output_size: [f32; 2],
    image_size: [f32; 2],
    output_direction: [f32; 2],
) -> [f32; 2] {
    let project = |u: f32, v: f32| {
        let row = |r: [f32; 3]| r[0] * u + r[1] * v + r[2];
        let w = row(projection[2]);
        [row(projection[0]) / w, row(projection[1]) / w]
    };
    let short = output_size[0].min(output_size[1]).max(1.0);
    let scale = [output_size[0] / short, output_size[1] / short];
    const STEP: f32 = 1e-3;
    let center = project(0.5, 0.5);
    let along_u = project(0.5 + STEP, 0.5);
    let along_v = project(0.5, 0.5 + STEP);
    // Output points per image point along each image axis.
    let along_x = [
        (along_u[0] - center[0]) * scale[0] / (STEP * image_size[0]),
        (along_u[1] - center[1]) * scale[1] / (STEP * image_size[0]),
    ];
    let along_y = [
        (along_v[0] - center[0]) * scale[0] / (STEP * image_size[1]),
        (along_v[1] - center[1]) * scale[1] / (STEP * image_size[1]),
    ];
    let determinant = along_x[0] * along_y[1] - along_y[0] * along_x[1];
    if determinant.abs() < 1e-6 {
        return output_direction;
    }
    let [fx, fy] = output_direction;
    let x = (fx * along_y[1] - fy * along_y[0]) / determinant;
    let y = (fy * along_x[0] - fx * along_x[1]) / determinant;
    let magnitude = x.hypot(y);
    if magnitude < 1e-6 {
        return output_direction;
    }
    [x / magnitude, y / magnitude]
}

/// Points the slots' fall directions, which are in the output frame when this
/// is called, along `direction_of`, which maps an output-frame direction to
/// image points.
pub(super) fn orient_rain_slots(
    packed: &mut [MaskData],
    direction_of: impl Fn([f32; 2]) -> [f32; 2],
) {
    for data in packed.iter_mut() {
        if data.effect_id() == MaskEffect::Rain.shader_id() {
            let fall = direction_of([data.lane(rain::FALL_X), data.lane(rain::FALL_Y)]);
            data.set_lane(rain::FALL_X, fall[0]);
            data.set_lane(rain::FALL_Y, fall[1]);
        }
    }
}

/// The slot of an effect: its settings as its shader reads them, active when
/// `enabled` and the settings change the image. `None` for an adjustment.
pub(super) fn pack_effect_mask(
    effect: MaskEffect,
    settings: &crate::pipeline::MaskEffectSettings,
    enabled: bool,
    shutter_seconds: f32,
) -> Option<MaskData> {
    let lanes = settings.pack(effect, &PackContext { shutter_seconds })?;
    let active = enabled && settings.effect_is_active(effect);
    Some(effect_mask_data(effect, active, &lanes))
}

pub(super) fn pack_adjustment_mask(mask: &LocalMask) -> MaskData {
    let adjustment = mask.adjustments;
    let adjustment_enabled =
        mask.enabled && mask.adjustments_enabled && mask.effect.uses_adjustments();
    let has_hsl = adjustment.has_color_mixer();
    let curve_flags = adjustment.curve_feature_flags();
    let has_grading = adjustment.has_color_grading();
    let has_hue = adjustment.hue.abs() > 1e-6;
    let (hsl_hue_0, hsl_hue_1) = split_eight(adjustment.hsl_hue);
    let (hsl_saturation_0, hsl_saturation_1) = split_eight(adjustment.hsl_saturation);
    let (hsl_luminance_0, hsl_luminance_1) = split_eight(adjustment.hsl_luminance);
    let mut point_colors = [PackedPointColor::zeroed(); MAX_POINT_COLORS];
    for (destination, point) in point_colors.iter_mut().zip(adjustment.point_colors.iter()) {
        *destination = pack_point_color(*point);
    }
    MaskData {
        metadata: [
            u32::from(adjustment_enabled),
            u32::from(!adjustment.is_neutral()),
            curve_flags,
            u32::from(has_hsl) | (u32::from(has_grading) << 1) | (u32::from(has_hue) << 2),
        ],
        adjust_0: [
            effect_params::adjustment::EXPOSURE.clamp(adjustment.exposure),
            effect_params::adjustment::CONTRAST.clamp(adjustment.contrast),
            effect_params::adjustment::HIGHLIGHTS.clamp(adjustment.highlights),
            effect_params::adjustment::SHADOWS.clamp(adjustment.shadows),
        ],
        adjust_1: [
            effect_params::adjustment::WHITES.clamp(adjustment.whites),
            effect_params::adjustment::BLACKS.clamp(adjustment.blacks),
            effect_params::adjustment::TEMPERATURE.clamp(adjustment.temperature),
            effect_params::adjustment::TINT.clamp(adjustment.tint),
        ],
        adjust_2: [
            effect_params::adjustment::SATURATION.clamp(adjustment.saturation),
            effect_params::adjustment::TEXTURE.clamp(adjustment.texture),
            effect_params::adjustment::CLARITY.clamp(adjustment.clarity),
            effect_params::adjustment::DEHAZE.clamp(adjustment.dehaze),
        ],
        effect_options: [0.0; 4],
        curves: pack_local_point_curve(&adjustment.tone_curve),
        grade_shadows: pack_color_grade_wheel(adjustment.color_grading.shadows),
        grade_midtones: pack_color_grade_wheel(adjustment.color_grading.midtones),
        grade_highlights: pack_color_grade_wheel(adjustment.color_grading.highlights),
        grade_global: pack_color_grade_wheel(adjustment.color_grading.global),
        grade_options: pack_view_color_options(adjustment.color_grading, adjustment.hue),
        curves_red: pack_local_point_curve(&adjustment.tone_curve_red),
        curves_green: pack_local_point_curve(&adjustment.tone_curve_green),
        curves_blue: pack_local_point_curve(&adjustment.tone_curve_blue),
        hsl_hue_0,
        hsl_hue_1,
        hsl_saturation_0,
        hsl_saturation_1,
        hsl_luminance_0,
        hsl_luminance_1,
        point_colors,
        point_color_meta: [
            adjustment.point_colors.len() as u32,
            adjustment
                .point_color_visualize
                .map_or(0, |index| (index + 1) as u32),
            0,
            0,
        ],
    }
}

pub(super) fn render_mask_slot_count(masks: &MaskStack) -> usize {
    masks
        .masks
        .iter()
        .map(|mask| 1 + mask.effect_components.len())
        .sum::<usize>()
        + masks.global_effects.len()
}

pub(super) fn pack_mask_params(masks: &MaskStack, shutter_seconds: f32) -> Box<[MaskData]> {
    let mut packed = vec![MaskData::zeroed(); MAX_RENDER_MASK_SLOTS].into_boxed_slice();
    let mut slot = 0;
    for (layer, mask) in masks.masks.iter().enumerate() {
        if mask.effect == MaskEffect::Adjustment {
            packed[slot] = pack_adjustment_mask(mask);
            packed[slot].point_color_meta[2] = layer as u32;
            slot += 1;
        } else if let Some(mut data) = pack_effect_mask(
            mask.effect,
            &mask.effect_settings,
            mask.enabled,
            shutter_seconds,
        ) {
            data.point_color_meta[2] = layer as u32;
            packed[slot] = data;
            slot += 1;
        }
        for component in &mask.effect_components {
            if let Some(mut data) = pack_effect_mask(
                component.effect,
                &component.settings,
                mask.enabled && component.enabled,
                shutter_seconds,
            ) {
                data.point_color_meta[2] = layer as u32;
                packed[slot] = data;
                slot += 1;
            }
        }
    }
    for component in &masks.global_effects {
        if let Some(mut data) = pack_effect_mask(
            component.effect,
            &component.settings,
            component.enabled,
            shutter_seconds,
        ) {
            data.point_color_meta[2] = u32::MAX;
            packed[slot] = data;
            slot += 1;
        }
    }
    assign_scene_light_channels(&mut packed[..slot]);
    packed
}
