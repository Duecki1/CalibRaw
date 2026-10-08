use super::*;

mod effects;
use effects::*;

pub(super) fn validate_edit_state(edits: &EditState) -> Result<(), SidecarError> {
    validate_exposure(&edits.exposure)?;
    if let Some(profile) = &edits.camera_profile {
        if profile.as_os_str().len() > MAX_EDIT_NAME_BYTES * 4 {
            return invalid("camera profile path is unreasonably long");
        }
        if profile.is_absolute()
            || profile.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            return invalid("camera profile path must stay inside the configured profile folder");
        }
    }
    let stack = &edits.masks;
    if stack.masks.len() > MAX_LOCAL_MASKS {
        return invalid("sidecar contains too many local masks");
    }
    if stack
        .selected_mask
        .is_some_and(|index| index >= stack.masks.len())
    {
        return invalid("selected mask index is out of range");
    }
    if edits.lens.maker.len() > MAX_EDIT_NAME_BYTES || edits.lens.model.len() > MAX_EDIT_NAME_BYTES
    {
        return invalid("lens name is unreasonably long");
    }

    if edits.remove.strokes.len() > crate::pipeline::REMOVE_MAX_STROKES {
        return invalid("sidecar contains too many Remove strokes");
    }
    for stroke in &edits.remove.strokes {
        finite("Remove stroke opacity", &[stroke.opacity])?;
        bounded("Remove stroke opacity", stroke.opacity, 0.0, 1.0)?;
        if stroke.brush.points.len() > crate::pipeline::REMOVE_MAX_POINTS_PER_STROKE {
            return invalid("Remove stroke contains too many brush points");
        }
        if stroke.patches.len() > crate::pipeline::REMOVE_MAX_PATCHES_PER_STROKE {
            return invalid("Remove stroke contains too many cached patches");
        }
        if stroke.brush.dilation_radius > 64 {
            return invalid("Remove stroke dilation is unreasonably large");
        }
        if let Some(retouch) = stroke.retouch {
            finite(
                "retouch stroke settings",
                &[
                    retouch.source[0],
                    retouch.source[1],
                    retouch.destination[0],
                    retouch.destination[1],
                    retouch.hardness,
                    retouch.opacity,
                ],
            )?;
            for value in retouch.source.into_iter().chain(retouch.destination) {
                bounded("retouch source or destination", value, -1.0, 1_000_000.0)?;
            }
            bounded("retouch hardness", retouch.hardness, 0.0, 1.0)?;
            bounded("retouch opacity", retouch.opacity, 0.0, 1.0)?;
        }
        for point in &stroke.brush.points {
            finite("Remove brush point", &[point.x, point.y, point.radius])?;
            bounded("Remove brush x", point.x, -1.0, 1_000_000.0)?;
            bounded("Remove brush y", point.y, -1.0, 1_000_000.0)?;
            bounded("Remove brush radius", point.radius, 0.0, 100_000.0)?;
        }
        for patch in &stroke.patches {
            if patch.bounds.width == 0
                || patch.bounds.height == 0
                || patch.bounds.width > 32_768
                || patch.bounds.height > 32_768
            {
                return invalid("Remove patch has invalid dimensions");
            }
            if patch.bounds.x.checked_add(patch.bounds.width).is_none()
                || patch.bounds.y.checked_add(patch.bounds.height).is_none()
            {
                return invalid("Remove patch bounds overflow native coordinates");
            }
            let pixels = (patch.bounds.width as usize)
                .checked_mul(patch.bounds.height as usize)
                .ok_or_else(|| {
                    SidecarError::Invalid("Remove patch dimensions overflow".to_owned())
                })?;
            let rgb_values = pixels.saturating_mul(3);
            if patch.rgb_scene16f.len() != rgb_values || patch.alpha.len() != pixels {
                return invalid("Remove patch payload does not match its dimensions");
            }
        }
    }

    let refinement = &stack.subject_refinement;
    finite(
        "subject refinement settings",
        &[refinement.size, refinement.feather, refinement.flow],
    )?;
    bounded("subject refinement size", refinement.size, 0.0, 16.0)?;
    bounded("subject refinement feather", refinement.feather, 0.0, 1.0)?;
    bounded("subject refinement flow", refinement.flow, 0.0, 1.0)?;
    if refinement.dabs.len() > MAX_BRUSH_DABS {
        return invalid("subject refinement contains too many dabs");
    }
    let mut previous_start = None;
    for &start in &refinement.stroke_starts {
        if start >= refinement.dabs.len()
            || previous_start.is_some_and(|previous| start <= previous)
        {
            return invalid("subject refinement contains invalid stroke boundaries");
        }
        previous_start = Some(start);
    }
    for dab in &refinement.dabs {
        finite(
            "subject refinement dab",
            &[
                dab.center[0],
                dab.center[1],
                dab.opacity,
                dab.size,
                dab.feather,
            ],
        )?;
        bounded("subject refinement dab x", dab.center[0], -16.0, 16.0)?;
        bounded("subject refinement dab y", dab.center[1], -16.0, 16.0)?;
        bounded("subject refinement dab opacity", dab.opacity, -1.0, 1.0)?;
        bounded("subject refinement dab size", dab.size, 0.0, 16.0)?;
        bounded("subject refinement dab feather", dab.feather, 0.0, 1.0)?;
    }

    validate_effect_components(&stack.global_effects)?;

    for (mask_index, mask) in stack.masks.iter().enumerate() {
        finite("mask opacity", &[mask.opacity])?;
        if !(0.0..=1.0).contains(&mask.opacity) {
            return invalid("mask opacity is outside 0..1");
        }
        validate_local_adjustments(&mask.adjustments)?;
        validate_effect_settings(&mask.effect_settings)?;
        validate_effect_components(&mask.effect_components)?;
        if mask.name.len() > MAX_EDIT_NAME_BYTES {
            return invalid("mask name is unreasonably long");
        }
        if mask.components.is_empty() || mask.components.len() > MAX_MASK_COMPONENTS {
            return invalid("mask has an invalid component count");
        }
        if stack.selected_mask == Some(mask_index)
            && stack
                .selected_component
                .is_some_and(|index| index >= mask.components.len())
        {
            return invalid("selected mask component index is out of range");
        }
        for component in &mask.components {
            if component.name.len() > MAX_EDIT_NAME_BYTES {
                return invalid("mask component name is unreasonably long");
            }
            if !geometry_matches_kind(component.kind, &component.geometry) {
                return invalid("mask component kind and geometry do not agree");
            }
            match &component.geometry {
                MaskGeometry::Brush {
                    size,
                    feather,
                    opacity,
                    stroke_starts,
                    dabs,
                    ..
                } => {
                    finite("brush geometry", &[*size, *feather, *opacity])?;
                    bounded("brush size", *size, 0.0, 16.0)?;
                    bounded("brush feather", *feather, 0.0, 1.0)?;
                    bounded("brush opacity", *opacity, 0.0, 1.0)?;
                    if dabs.len() > MAX_BRUSH_DABS {
                        return invalid("brush mask contains too many dabs");
                    }
                    let mut previous_start = None;
                    for &start in stroke_starts {
                        if start >= dabs.len()
                            || previous_start.is_some_and(|previous| start <= previous)
                        {
                            return invalid("brush mask contains invalid stroke boundaries");
                        }
                        previous_start = Some(start);
                    }
                    for dab in dabs {
                        finite(
                            "brush dab",
                            &[
                                dab.center[0],
                                dab.center[1],
                                dab.opacity,
                                dab.size,
                                dab.feather,
                            ],
                        )?;
                        bounded("brush dab x", dab.center[0], -16.0, 16.0)?;
                        bounded("brush dab y", dab.center[1], -16.0, 16.0)?;
                        bounded("brush dab opacity", dab.opacity, -1.0, 1.0)?;
                        bounded("brush dab size", dab.size, 0.0, 16.0)?;
                        bounded("brush dab feather", dab.feather, 0.0, 1.0)?;
                    }
                }
                MaskGeometry::Radial {
                    center,
                    radius,
                    rotation,
                    feather,
                    ..
                } => {
                    finite(
                        "radial geometry",
                        &[
                            center[0], center[1], radius[0], radius[1], *rotation, *feather,
                        ],
                    )?;
                    for value in center {
                        bounded("radial center", *value, -16.0, 16.0)?;
                    }
                    for value in radius {
                        bounded("radial radius", *value, 0.0, 16.0)?;
                    }
                    bounded("radial rotation", *rotation, -1_000_000.0, 1_000_000.0)?;
                    bounded("radial feather", *feather, 0.0, 1.0)?;
                }
                MaskGeometry::Linear {
                    start,
                    end,
                    feather,
                    ..
                } => {
                    finite(
                        "linear geometry",
                        &[start[0], start[1], end[0], end[1], *feather],
                    )?;
                    for value in start.iter().chain(end.iter()) {
                        bounded("linear point", *value, -16.0, 16.0)?;
                    }
                    bounded("linear feather", *feather, 0.0, 16.0)?;
                }
                MaskGeometry::Path {
                    points,
                    grow,
                    feather,
                } => {
                    finite("path shape", &[*grow, *feather])?;
                    bounded("path grow", *grow, -1.0, 1.0)?;
                    bounded("path feather", *feather, 0.0, 1.0)?;
                    if points.len() > MAX_PATH_POINTS {
                        return invalid("path mask contains too many points");
                    }
                    for point in points {
                        finite(
                            "path point",
                            &[
                                point.position[0],
                                point.position[1],
                                point.handle_in[0],
                                point.handle_in[1],
                                point.handle_out[0],
                                point.handle_out[1],
                            ],
                        )?;
                        for value in point.position {
                            bounded("path point position", value, -16.0, 16.0)?;
                        }
                        for value in point.handle_in.into_iter().chain(point.handle_out) {
                            bounded("path point handle", value, -32.0, 32.0)?;
                        }
                    }
                }
                MaskGeometry::Ai {
                    mask,
                    grow,
                    feather,
                } => {
                    finite("AI mask settings", &[*grow, *feather])?;
                    bounded("AI mask grow", *grow, -1.0, 1.0)?;
                    bounded("AI mask feather", *feather, 0.0, 1.0)?;
                    if let Some(image) = mask {
                        validate_image(image.width, image.height, image.pixels.len(), 1)?;
                    }
                }
                MaskGeometry::Object {
                    mask,
                    grow,
                    feather,
                    brush_size,
                    edge_refine,
                    strokes,
                    ..
                } => {
                    finite(
                        "object mask settings",
                        &[*grow, *feather, *brush_size, *edge_refine],
                    )?;
                    bounded("object mask grow", *grow, -1.0, 1.0)?;
                    bounded("object mask feather", *feather, 0.0, 1.0)?;
                    bounded("object brush size", *brush_size, 0.0, 16.0)?;
                    bounded("object edge refine", *edge_refine, 0.0, 1.0)?;
                    if strokes.len() > MAX_OBJECT_STROKES {
                        return invalid("object mask contains too many strokes");
                    }
                    let mut point_count = 0usize;
                    for stroke in strokes {
                        point_count =
                            point_count
                                .checked_add(stroke.points.len())
                                .ok_or_else(|| {
                                    SidecarError::Invalid("object prompt count overflow".to_owned())
                                })?;
                        if point_count > MAX_OBJECT_STROKE_POINTS {
                            return invalid("object mask contains too many prompt points");
                        }
                        for point in &stroke.points {
                            finite("object prompt", point)?;
                            bounded("object prompt x", point[0], -16.0, 16.0)?;
                            bounded("object prompt y", point[1], -16.0, 16.0)?;
                        }
                    }
                    if let Some(image) = mask {
                        validate_image(image.width, image.height, image.pixels.len(), 1)?;
                    }
                }
                MaskGeometry::LuminanceRange {
                    source,
                    low,
                    high,
                    grow,
                    feather,
                    high_feather,
                } => {
                    finite("luminance range mask", &[*low, *high, *grow, *feather])?;
                    if let Some(high_feather) = high_feather {
                        finite("luminance range mask", &[*high_feather])?;
                        bounded("luminance high feather", *high_feather, 0.0, 16.0)?;
                    }
                    bounded("luminance low", *low, -16.0, 16.0)?;
                    bounded("luminance high", *high, -16.0, 16.0)?;
                    bounded("luminance grow", *grow, -1.0, 1.0)?;
                    bounded("luminance feather", *feather, 0.0, 16.0)?;
                    if let Some(image) = source {
                        validate_image(image.width, image.height, image.rgba.len(), 4)?;
                    }
                }
                MaskGeometry::ColorRange {
                    source,
                    sample,
                    tolerance,
                    grow,
                    feather,
                    ..
                } => {
                    finite(
                        "color range mask",
                        &[sample[0], sample[1], sample[2], *tolerance, *grow, *feather],
                    )?;
                    for value in sample {
                        bounded("color sample", *value, -16.0, 16.0)?;
                    }
                    bounded("color tolerance", *tolerance, 0.0, 16.0)?;
                    bounded("color grow", *grow, -1.0, 1.0)?;
                    bounded("color feather", *feather, 0.0, 16.0)?;
                    if let Some(image) = source {
                        validate_image(image.width, image.height, image.rgba.len(), 4)?;
                    }
                }
                MaskGeometry::DepthRange { depth, range } => {
                    finite(
                        "depth range mask",
                        &[range.near, range.far, range.near_feather, range.far_feather],
                    )?;
                    bounded("depth near", range.near, 0.0, 1.0)?;
                    bounded("depth far", range.far, 0.0, 1.0)?;
                    bounded("depth near feather", range.near_feather, 0.0, 1.0)?;
                    bounded("depth far feather", range.far_feather, 0.0, 1.0)?;
                    if range.near > range.far {
                        return invalid("depth near must not exceed depth far");
                    }
                    if let Some(image) = depth {
                        validate_image(image.width, image.height, image.pixels.len(), 1)?;
                    }
                }
                _ => {}
            }
        }
    }
    if stack.selected_mask.is_none() && stack.selected_component.is_some() {
        return invalid("a component is selected without a selected mask");
    }
    Ok(())
}

