//! Local adjustments of XMP presets: corrections, the masks that shape them
//! and their sliders, translated into CalibRaw masks.
//!
//! A correction becomes one mask. Its components translate as follows:
//!
//! - Subject, background and sky become AI components without a raster;
//!   CalibRaw regenerates them on the target photo.
//! - Radial gradients keep their ellipse, given as an unrotated bounding box
//!   in 0–1 image coordinates that may reach past the frame, rotated by
//!   `Angle` degrees in the image's y-down frame. Their feather fades from
//!   the full effect inside to none at the ellipse, as CalibRaw's does.
//! - Linear gradients run from full effect at `Full` to none at `Zero`.
//! - Luminance ranges keep their four-point lightness range. Older files
//!   attach one range to the whole correction; it narrows every component.
//!
//! A correction with any other component (brushes, people, objects, colour
//! and depth ranges) is skipped with a warning rather than imported with a
//! different shape.

use super::curves::{point_curve, read_points};
use super::document::{parse_number, Element, Properties, Warnings};
use super::point_colors::read_point_colors;
use super::sanitize_label;
use crate::pipeline::{
    lightness_to_luminance, LocalAdjustments, LocalMask, MaskCombineMode, MaskComponent,
    MaskGeometry, MaskKind, HUE_ROTATION_LIMIT_DEGREES, LUMINANCE_FEATHER_MAX,
    LUMINANCE_FEATHER_WIDTH, MAX_LOCAL_MASKS, MAX_MASK_COMPONENTS,
};
use crate::presets::MAX_PRESET_NAME_CHARS;

const LOCAL_TONE_CURVES: [(&str, &str); 4] = [
    ("crs:MainCurve", "RGB"),
    ("crs:RedCurve", "red"),
    ("crs:GreenCurve", "green"),
    ("crs:BlueCurve", "blue"),
];
/// Local adjustments without an equivalent, with their neutral values.
const UNSUPPORTED_LOCAL_SETTINGS: &[(&str, f32)] = &[
    ("LocalSharpness", 0.0),
    ("LocalLuminanceNoise", 0.0),
    ("LocalMoire", 0.0),
    ("LocalDefringe", 0.0),
    ("LocalGrain", 0.0),
    ("LocalBrightness", 0.0),
    ("LocalToningHue", 0.0),
    ("LocalToningSaturation", 0.0),
    ("LocalCorrectedDepth", 0.0),
    ("LocalCurveRefineSaturation", 100.0),
];
/// Lightness softness kept above a luminance range that is full up to
/// white, so pixels at white stay selected.
const OPEN_RANGE_FEATHER: f32 = 0.01;

/// The masks of `corrections`, at most [`MAX_LOCAL_MASKS`].
pub(super) fn convert_masks(corrections: &[&Element], warnings: &mut Warnings) -> Vec<LocalMask> {
    let mut masks = Vec::new();
    for (index, correction) in corrections.iter().enumerate() {
        if masks.len() >= MAX_LOCAL_MASKS {
            warnings.push(format!(
                "Only the first {MAX_LOCAL_MASKS} masks were imported."
            ));
            break;
        }
        if let Some(mask) = convert_correction(correction, index, warnings) {
            masks.push(mask);
        }
    }
    masks
}

