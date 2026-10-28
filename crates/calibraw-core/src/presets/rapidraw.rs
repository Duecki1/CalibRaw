//! Importing RapidRAW `.rrpreset` JSON files.

use super::{PresetError, DEFAULT_PRESET_GROUP, MAX_PRESET_BYTES};
use crate::pipeline::{
    AdjustmentGroup, ColorGradeWheel, EffectComponent, MaskEffect, PointCurve, WhiteBalance,
    MAX_POINT_CURVE_POINTS,
};
use crate::sidecar::{default_edit_state, EditSelection, EditState};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct RapidRawPreset {
    pub name: String,
    pub group: String,
    pub selection: EditSelection,
    pub edits: EditState,
    pub warnings: Vec<String>,
}

pub fn read_rrpreset_file(path: &Path) -> Result<Vec<RapidRawPreset>, PresetError> {
    import_rrpreset(&super::read_preset_bytes(path)?)
}

/// Reads RapidRAW's single-preset and exported multi-preset container layouts.
pub fn import_rrpreset(bytes: &[u8]) -> Result<Vec<RapidRawPreset>, PresetError> {
    if bytes.len() as u64 > MAX_PRESET_BYTES {
        return Err(PresetError::TooLarge(bytes.len() as u64));
    }
    let root: Value = serde_json::from_slice(bytes)
        .map_err(|error| PresetError::Invalid(format!("invalid RapidRAW preset JSON: {error}")))?;
    let candidates = if let Some(presets) = root.get("presets").and_then(Value::as_array) {
        presets.iter().collect::<Vec<_>>()
    } else {
        vec![&root]
    };
    if candidates.is_empty() {
        return Err(PresetError::Invalid(
            "RapidRAW preset contains no presets.".to_owned(),
        ));
    }

    candidates
        .into_iter()
        .map(|candidate| {
            let preset = candidate.get("preset").unwrap_or(candidate);
            let name = preset
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .ok_or_else(|| PresetError::Invalid("RapidRAW preset has no name.".to_owned()))?
                .to_owned();
            let group = ["folder", "folderName", "folderPath"]
                .into_iter()
                .find_map(|key| candidate.get(key).and_then(Value::as_str))
                .filter(|group| !group.trim().is_empty())
                .unwrap_or(DEFAULT_PRESET_GROUP)
                .to_owned();
            let adjustments = preset
                .get("adjustments")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    PresetError::Invalid(format!(
                        "RapidRAW preset “{name}” has no adjustments object."
                    ))
                })?;
            let mut edits = default_edit_state();
            let mut selection = EditSelection::default();
            let mut warnings = Vec::new();
            let exposure = &mut edits.exposure;

            let light = [
                "exposure",
                "contrast",
                "highlights",
                "shadows",
                "whites",
                "blacks",
            ];
            for key in light {
                if let Some(value) = adjustments.get(key).and_then(number) {
                    match key {
                        "exposure" => exposure.exposure = value,
                        "contrast" => exposure.contrast = value,
                        "highlights" => exposure.highlights = value,
                        "shadows" => exposure.shadows = value,
                        "whites" => exposure.whites = value,
                        "blacks" => exposure.blacks = value,
                        _ => {}
                    }
                    selection
                        .adjustment_groups
                        .set(AdjustmentGroup::Light, true);
                }
            }
            for (key, dest) in [
                ("saturation", &mut exposure.saturation),
                ("vibrance", &mut exposure.vibrance),
            ] {
                if let Some(value) = adjustments.get(key).and_then(number) {
                    *dest = value;
                    selection
                        .adjustment_groups
                        .set(AdjustmentGroup::Color, true);
                }
            }
            if adjustments.contains_key("temperature") || adjustments.contains_key("tint") {
                exposure.white_balance = WhiteBalance::LegacyOffset {
                    temperature: adjustments
                        .get("temperature")
                        .and_then(number)
                        .unwrap_or_default()
                        * 1.5,
                    tint: -adjustments.get("tint").and_then(number).unwrap_or_default() / 5.0,
                };
                selection
                    .adjustment_groups
                    .set(AdjustmentGroup::Color, true);
            }
            let grain_keys = ["grainAmount", "grainSize", "grainRoughness"];
            if grain_keys.iter().any(|key| adjustments.contains_key(*key)) {
                let masks = Arc::make_mut(&mut edits.masks);
                let mut component = EffectComponent::new(MaskEffect::Grain);
                if let Some(value) = adjustments.get("grainAmount").and_then(number) {
                    component.settings.grain.amount = value;
                }
                if let Some(value) = adjustments.get("grainSize").and_then(number) {
                    component.settings.grain.size = super::grain_size_from_import(value);
                }
                if let Some(value) = adjustments.get("grainRoughness").and_then(number) {
                    component.settings.grain.roughness = value;
                }
                masks.global_effects.push(component);
                selection
                    .adjustment_groups
                    .set(AdjustmentGroup::Effects, true);
            }
            let vignette_keys = [
                "vignetteAmount",
                "vignetteMidpoint",
                "vignetteRoundness",
                "vignetteFeather",
            ];
            if vignette_keys
                .iter()
                .any(|key| adjustments.contains_key(*key))
            {
                let masks = Arc::make_mut(&mut edits.masks);
                let mut component = EffectComponent::new(MaskEffect::Vignette);
                if let Some(value) = adjustments.get("vignetteAmount").and_then(number) {
                    component.settings.vignette.amount = value;
                }
                if let Some(value) = adjustments.get("vignetteMidpoint").and_then(number) {
                    component.settings.vignette.midpoint = value;
                }
                if let Some(value) = adjustments.get("vignetteRoundness").and_then(number) {
                    component.settings.vignette.roundness = value;
                }
                if let Some(value) = adjustments.get("vignetteFeather").and_then(number) {
                    component.settings.vignette.feather = value;
                }
                masks.global_effects.push(component);
                selection
                    .adjustment_groups
                    .set(AdjustmentGroup::Effects, true);
            }
            for (key, dest) in [
                ("clarity", &mut exposure.clarity),
                ("dehaze", &mut exposure.dehaze),
                ("structure", &mut exposure.texture),
            ] {
                if let Some(value) = adjustments.get(key).and_then(number) {
                    *dest = value;
                    selection
                        .adjustment_groups
                        .set(AdjustmentGroup::Effects, true);
                }
            }
            for (key, dest) in [
                ("sharpness", &mut exposure.sharpen_amount),
                ("lumaNoiseReduction", &mut exposure.luminance_denoise),
                ("colorNoiseReduction", &mut exposure.chroma_denoise),
            ] {
                if let Some(value) = adjustments.get(key).and_then(number) {
                    *dest = value;
                    selection
                        .adjustment_groups
                        .set(AdjustmentGroup::Detail, true);
                }
            }
            if let Some(curves) = adjustments.get("pointCurves") {
                for (key, destination) in [
                    ("luma", &mut exposure.tone_curve),
                    ("red", &mut exposure.tone_curve_red),
                    ("green", &mut exposure.tone_curve_green),
                    ("blue", &mut exposure.tone_curve_blue),
                ] {
                    if let Some(curve) = read_point_curve(curves.get(key)) {
                        *destination = curve;
                        selection
                            .adjustment_groups
                            .set(AdjustmentGroup::ToneCurve, true);
                    }
                }
            }
            if let Some(grading) = adjustments.get("colorGrading") {
                if let Some(value) = grading.get("blending").and_then(number) {
                    exposure.color_grading.blending = value;
                }
                if let Some(value) = grading.get("balance").and_then(number) {
                    exposure.color_grading.balance = value;
                }
                for (key, wheel) in [
                    ("shadows", &mut exposure.color_grading.shadows),
                    ("midtones", &mut exposure.color_grading.midtones),
                    ("highlights", &mut exposure.color_grading.highlights),
                    ("global", &mut exposure.color_grading.global),
                ] {
                    set_wheel(grading.get(key), wheel);
                }
                selection
                    .adjustment_groups
                    .set(AdjustmentGroup::ColorGrading, true);
            }
            if let Some(hsl) = adjustments.get("hsl") {
                let colors = [
                    "reds", "oranges", "yellows", "greens", "aquas", "blues", "purples", "magentas",
                ];
                for (index, color) in colors.into_iter().enumerate() {
                    let Some(channel) = hsl.get(color) else {
                        continue;
                    };
                    if let Some(value) = channel.get("hue").and_then(number) {
                        exposure.hsl_hue[index] = value;
                    }
                    if let Some(value) = channel.get("saturation").and_then(number) {
                        exposure.hsl_saturation[index] = value;
                    }
                    if let Some(value) = channel.get("luminance").and_then(number) {
                        exposure.hsl_luminance[index] = value;
                    }
                }
                selection
                    .adjustment_groups
                    .set(AdjustmentGroup::ColorMixer, true);
            }

            let known = [
                "exposure",
                "contrast",
                "highlights",
                "shadows",
                "whites",
                "blacks",
                "saturation",
                "vibrance",
                "clarity",
                "dehaze",
                "structure",
                "sharpness",
                "lumaNoiseReduction",
                "colorNoiseReduction",
                "pointCurves",
                "grainAmount",
                "grainSize",
                "grainRoughness",
                "vignetteAmount",
                "vignetteMidpoint",
                "vignetteRoundness",
                "vignetteFeather",
                "temperature",
                "tint",
                "colorGrading",
                "hsl",
            ];
            let unsupported = adjustments
                .keys()
                .filter(|key| !known.contains(&key.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            if !unsupported.is_empty() {
                warnings.push(format!(
                    "Unsupported RapidRAW settings: {}.",
                    unsupported.join(", ")
                ));
            }
            Ok(RapidRawPreset {
                name,
                group,
                selection,
                edits,
                warnings,
            })
        })
        .collect()
}

fn number(value: &Value) -> Option<f32> {
    value
        .as_f64()
        .map(|value| value as f32)
        .filter(|value| value.is_finite())
}

fn set_wheel(value: Option<&Value>, wheel: &mut ColorGradeWheel) {
    let Some(value) = value else { return };
    if let Some(number) = value.get("hue").and_then(number) {
        wheel.hue = number;
    }
    if let Some(number) = value.get("saturation").and_then(number) {
        wheel.saturation = number;
    }
    if let Some(number) = value.get("luminance").and_then(number) {
        wheel.luminance = number;
    }
}

fn read_point_curve(value: Option<&Value>) -> Option<PointCurve> {
    let values = value?.as_array()?;
    if values.len() < 2 || values.len() > MAX_POINT_CURVE_POINTS {
        return None;
    }
    let mut curve = PointCurve::linear();
    for (index, point) in values.iter().enumerate() {
        let x = number(point.get("x")?)? / 255.0;
        let y = number(point.get("y")?)? / 255.0;
        curve.points[index] = [x, y];
    }
    curve.len = values.len() as u32;
    curve.sanitize();
    Some(curve)
}