fn geometry_matches_kind(kind: MaskKind, geometry: &MaskGeometry) -> bool {
    matches!(
        (kind, geometry),
        (MaskKind::Fullscreen, MaskGeometry::Fullscreen)
            | (MaskKind::Brush, MaskGeometry::Brush { .. })
            | (MaskKind::Radial, MaskGeometry::Radial { .. })
            | (MaskKind::Linear, MaskGeometry::Linear { .. })
            | (MaskKind::Path, MaskGeometry::Path { .. })
            | (
                MaskKind::Subject | MaskKind::Background | MaskKind::Sky,
                MaskGeometry::Ai { .. }
            )
            | (MaskKind::Object, MaskGeometry::Object { .. })
            | (
                MaskKind::LuminanceRange,
                MaskGeometry::LuminanceRange { .. }
            )
            | (MaskKind::ColorRange, MaskGeometry::ColorRange { .. })
            | (MaskKind::DepthRange, MaskGeometry::DepthRange { .. })
            | (MaskKind::DepthRange, MaskGeometry::Placeholder)
    )
}

fn validate_exposure(exposure: &ExposureParams) -> Result<(), SidecarError> {
    finite(
        "global adjustment",
        &[
            exposure.black_point,
            exposure.exposure,
            exposure.contrast,
            exposure.temperature,
            exposure.tint,
            exposure.hue,
            exposure.saturation,
            exposure.vibrance,
            exposure.chroma_denoise,
            exposure.luminance_denoise,
            exposure.denoise_detail,
            exposure.dual_threshold,
            exposure.frequency_chroma,
            exposure.ca_red,
            exposure.ca_blue,
            exposure.highlight_clip,
            exposure.highlight_reconstruction,
            exposure.highlights,
            exposure.shadows,
            exposure.whites,
            exposure.blacks,
            exposure.texture,
            exposure.clarity,
            exposure.dehaze,
            exposure.sharpen_amount,
            exposure.sharpen_radius,
            exposure.sharpen_detail,
            exposure.sharpen_masking,
            exposure.sigmoid.contrast,
            exposure.sigmoid.skew,
            exposure.sigmoid.display_white_target,
            exposure.sigmoid.display_black_target,
            exposure.sigmoid.hue_preservation,
        ],
    )?;
    finite("global HSL hue", &exposure.hsl_hue)?;
    finite("global HSL saturation", &exposure.hsl_saturation)?;
    finite("global HSL luminance", &exposure.hsl_luminance)?;
    validate_point_colors(&exposure.point_colors)?;
    validate_curves(
        &[
            &exposure.tone_curve,
            &exposure.tone_curve_red,
            &exposure.tone_curve_green,
            &exposure.tone_curve_blue,
        ],
        "global tone curve",
    )?;
    validate_grading(&exposure.color_grading, "global color grading")
}