fn convert_correction(
    correction: &Element,
    index: usize,
    warnings: &mut Warnings,
) -> Option<LocalMask> {
    let properties = Properties::of(correction);
    let fallback_name = format!("Mask {}", index + 1);
    let (name, shortened) = sanitize_label(
        properties.text("CorrectionName").unwrap_or(&fallback_name),
        &fallback_name,
        MAX_PRESET_NAME_CHARS,
    );
    if shortened {
        warnings.push(format!(
            "A mask name was shortened to {MAX_PRESET_NAME_CHARS} characters."
        ));
    }

    let items = correction
        .child("crs:CorrectionMasks")
        .into_iter()
        .flat_map(|masks| masks.seq_items())
        .map(|item| component(item.resource()));
    let shared_range = correction
        .child("crs:CorrectionRangeMask")
        .and_then(|range| correction_range(range.resource()).transpose());
    let mut components = Vec::new();
    let mut unsupported = Vec::new();
    for item in items.chain(shared_range) {
        match item {
            Ok(_) if components.len() >= MAX_MASK_COMPONENTS => {
                warnings.push(format!(
                    "The mask “{name}” has more than {MAX_MASK_COMPONENTS} components; the extra components were not imported."
                ));
                break;
            }
            Ok(component) => components.push(component),
            Err(label) => unsupported.push(label),
        }
    }

    if !unsupported.is_empty() {
        warnings.push(format!(
            "The mask “{name}” was not imported because it uses unsupported components: {}.",
            unsupported.join(", ")
        ));
        return None;
    }
    let Some(first) = components.first() else {
        warnings.push(format!(
            "The mask “{name}” has no components and was not imported."
        ));
        return None;
    };
    if first.combine != MaskCombineMode::Add {
        warnings.push(format!(
            "The mask “{name}” starts with a subtracted component and was not imported."
        ));
        return None;
    }

    let mut mask = LocalMask::new(first.kind, index + 1);
    mask.adjustments = local_adjustments(correction, &properties, &name, warnings);
    mask.common.name = name;
    mask.common.enabled = properties.boolean("CorrectionActive").unwrap_or(true);
    mask.opacity = properties
        .value("CorrectionAmount")
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);
    mask.components = components;
    Some(mask)
}

/// One mask component, or a label saying why it cannot be imported.
fn component(element: &Element) -> Result<MaskComponent, String> {
    let attributes = Properties::of(element);
    let unsupported = |kind: &str| format!("“{}” ({kind})", component_name(&attributes));
    let shape = match attributes.text("What") {
        Some("Mask/CircularGradient") => radial(&attributes)
            .map(|geometry| (MaskKind::Radial, geometry))
            .ok_or("incomplete radial gradient"),
        Some("Mask/LinearGradient" | "Mask/Gradient") => linear(&attributes)
            .map(|geometry| (MaskKind::Linear, geometry))
            .ok_or("incomplete linear gradient"),
        Some("Mask/RangeMask") => element
            .child("crs:CorrectionRangeMask")
            .ok_or("range")
            .and_then(|range| range_geometry(&Properties::of(range.resource())))
            .map(|geometry| (MaskKind::LuminanceRange, geometry)),
        _ => ai_kind(&attributes)
            .map(|kind| (kind, MaskGeometry::for_kind(kind)))
            .ok_or_else(|| mask_kind_label(&attributes)),
    };
    let (kind, geometry) = shape.map_err(unsupported)?;
    let combine = mask_combine(&attributes).ok_or_else(|| unsupported("unknown blend mode"))?;
    let mut component = MaskComponent::new(kind, combine);
    component.geometry = geometry;
    if let Some(name) = attributes.text("MaskName") {
        component.name = sanitize_label(name, kind.label(), MAX_PRESET_NAME_CHARS).0;
    }
    component.enabled = attributes.boolean("MaskActive").unwrap_or(true);
    // `Flipped` is the older radial filter's own inversion.
    component.invert = attributes.boolean("MaskInverted").unwrap_or(false)
        ^ attributes.boolean("Flipped").unwrap_or(false);
    Ok(component)
}

/// The range an older correction attaches to all its components, as an
/// intersected component. `Ok(None)` when the correction has no range.
fn correction_range(range: &Element) -> Result<Option<MaskComponent>, String> {
    let attributes = Properties::of(range);
    if attributes.value("Type").and_then(integer) == Some(0) {
        return Ok(None);
    }
    let geometry = range_geometry(&attributes).map_err(|kind| format!("shared {kind}"))?;
    let mut component = MaskComponent::new(MaskKind::LuminanceRange, MaskCombineMode::Intersect);
    component.geometry = geometry;
    component.invert = attributes.boolean("Invert").unwrap_or(false);
    Ok(Some(component))
}

