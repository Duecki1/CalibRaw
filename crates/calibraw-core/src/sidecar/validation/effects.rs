//! Validation of mask effect settings and their parameters.

use super::*;

pub(super) fn validate_effect_settings(
    settings: &crate::pipeline::MaskEffectSettings,
) -> Result<(), SidecarError> {
    validate_blur_effect(&settings.blur)?;
    validate_lens_blur_effect(&settings.lens_blur)?;
    validate_motion_blur_effect(&settings.motion_blur)?;
    validate_radial_blur_effect(&settings.radial_blur)?;
    validate_tilt_shift_effect(&settings.tilt_shift)?;
    validate_edge_glow_effect(&settings.edge_glow)?;
    validate_glow_effect(&settings.glow)?;
    validate_light_rays_effect(&settings.light_rays)?;
    validate_relight_effect(&settings.relight)?;
    validate_neon_effect(&settings.neon)?;
    validate_pixelate_effect(&settings.pixelate)?;
    validate_fog_effect(&settings.fog)?;
    validate_smoke_effect(&settings.smoke)?;
    validate_grain_effect(&settings.grain)?;
    validate_halation_effect(&settings.halation)?;
    validate_vignette_effect(&settings.vignette)?;
    Ok(())
}

fn validate_grain_effect(grain: &crate::pipeline::GrainEffectSettings) -> Result<(), SidecarError> {
    use crate::pipeline::effect_params::grain;
    validate_effect_params(
        crate::pipeline::MaskEffect::Grain,
        &[
            (grain::AMOUNT, grain.amount),
            (grain::SIZE, grain.size),
            (grain::ROUGHNESS, grain.roughness),
            (grain::COLOR, grain.color),
            (grain::SEED, grain.seed),
        ],
        &[],
    )
}

fn validate_halation_effect(
    halation: &crate::pipeline::HalationEffectSettings,
) -> Result<(), SidecarError> {
    use crate::pipeline::effect_params::halation;
    validate_effect_params(
        crate::pipeline::MaskEffect::Halation,
        &[
            (halation::AMOUNT, halation.amount),
            (halation::RADIUS, halation.radius),
            (halation::THRESHOLD, halation.threshold),
            (halation::WARMTH, halation.warmth),
        ],
        &[],
    )
}

fn validate_vignette_effect(
    vignette: &crate::pipeline::VignetteEffectSettings,
) -> Result<(), SidecarError> {
    use crate::pipeline::effect_params::vignette;
    validate_effect_params(
        crate::pipeline::MaskEffect::Vignette,
        &[
            (vignette::AMOUNT, vignette.amount),
            (vignette::MIDPOINT, vignette.midpoint),
            (vignette::ROUNDNESS, vignette.roundness),
            (vignette::FEATHER, vignette.feather),
            (vignette::HIGHLIGHTS, vignette.highlights),
            (vignette::CENTER_X, vignette.center[0]),
            (vignette::CENTER_Y, vignette.center[1]),
        ],
        &[],
    )
}

fn validate_blur_effect(blur: &crate::pipeline::BlurEffectSettings) -> Result<(), SidecarError> {
    use crate::pipeline::effect_params::blur;
    validate_effect_params(
        crate::pipeline::MaskEffect::Blur,
        &[(blur::AMOUNT, blur.amount), (blur::RADIUS, blur.radius)],
        &[],
    )
}

fn validate_lens_blur_effect(
    lens_blur: &crate::pipeline::LensBlurEffectSettings,
) -> Result<(), SidecarError> {
    use crate::pipeline::effect_params::lens_blur;
    validate_effect_params(
        crate::pipeline::MaskEffect::LensBlur,
        &[
            (lens_blur::AMOUNT, lens_blur.amount),
            (lens_blur::RADIUS, lens_blur.radius),
            (lens_blur::BLADES, lens_blur.blades),
            (lens_blur::ROTATION, lens_blur.rotation),
            (lens_blur::HIGHLIGHTS, lens_blur.highlight_boost),
        ],
        &[],
    )
}