fn validate_point_colors(colors: &crate::pipeline::PointColors) -> Result<(), SidecarError> {
    if colors.len() > crate::pipeline::MAX_POINT_COLORS {
        return invalid("sidecar contains too many point colors");
    }
    for point in colors.iter() {
        finite(
            "point color",
            &[
                point.sample_hsl[0],
                point.sample_hsl[1],
                point.sample_hsl[2],
                point.hue_shift,
                point.saturation_shift,
                point.luminance_shift,
                point.range,
                point.hue_range.min,
                point.hue_range.inner_min,
                point.hue_range.inner_max,
                point.hue_range.max,
                point.saturation_range.min,
                point.saturation_range.inner_min,
                point.saturation_range.inner_max,
                point.saturation_range.max,
                point.luminance_range.min,
                point.luminance_range.inner_min,
                point.luminance_range.inner_max,
                point.luminance_range.max,
            ],
        )?;
        for value in point.sample_hsl {
            bounded("point color sample", value, 0.0, 1.0)?;
        }
        bounded("point color hue shift", point.hue_shift, -100.0, 100.0)?;
        bounded(
            "point color saturation shift",
            point.saturation_shift,
            -100.0,
            100.0,
        )?;
        bounded(
            "point color luminance shift",
            point.luminance_shift,
            -100.0,
            100.0,
        )?;
        bounded("point color range", point.range, 0.0, 100.0)?;
        validate_point_color_range(point.hue_range, 0.5, "point color hue range")?;
        validate_point_color_range(point.saturation_range, 1.0, "point color saturation range")?;
        validate_point_color_range(point.luminance_range, 1.0, "point color luminance range")?;
    }
    Ok(())
}

