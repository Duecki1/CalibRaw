//! Repairs grain sizes written by the early RapidRAW preset importer.
//!
//! migration: remove in v2.0.0

use crate::pipeline::MaskEffect;
use crate::sidecar::EditState;
use std::sync::Arc;

/// Early RapidRAW imports copied its 0–100 Grain Size slider directly into
/// CalibRaw's 0.5–4 parameter. Repair those values before normal validation.
pub(crate) fn migrate(edits: &mut EditState) -> bool {
    let masks = Arc::make_mut(&mut edits.masks);
    let mut changed = false;
    for component in masks.global_effects.iter_mut().chain(
        masks
            .masks
            .iter_mut()
            .flat_map(|mask| &mut mask.effect_components),
    ) {
        if component.effect == MaskEffect::Grain {
            let size = component.settings.grain.size;
            if !(0.5..=4.0).contains(&size) && (0.0..=100.0).contains(&size) {
                component.settings.grain.size = crate::presets::grain_size_from_import(size);
                changed = true;
            }
        }
    }
    changed
}