fn radial(attributes: &Properties<'_>) -> Option<MaskGeometry> {
    let [top, left, bottom, right] =
        ["Top", "Left", "Bottom", "Right"].map(|key| attributes.value(key));
    let (top, left, bottom, right) = (top?, left?, bottom?, right?);
    Some(MaskGeometry::Radial {
        center: [(left + right) * 0.5, (top + bottom) * 0.5],
        radius: [(right - left).abs() * 0.5, (bottom - top).abs() * 0.5],
        rotation: attributes.value("Angle").unwrap_or(0.0).to_radians(),
        feather: (attributes.value("Feather").unwrap_or(50.0) / 100.0).clamp(0.0, 1.0),
        initialized: true,
    })
}

fn linear(attributes: &Properties<'_>) -> Option<MaskGeometry> {
    let point = |x: &str, y: &str| Some([attributes.value(x)?, attributes.value(y)?]);
    // With a feather of 1, CalibRaw's gradient is full at `start` and has
    // faded out at `end`.
    Some(MaskGeometry::Linear {
        start: point("FullX", "FullY")?,
        end: point("ZeroX", "ZeroY")?,
        feather: 1.0,
        initialized: true,
    })
}

/// The geometry of a `crs:CorrectionRangeMask`, or the name of its
/// unsupported kind. Only luminance ranges (type 2) are supported.
fn range_geometry(range: &Properties<'_>) -> Result<MaskGeometry, &'static str> {
    match range.value("Type").and_then(integer) {
        Some(2) => luminance_range(range).ok_or("luminance range"),
        Some(1) => Err("color range"),
        Some(3) => Err("depth range"),
        _ => Err("range"),
    }
}

/// A luminance range from its `LumRange`: the lightness (0–1) where the
/// selection starts, becomes full, stops being full and ends. CalibRaw places
/// and softens its bounds on the same lightness scale.
fn luminance_range(range: &Properties<'_>) -> Option<MaskGeometry> {
    let values = range
        .text("LumRange")?
        .split_whitespace()
        .map(parse_number)
        .collect::<Option<Vec<f32>>>()?;
    let [starts, full_from, full_to, ends]: [f32; 4] = values.try_into().ok()?;
    let [starts, full_from, full_to, ends] =
        [starts, full_from, full_to, ends].map(|value| value.clamp(0.0, 1.0));
    if !(starts <= full_from && full_from <= full_to && full_to <= ends) {
        return None;
    }
    let above = if full_to >= 1.0 {
        OPEN_RANGE_FEATHER
    } else {
        ends - full_to
    };
    let feather = |width: f32| (width / LUMINANCE_FEATHER_WIDTH).clamp(0.0, LUMINANCE_FEATHER_MAX);
    Some(MaskGeometry::LuminanceRange {
        source: None,
        low: lightness_to_luminance(full_from),
        high: lightness_to_luminance(full_to),
        grow: 0.0,
        feather: feather(full_from - starts),
        high_feather: Some(feather(above)),
    })
}

fn local_adjustments(
    correction: &Element,
    properties: &Properties<'_>,
    mask_name: &str,
    warnings: &mut Warnings,
) -> LocalAdjustments {
    let mut adjustments = LocalAdjustments::default();

    // Local sliders are stored as -1..1, unlike the -100..100 global sliders.
    // Exposure is already in EV, and hue scales to CalibRaw's hue rotation.
    if let Some(value) = properties.first_number(&["LocalExposure2012", "LocalExposure"], warnings)
    {
        adjustments.exposure = value.clamp(-5.0, 5.0);
    }
    if let Some(value) = properties.number("LocalHue", warnings) {
        adjustments.hue = (value * HUE_ROTATION_LIMIT_DEGREES)
            .clamp(-HUE_ROTATION_LIMIT_DEGREES, HUE_ROTATION_LIMIT_DEGREES);
    }
    for (keys, target) in [
        (
            &["LocalContrast2012", "LocalContrast"][..],
            &mut adjustments.contrast,
        ),
        (
            &["LocalHighlights2012", "LocalHighlights"],
            &mut adjustments.highlights,
        ),
        (
            &["LocalShadows2012", "LocalShadows"],
            &mut adjustments.shadows,
        ),
        (&["LocalWhites2012", "LocalWhites"], &mut adjustments.whites),
        (&["LocalBlacks2012", "LocalBlacks"], &mut adjustments.blacks),
        (&["LocalTemperature"], &mut adjustments.temperature),
        (&["LocalTint"], &mut adjustments.tint),
        (&["LocalSaturation"], &mut adjustments.saturation),
        (&["LocalTexture"], &mut adjustments.texture),
        (
            &["LocalClarity2012", "LocalClarity"],
            &mut adjustments.clarity,
        ),
        (&["LocalDehaze"], &mut adjustments.dehaze),
    ] {
        if let Some(value) = properties.first_number(keys, warnings) {
            *target = (value * 100.0).clamp(-100.0, 100.0);
        }
    }

    let targets = [
        &mut adjustments.tone_curve,
        &mut adjustments.tone_curve_red,
        &mut adjustments.tone_curve_green,
        &mut adjustments.tone_curve_blue,
    ];
    for ((key, label), target) in LOCAL_TONE_CURVES.iter().zip(targets) {
        let Some(element) = correction.child(key) else {
            continue;
        };
        let what = format!("The local {label} tone curve of the mask “{mask_name}”");
        if let Some(points) = read_points(element, &what, warnings) {
            *target = point_curve(&points, warnings);
        }
    }

    if let Some(swatches) = correction.child("crs:LocalPointColors") {
        adjustments.point_colors = read_point_colors(
            swatches,
            correction.child("crs:LocalColorVariance"),
            &format!(" of the mask “{mask_name}”"),
            warnings,
        );
    }

    if UNSUPPORTED_LOCAL_SETTINGS
        .iter()
        .any(|(key, neutral)| properties.differs(key, *neutral))
    {
        warnings.push(format!(
            "Some local adjustments of the mask “{mask_name}” were not imported."
        ));
    }
    adjustments
}

fn ai_kind(attributes: &Properties<'_>) -> Option<MaskKind> {
    match integer(attributes.value("MaskSubType")?)? {
        0 if attributes.value("MaskSubCategoryID") == Some(22.0) => Some(MaskKind::Background),
        1 => Some(MaskKind::Subject),
        2 => Some(MaskKind::Sky),
        _ => None,
    }
}

fn mask_combine(attributes: &Properties<'_>) -> Option<MaskCombineMode> {
    if !attributes.has("MaskBlendMode") {
        return Some(MaskCombineMode::Add);
    }
    match integer(attributes.value("MaskBlendMode")?)? {
        0 => Some(MaskCombineMode::Add),
        1 => Some(MaskCombineMode::Subtract),
        2 => Some(MaskCombineMode::Intersect),
        _ => None,
    }
}

/// `value` as an enumeration index, when it is a whole number.
fn integer(value: f32) -> Option<i32> {
    ((value - value.round()).abs() <= 1e-6).then_some(value.round() as i32)
}

fn component_name(attributes: &Properties<'_>) -> String {
    let fallback = "unnamed mask";
    sanitize_label(
        attributes.text("MaskName").unwrap_or(fallback),
        fallback,
        MAX_PRESET_NAME_CHARS,
    )
    .0
}

fn mask_kind_label(attributes: &Properties<'_>) -> &'static str {
    match attributes
        .value("MaskSubType")
        .map(|value| value.round() as i32)
    {
        Some(0) => "landscape element",
        Some(1) => "subject",
        Some(2) => "sky",
        Some(3) => "person",
        Some(4) => "object",
        Some(_) => "unknown AI mask",
        None => match attributes.text("What") {
            Some("Mask/Aggregate" | "Mask/Paint") => "brush",
            Some("Mask/Path") => "path",
            Some("Mask/Image") => "unsupported AI mask",
            Some(_) | None => "unsupported mask",
        },
    }
}
