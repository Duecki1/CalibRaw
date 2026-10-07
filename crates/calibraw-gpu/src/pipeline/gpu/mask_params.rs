//! Packing of local adjustment masks and mask effects into `MaskData` slots.

use super::*;

fn effect_mask_data(
    effect: MaskEffect,
    active: bool,
    adjust_0: [f32; 4],
    adjust_1: [f32; 4],
    adjust_2: [f32; 4],
) -> MaskData {
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
        ..MaskData::zeroed()
    }
}

pub(super) fn pack_effect_mask(
    effect: MaskEffect,
    settings: &crate::pipeline::MaskEffectSettings,
    enabled: bool,
) -> Option<MaskData> {
    let zero = [0.0; 4];
    let data = match effect {
        MaskEffect::Blur => {
            let config = settings.blur;
            effect_mask_data(
                effect,
                enabled && config.is_active(),
                [
                    effect_params::blur::AMOUNT.clamp(config.amount),
                    effect_params::blur::RADIUS.clamp(config.radius),
                    0.0,
                    0.0,
                ],
                zero,
                zero,
            )
        }
        MaskEffect::LensBlur => {
            let config = settings.lens_blur;
            effect_mask_data(
                effect,
                enabled && config.is_active(),
                [
                    effect_params::lens_blur::AMOUNT.clamp(config.amount),
                    effect_params::lens_blur::RADIUS.clamp(config.radius),
                    effect_params::lens_blur::BLADES
                        .clamp(config.blades)
                        .round(),
                    effect_params::lens_blur::ROTATION.clamp(config.rotation),
                ],
                [
                    effect_params::lens_blur::HIGHLIGHTS.clamp(config.highlight_boost),
                    0.0,
                    0.0,
                    0.0,
                ],
                zero,
            )
        }
        MaskEffect::MotionBlur => {
            let config = settings.motion_blur;
            effect_mask_data(
                effect,
                enabled && config.is_active(),
                [
                    effect_params::motion_blur::AMOUNT.clamp(config.amount),
                    effect_params::motion_blur::DISTANCE.clamp(config.distance),
                    effect_params::motion_blur::ANGLE.clamp(config.angle),
                    0.0,
                ],
                zero,
                zero,
            )
        }
        MaskEffect::RadialBlur => {
            let config = settings.radial_blur;
            effect_mask_data(
                effect,
                enabled && config.is_active(),
                [
                    effect_params::radial_blur::AMOUNT.clamp(config.amount),
                    effect_params::radial_blur::STRENGTH.clamp(config.strength),
                    effect_params::radial_blur::CENTER_X.clamp(config.center[0]),
                    effect_params::radial_blur::CENTER_Y.clamp(config.center[1]),
                ],
                [config.mode.shader_value(), 0.0, 0.0, 0.0],
                zero,
            )
        }
        MaskEffect::TiltShift => {
            let config = settings.tilt_shift;
            effect_mask_data(
                effect,
                enabled && config.is_active(),
                [
                    effect_params::tilt_shift::AMOUNT.clamp(config.amount),
                    effect_params::tilt_shift::RADIUS.clamp(config.radius),
                    effect_params::tilt_shift::CENTER_X.clamp(config.center[0]),
                    effect_params::tilt_shift::CENTER_Y.clamp(config.center[1]),
                ],
                [
                    effect_params::tilt_shift::ANGLE.clamp(config.angle),
                    effect_params::tilt_shift::FOCUS_WIDTH.clamp(config.focus_width),
                    effect_params::tilt_shift::FEATHER.clamp(config.feather),
                    0.0,
                ],
                zero,
            )
        }
        MaskEffect::EdgeGlow => {
            let config = settings.edge_glow;
            let color = effect_params::edge_glow::COLOR.clamp(config.color);
            effect_mask_data(
                effect,
                enabled && config.is_active(),
                [
                    effect_params::edge_glow::AMOUNT.clamp(config.amount),
                    effect_params::edge_glow::EDGE_WIDTH.clamp(config.edge_width),
                    effect_params::edge_glow::DETAIL.clamp(config.detail),
                    effect_params::edge_glow::GLOW.clamp(config.glow),
                ],
                [color[0], color[1], color[2], 0.0],
                zero,
            )
        }
        MaskEffect::Glow => {
            let config = settings.glow;
            let color = effect_params::glow::COLOR.clamp(config.color);
            effect_mask_data(
                effect,
                enabled && config.is_active(),
                [
                    effect_params::glow::AMOUNT.clamp(config.amount),
                    effect_params::glow::RADIUS.clamp(config.radius),
                    effect_params::glow::CORE.clamp(config.core),
                    if config.self_illuminating { 1.0 } else { 0.0 },
                ],
                [color[0], color[1], color[2], 0.0],
                zero,
            )
        }
        MaskEffect::Neon => {
            let config = settings.neon;
            let color = effect_params::neon::COLOR.clamp(config.color);
            effect_mask_data(
                effect,
                enabled && config.is_active(),
                [
                    effect_params::neon::AMOUNT.clamp(config.amount),
                    effect_params::neon::EDGE_WIDTH.clamp(config.edge_width),
                    effect_params::neon::DETAIL.clamp(config.detail),
                    effect_params::neon::GLOW.clamp(config.glow),
                ],
                [
                    color[0],
                    color[1],
                    color[2],
                    effect_params::neon::BACKGROUND.clamp(config.background),
                ],
                zero,
            )
        }
        MaskEffect::LightRays => {
            let config = settings.light_rays;
            let color = effect_params::light_rays::COLOR.clamp(config.color);
            effect_mask_data(
                effect,
                enabled && config.is_active(),
                [
                    effect_params::light_rays::AMOUNT.clamp(config.amount),
                    effect_params::light_rays::LENGTH.clamp(config.length),
                    effect_params::light_rays::SOURCE_X.clamp(config.source[0]),
                    effect_params::light_rays::SOURCE_Y.clamp(config.source[1]),
                ],
                [
                    color[0],
                    color[1],
                    color[2],
                    effect_params::light_rays::FADE.clamp(config.fade),
                ],
                [
                    effect_params::light_rays::SPREAD.clamp(config.spread),
                    effect_params::light_rays::RAY_COUNT.clamp(config.ray_count),
                    effect_params::light_rays::VARIATION.clamp(config.variation),
                    effect_params::light_rays::SOFTNESS.clamp(config.softness),
                ],
            )
        }
        MaskEffect::Relight => {
            let config = settings.relight;
            use effect_params::relight as params;
            let color = params::COLOR.clamp(config.color);
            // Layout read by `apply_relight` in relight.wgsl.
            effect_mask_data(
                effect,
                enabled && config.is_active(),
                [
                    params::AMOUNT.clamp(config.amount),
                    params::REACH.clamp(config.reach),
                    params::SOURCE_X.clamp(config.source[0]),
                    params::SOURCE_Y.clamp(config.source[1]),
                ],
                [
                    color[0],
                    color[1],
                    color[2],
                    params::DEPTH.clamp(config.depth),
                ],
                [
                    params::SIZE.clamp(config.size),
                    if config.shadows_enabled {
                        params::SHADOWS.clamp(config.shadows)
                    } else {
                        0.0
                    },
                    params::RELIEF.clamp(config.relief),
                    params::AMBIENT.clamp(config.ambient),
                ],
            )
        }
        MaskEffect::Pixelate => {
            let config = settings.pixelate;
            effect_mask_data(
                effect,
                enabled && config.is_active(),
                [
                    effect_params::pixelate::AMOUNT.clamp(config.amount),
                    effect_params::pixelate::BLOCK_SIZE.clamp(config.block_size),
                    0.0,
                    0.0,
                ],
                zero,
                zero,
            )
        }
        MaskEffect::Fog => {
            let config = settings.fog;
            let color = effect_params::fog::COLOR.clamp(config.color);
            effect_mask_data(
                effect,
                enabled && config.is_active(),
                [
                    effect_params::fog::AMOUNT.clamp(config.amount),
                    effect_params::fog::DENSITY.clamp(config.density),
                    effect_params::fog::SCALE.clamp(config.scale),
                    effect_params::fog::SOFTNESS.clamp(config.softness),
                ],
                [
                    color[0],
                    color[1],
                    color[2],
                    effect_params::fog::VARIATION.clamp(config.variation),
                ],
                [
                    effect_params::fog::SEED.clamp(config.seed),
                    if config.depth_enabled {
                        effect_params::fog::START.clamp(config.start)
                    } else {
                        0.0
                    },
                    if config.depth_enabled {
                        effect_params::fog::DEPTH_INFLUENCE.clamp(config.depth_influence)
                    } else {
                        0.0
                    },
                    0.0,
                ],
            )
        }
        MaskEffect::Smoke => {
            let config = settings.smoke;
            let color = effect_params::smoke::COLOR.clamp(config.color);
            effect_mask_data(
                effect,
                enabled && config.is_active(),
                [
                    effect_params::smoke::AMOUNT.clamp(config.amount),
                    effect_params::smoke::DENSITY.clamp(config.density),
                    effect_params::smoke::SCALE.clamp(config.scale),
                    effect_params::smoke::TURBULENCE.clamp(config.turbulence),
                ],
                [
                    color[0],
                    color[1],
                    color[2],
                    effect_params::smoke::ANGLE.clamp(config.angle),
                ],
                [
                    effect_params::smoke::SOFTNESS.clamp(config.softness),
                    effect_params::smoke::SEED.clamp(config.seed),
                    0.0,
                    0.0,
                ],
            )
        }
        MaskEffect::Grain => {
            let config = settings.grain;
            use effect_params::grain as params;
            effect_mask_data(
                effect,
                enabled && config.is_active(),
                [
                    params::AMOUNT.clamp(config.amount),
                    params::SIZE.clamp(config.size),
                    params::ROUGHNESS.clamp(config.roughness),
                    params::COLOR.clamp(config.color),
                ],
                [params::SEED.clamp(config.seed).round(), 0.0, 0.0, 0.0],
                zero,
            )
        }
        MaskEffect::Halation => {
            let config = settings.halation;
            use effect_params::halation as params;
            effect_mask_data(
                effect,
                enabled && config.is_active(),
                [
                    params::AMOUNT.clamp(config.amount),
                    params::RADIUS.clamp(config.radius),
                    params::THRESHOLD.clamp(config.threshold),
                    params::WARMTH.clamp(config.warmth),
                ],
                zero,
                zero,
            )
        }
        MaskEffect::Vignette => {
            let config = settings.vignette;
            use effect_params::vignette as params;
            effect_mask_data(
                effect,
                enabled && config.is_active(),
                [
                    params::AMOUNT.clamp(config.amount),
                    params::MIDPOINT.clamp(config.midpoint),
                    params::ROUNDNESS.clamp(config.roundness),
                    params::FEATHER.clamp(config.feather),
                ],
                [
                    params::HIGHLIGHTS.clamp(config.highlights),
                    params::CENTER_X.clamp(config.center[0]),
                    params::CENTER_Y.clamp(config.center[1]),
                    0.0,
                ],
                zero,
            )
        }
        MaskEffect::Adjustment => return None,
    };
    Some(data)
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
        film_effects: [
            effect_params::adjustment::HALATION.clamp(adjustment.halation_amount),
            0.0,
            0.0,
            0.0,
        ],
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

pub(super) fn pack_mask_params(masks: &MaskStack) -> Box<[MaskData]> {
    let mut packed = vec![MaskData::zeroed(); MAX_RENDER_MASK_SLOTS].into_boxed_slice();
    let mut slot = 0;
    for (layer, mask) in masks.masks.iter().enumerate() {
        if mask.effect == MaskEffect::Adjustment {
            packed[slot] = pack_adjustment_mask(mask);
            packed[slot].point_color_meta[2] = layer as u32;
            slot += 1;
        } else if let Some(mut data) =
            pack_effect_mask(mask.effect, &mask.effect_settings, mask.enabled)
        {
            data.point_color_meta[2] = layer as u32;
            packed[slot] = data;
            slot += 1;
        }
        for component in &mask.effect_components {
            if let Some(mut data) = pack_effect_mask(
                component.effect,
                &component.settings,
                mask.enabled && component.enabled,
            ) {
                data.point_color_meta[2] = layer as u32;
                packed[slot] = data;
                slot += 1;
            }
        }
    }
    for component in &masks.global_effects {
        if let Some(mut data) =
            pack_effect_mask(component.effect, &component.settings, component.enabled)
        {
            data.point_color_meta[2] = u32::MAX;
            packed[slot] = data;
            slot += 1;
        }
    }
    packed
}