fn validate_motion_blur_effect(
    motion_blur: &crate::pipeline::MotionBlurEffectSettings,
) -> Result<(), SidecarError> {
    use crate::pipeline::effect_params::motion_blur;
    validate_effect_params(
        crate::pipeline::MaskEffect::MotionBlur,
        &[
            (motion_blur::AMOUNT, motion_blur.amount),
            (motion_blur::DISTANCE, motion_blur.distance),
            (motion_blur::ANGLE, motion_blur.angle),
        ],
        &[],
    )
}

fn validate_radial_blur_effect(
    radial_blur: &crate::pipeline::RadialBlurEffectSettings,
) -> Result<(), SidecarError> {
    use crate::pipeline::effect_params::radial_blur;
    validate_effect_params(
        crate::pipeline::MaskEffect::RadialBlur,
        &[
            (radial_blur::AMOUNT, radial_blur.amount),
            (radial_blur::STRENGTH, radial_blur.strength),
            (radial_blur::CENTER_X, radial_blur.center[0]),
            (radial_blur::CENTER_Y, radial_blur.center[1]),
        ],
        &[],
    )
}

fn validate_tilt_shift_effect(
    tilt_shift: &crate::pipeline::TiltShiftEffectSettings,
) -> Result<(), SidecarError> {
    use crate::pipeline::effect_params::tilt_shift;
    validate_effect_params(
        crate::pipeline::MaskEffect::TiltShift,
        &[
            (tilt_shift::AMOUNT, tilt_shift.amount),
            (tilt_shift::RADIUS, tilt_shift.radius),
            (tilt_shift::CENTER_X, tilt_shift.center[0]),
            (tilt_shift::CENTER_Y, tilt_shift.center[1]),
            (tilt_shift::ANGLE, tilt_shift.angle),
            (tilt_shift::FOCUS_WIDTH, tilt_shift.focus_width),
            (tilt_shift::FEATHER, tilt_shift.feather),
        ],
        &[],
    )
}

fn validate_edge_glow_effect(
    edge_glow: &crate::pipeline::EdgeGlowEffectSettings,
) -> Result<(), SidecarError> {
    use crate::pipeline::effect_params::edge_glow;
    validate_effect_params(
        crate::pipeline::MaskEffect::EdgeGlow,
        &[
            (edge_glow::AMOUNT, edge_glow.amount),
            (edge_glow::EDGE_WIDTH, edge_glow.edge_width),
            (edge_glow::DETAIL, edge_glow.detail),
            (edge_glow::GLOW, edge_glow.glow),
        ],
        &edge_glow.color,
    )?;
    validate_effect_color(
        crate::pipeline::MaskEffect::EdgeGlow,
        edge_glow::COLOR,
        edge_glow.color,
    )
}

fn validate_pixelate_effect(
    pixelate: &crate::pipeline::PixelateEffectSettings,
) -> Result<(), SidecarError> {
    use crate::pipeline::effect_params::pixelate;
    validate_effect_params(
        crate::pipeline::MaskEffect::Pixelate,
        &[
            (pixelate::AMOUNT, pixelate.amount),
            (pixelate::BLOCK_SIZE, pixelate.block_size),
        ],
        &[],
    )
}

fn validate_fog_effect(fog: &crate::pipeline::FogEffectSettings) -> Result<(), SidecarError> {
    use crate::pipeline::effect_params::fog;
    validate_effect_params(
        crate::pipeline::MaskEffect::Fog,
        &[
            (fog::AMOUNT, fog.amount),
            (fog::DENSITY, fog.density),
            (fog::SCALE, fog.scale),
            (fog::SOFTNESS, fog.softness),
            (fog::VARIATION, fog.variation),
            (fog::SEED, fog.seed),
            (fog::START, fog.start),
            (fog::DEPTH_INFLUENCE, fog.depth_influence),
        ],
        &fog.color,
    )?;
    validate_effect_color(crate::pipeline::MaskEffect::Fog, fog::COLOR, fog.color)
}

fn validate_smoke_effect(smoke: &crate::pipeline::SmokeEffectSettings) -> Result<(), SidecarError> {
    use crate::pipeline::effect_params::smoke;
    validate_effect_params(
        crate::pipeline::MaskEffect::Smoke,
        &[
            (smoke::AMOUNT, smoke.amount),
            (smoke::DENSITY, smoke.density),
            (smoke::SCALE, smoke.scale),
            (smoke::TURBULENCE, smoke.turbulence),
            (smoke::SOFTNESS, smoke.softness),
            (smoke::ANGLE, smoke.angle),
            (smoke::SEED, smoke.seed),
        ],
        &smoke.color,
    )?;
    validate_effect_color(
        crate::pipeline::MaskEffect::Smoke,
        smoke::COLOR,
        smoke.color,
    )
}