fn validate_point_color_range(
    range: crate::pipeline::PointColorRange,
    limit: f32,
    label: &str,
) -> Result<(), SidecarError> {
    for value in [range.min, range.inner_min, range.inner_max, range.max] {
        bounded(label, value, -limit, limit)?;
    }
    if range.min > range.inner_min
        || range.inner_min > range.inner_max
        || range.inner_max > range.max
    {
        return invalid("point color range bounds are out of order");
    }
    Ok(())
}

fn validate_local_adjustments(
    adjustments: &crate::pipeline::LocalAdjustments,
) -> Result<(), SidecarError> {
    finite(
        "local adjustment",
        &[
            adjustments.exposure,
            adjustments.contrast,
            adjustments.highlights,
            adjustments.shadows,
            adjustments.whites,
            adjustments.blacks,
            adjustments.temperature,
            adjustments.tint,
            adjustments.hue,
            adjustments.saturation,
            adjustments.texture,
            adjustments.clarity,
            adjustments.dehaze,
        ],
    )?;
    finite("local HSL hue", &adjustments.hsl_hue)?;
    finite("local HSL saturation", &adjustments.hsl_saturation)?;
    finite("local HSL luminance", &adjustments.hsl_luminance)?;
    validate_point_colors(&adjustments.point_colors)?;
    validate_curves(
        &[
            &adjustments.tone_curve,
            &adjustments.tone_curve_red,
            &adjustments.tone_curve_green,
            &adjustments.tone_curve_blue,
        ],
        "local tone curve",
    )?;
    validate_grading(&adjustments.color_grading, "local color grading")
}

