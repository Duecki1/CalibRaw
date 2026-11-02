//! The WGSL the effect registry determines (calibraw-core
//! `pipeline/masks/effects`): each effect's shader ID, its lanes, the per-slot
//! switches and the creative pass's dispatch. It is appended to the creative
//! effects module, so the effects read these names as if declared beside them:
//!
//! - `MASK_EFFECT_<NAME>_ID` for every effect;
//! - `<NAME>_<LANE>_LANE` for every lane an effect packs, and `<OPTION>_OPTION`;
//! - `is_creative_slot_effect(id)` and `apply_creative_slot_effect(id, pos, rgb,
//!   params)`, which calls `apply_<name>(pos, rgb, params)` of each effect that
//!   runs in [`EffectPass::Creative`].
//!
//! `<NAME>` is [`MaskEffect::shader_name`] in upper case.

use crate::pipeline::{effect_lanes::options, EffectPass, MaskEffect};

/// The registry's WGSL.
pub(super) fn registry_wgsl() -> String {
    let effects = || {
        MaskEffect::ALL
            .into_iter()
            .filter(|effect| *effect != MaskEffect::Adjustment)
    };
    let mut wgsl = String::from("// Generated from the effect registry (effect_wgsl.rs).\n");
    for effect in effects() {
        let name = effect.shader_name().to_uppercase();
        wgsl.push_str(&format!(
            "const MASK_EFFECT_{name}_ID: u32 = {}u;\n",
            effect.shader_id()
        ));
        for lane in effect.lanes() {
            wgsl.push_str(&format!(
                "const {name}_{}_LANE: u32 = {}u;\n",
                lane.name, lane.index
            ));
        }
    }
    for (name, index) in options::ALL {
        wgsl.push_str(&format!("const {name}_OPTION: u32 = {index}u;\n"));
    }

    let creative: Vec<MaskEffect> = effects()
        .filter(|effect| effect.runs_in(EffectPass::Creative))
        .collect();
    let ids = creative
        .iter()
        .map(|effect| format!("{}u", effect.shader_id()))
        .collect::<Vec<_>>()
        .join(", ");
    wgsl.push_str("fn is_creative_slot_effect(effect_id: u32) -> bool {\n    switch effect_id {\n");
    if !creative.is_empty() {
        wgsl.push_str(&format!("        case {ids}: {{ return true; }}\n"));
    }
    wgsl.push_str("        default: { return false; }\n    }\n}\n");
    wgsl.push_str(
        "fn apply_creative_slot_effect(\n    effect_id: u32,\n    pos: vec2<i32>,\n    \
         rgb: vec3<f32>,\n    params: MaskEffectParams,\n) -> vec3<f32> {\n    switch effect_id {\n",
    );
    for effect in &creative {
        wgsl.push_str(&format!(
            "        case {}u: {{ return apply_{}(pos, rgb, params); }}\n",
            effect.shader_id(),
            effect.shader_name()
        ));
    }
    wgsl.push_str("        default: { return rgb; }\n    }\n}\n");
    wgsl
}