fn validate_glow_effect(glow: &crate::pipeline::GlowEffectSettings) -> Result<(), SidecarError> {
    use crate::pipeline::effect_params::glow;
    validate_effect_params(
        crate::pipeline::MaskEffect::Glow,
        &[
            (glow::AMOUNT, glow.amount),
            (glow::RADIUS, glow.radius),
            (glow::CORE, glow.core),
        ],
        &glow.color,
    )?;
    validate_effect_color(crate::pipeline::MaskEffect::Glow, glow::COLOR, glow.color)
}

fn validate_light_rays_effect(
    light_rays: &crate::pipeline::LightRaysEffectSettings,
) -> Result<(), SidecarError> {
    use crate::pipeline::effect_params::light_rays;
    validate_effect_params(
        crate::pipeline::MaskEffect::LightRays,
        &[
            (light_rays::AMOUNT, light_rays.amount),
            (light_rays::LENGTH, light_rays.length),
            (light_rays::SOURCE_X, light_rays.source[0]),
            (light_rays::SOURCE_Y, light_rays.source[1]),
            (light_rays::SPREAD, light_rays.spread),
            (light_rays::FADE, light_rays.fade),
            (light_rays::RAY_COUNT, light_rays.ray_count),
            (light_rays::VARIATION, light_rays.variation),
            (light_rays::SOFTNESS, light_rays.softness),
        ],
        &light_rays.color,
    )?;
    validate_effect_color(
        crate::pipeline::MaskEffect::LightRays,
        light_rays::COLOR,
        light_rays.color,
    )
}

fn validate_relight_effect(
    relight: &crate::pipeline::RelightEffectSettings,
) -> Result<(), SidecarError> {
    use crate::pipeline::effect_params::relight as params;
    validate_effect_params(
        crate::pipeline::MaskEffect::Relight,
        &[
            (params::AMOUNT, relight.amount),
            (params::SOURCE_X, relight.source[0]),
            (params::SOURCE_Y, relight.source[1]),
            (params::DEPTH, relight.depth),
            (params::REACH, relight.reach),
            (params::SIZE, relight.size),
            (params::SHADOWS, relight.shadows),
            (params::RELIEF, relight.relief),
            (params::AMBIENT, relight.ambient),
        ],
        &relight.color,
    )?;
    validate_effect_color(
        crate::pipeline::MaskEffect::Relight,
        params::COLOR,
        relight.color,
    )
}

pub(super) fn validate_effect_params(
    effect: crate::pipeline::MaskEffect,
    params: &[(crate::pipeline::effect_params::FloatParamSpec, f32)],
    extra_finite: &[f32],
) -> Result<(), SidecarError> {
    if params
        .iter()
        .map(|(_, value)| value)
        .chain(extra_finite)
        .all(|value| value.is_finite())
    {
        for (spec, value) in params {
            bounded(
                &format!("{} {}", effect.label(), spec.label),
                *value,
                spec.min,
                spec.max,
            )?;
        }
        Ok(())
    } else {
        invalid(&format!(
            "{} mask effect contains a non-finite value",
            effect.label()
        ))
    }
}

pub(super) fn validate_effect_color(
    effect: crate::pipeline::MaskEffect,
    spec: crate::pipeline::effect_params::ColorParamSpec,
    color: [f32; 3],
) -> Result<(), SidecarError> {
    for channel in color {
        bounded(
            &format!("{} {} channel", effect.label(), spec.label),
            channel,
            spec.min,
            spec.max,
        )?;
    }
    Ok(())
}

pub(super) fn validate_curves(
    curves: &[&crate::pipeline::PointCurve],
    label: &str,
) -> Result<(), SidecarError> {
    for curve in curves {
        if !(2..=crate::pipeline::MAX_POINT_CURVE_POINTS as u32).contains(&curve.len) {
            return invalid("tone curve point count is invalid");
        }
        for point in curve.points {
            finite(label, &point)?;
        }
    }
    Ok(())
}