fn validate_neon_effect(neon: &crate::pipeline::NeonEffectSettings) -> Result<(), SidecarError> {
    use crate::pipeline::effect_params::neon;
    validate_effect_params(
        crate::pipeline::MaskEffect::Neon,
        &[
            (neon::AMOUNT, neon.amount),
            (neon::EDGE_WIDTH, neon.edge_width),
            (neon::DETAIL, neon.detail),
            (neon::GLOW, neon.glow),
            (neon::BACKGROUND, neon.background),
        ],
        &neon.color,
    )?;
    validate_effect_color(crate::pipeline::MaskEffect::Neon, neon::COLOR, neon.color)
}

fn validate_effect_components(
    components: &[crate::pipeline::EffectComponent],
) -> Result<(), SidecarError> {
    if components.len() > crate::pipeline::MAX_EFFECT_COMPONENTS {
        return invalid("too many effect components");
    }
    for component in components {
        if component.effect == crate::pipeline::MaskEffect::Adjustment {
            return invalid("Adjustment is not an effect component");
        }
        validate_effect_settings(&component.settings)?;
    }
    Ok(())
}

fn validate_grading(
    grading: &crate::pipeline::ColorGrading,
    label: &str,
) -> Result<(), SidecarError> {
    finite(
        label,
        &[
            grading.shadows.hue,
            grading.shadows.saturation,
            grading.shadows.luminance,
            grading.midtones.hue,
            grading.midtones.saturation,
            grading.midtones.luminance,
            grading.highlights.hue,
            grading.highlights.saturation,
            grading.highlights.luminance,
            grading.global.hue,
            grading.global.saturation,
            grading.global.luminance,
            grading.blending,
            grading.balance,
        ],
    )
}

fn finite(label: &str, values: &[f32]) -> Result<(), SidecarError> {
    if values.iter().all(|value| value.is_finite()) {
        Ok(())
    } else {
        invalid(&format!("{label} contains a non-finite value"))
    }
}

fn bounded(label: &str, value: f32, minimum: f32, maximum: f32) -> Result<(), SidecarError> {
    if (minimum..=maximum).contains(&value) {
        Ok(())
    } else {
        invalid(&format!("{label} is outside the safe range"))
    }
}

pub(super) fn validate_image(
    width: u32,
    height: u32,
    bytes: usize,
    channels: usize,
) -> Result<(), SidecarError> {
    if width == 0 || height == 0 || width > MAX_MASK_IMAGE_EDGE || height > MAX_MASK_IMAGE_EDGE {
        return invalid("mask image dimensions are invalid");
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(channels))
        .ok_or_else(|| SidecarError::Invalid("mask image dimensions overflow".to_owned()))?;
    if bytes != expected {
        return invalid("mask image byte count does not match its dimensions");
    }
    Ok(())
}

pub(super) fn invalid<T>(message: &str) -> Result<T, SidecarError> {
    Err(SidecarError::Invalid(message.to_owned()))
}
