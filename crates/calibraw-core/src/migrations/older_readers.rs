//! Writing sidecars and presets that v1.2 and older can still open.
//!
//! migration: remove in v2.0.0
//!
//! Devices share sidecars and may run different versions, so a file saved
//! here stays readable by those versions where it can:
//! - Up to v1.2, readers require the Effects-card slider fields that edits no
//!   longer hold. They are written at rest, so those readers decode the edit
//!   and show the effects that replaced the sliders as effect components,
//!   while [`super::LegacyEffectSliders`] reads them back as nothing to
//!   migrate.
//! - Those readers cannot decode a Relight effect. Edits with one are written
//!   with schema 2, which they report as newer instead of as corrupt, so they
//!   never replace the file with an unedited one. Every other edit is written
//!   with schema 1, whose layout schema 2 shares.

use super::effect_sliders::GlobalSliders;
use crate::pipeline::{ExposureParams, MaskEffect};
use crate::sidecar::EditState;
use serde::{Serialize, Serializer};

/// The sidecar and preset schema that v1.2 and older read.
pub(crate) const V1_SCHEMA_VERSION: u32 = 1;

/// Serializes `exposure` with the Effects-card sliders at rest.
pub(crate) fn serialize_exposure<S: Serializer>(
    exposure: &ExposureParams,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct WithRestingSliders<'a> {
        #[serde(flatten)]
        exposure: &'a ExposureParams,
        #[serde(flatten)]
        sliders: GlobalSliders,
    }
    WithRestingSliders {
        exposure,
        sliders: GlobalSliders::default(),
    }
    .serialize(serializer)
}

/// The schema to write `edits` with: [`V1_SCHEMA_VERSION`] when readers up to
/// v1.2 can decode them, otherwise `current`.
pub(crate) fn schema_version(edits: &EditState, current: u32) -> u32 {
    let masks = &edits.masks;
    let uses_relight = masks
        .global_effects
        .iter()
        .chain(masks.masks.iter().flat_map(|mask| &mask.effect_components))
        .any(|component| component.effect == MaskEffect::Relight)
        || masks
            .masks
            .iter()
            .any(|mask| mask.effect == MaskEffect::Relight);
    if uses_relight {
        current
    } else {
        V1_SCHEMA_VERSION
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{AdjustmentGroup, AdjustmentGroupSet, EffectComponent, MaskKind};
    use crate::presets::{Preset, PRESET_SCHEMA_VERSION};
    use crate::sidecar::{
        decode, default_edit_state, encode, EditSelection, SIDECAR_SCHEMA_VERSION,
    };
    use serde_json::Value;
    use std::sync::Arc;

    /// Edits with a Glow, and a Relight on the photo or in a mask when asked.
    fn edits(relight: Option<bool>) -> EditState {
        let mut edits = default_edit_state();
        let masks = Arc::make_mut(&mut edits.masks);
        masks
            .global_effects
            .push(EffectComponent::new(MaskEffect::Glow));
        masks.add_mask(MaskKind::Radial).unwrap();
        match relight {
            Some(false) => masks
                .global_effects
                .push(EffectComponent::new(MaskEffect::Relight)),
            Some(true) => masks.masks[0]
                .effect_components
                .push(EffectComponent::new(MaskEffect::Relight)),
            None => {}
        }
        edits
    }

    fn document(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).unwrap()
    }

    #[test]
    fn edits_without_relight_stay_readable_by_v1_2() {
        let edits = edits(None);
        let bytes = encode(edits.clone()).unwrap();
        let document = document(&bytes);
        assert_eq!(document["schema_version"], V1_SCHEMA_VERSION);
        // The fields v1.2 requires, at the defaults it gave them.
        let exposure = &document["edits"]["exposure"];
        for (field, value) in [
            ("glow_amount", 0.0),
            ("glow_radius", 50.0),
            ("glow_threshold", 60.0),
            ("vignette_amount", 0.0),
            ("vignette_midpoint", 50.0),
            ("vignette_roundness", 0.0),
            ("vignette_feather", 50.0),
            ("vignette_highlights", 0.0),
        ] {
            assert_eq!(exposure[field], value, "{field}");
        }
        let loaded = decode(&bytes).unwrap();
        assert!(!loaded.migrated);
        assert_eq!(loaded.edits, edits);
    }

    #[test]
    fn relight_anywhere_is_written_as_the_newer_schema() {
        for in_mask in [false, true] {
            let edits = edits(Some(in_mask));
            let bytes = encode(edits.clone()).unwrap();
            assert_eq!(document(&bytes)["schema_version"], SIDECAR_SCHEMA_VERSION);
            let loaded = decode(&bytes).unwrap();
            assert!(!loaded.migrated);
            assert_eq!(loaded.edits, edits, "in_mask={in_mask}");
        }
    }

    #[test]
    fn presets_choose_their_schema_like_sidecars() {
        let selection = EditSelection {
            adjustment_groups: AdjustmentGroupSet::from_iter([AdjustmentGroup::Effects]),
            masks: true,
            ..EditSelection::default()
        };
        for (relight, expected) in [
            (None, V1_SCHEMA_VERSION),
            (Some(false), PRESET_SCHEMA_VERSION),
        ] {
            let preset = Preset::new("Light", "", selection, &edits(relight)).unwrap();
            let bytes = preset.encode().unwrap();
            assert_eq!(document(&bytes)["schema_version"], expected);
            assert_eq!(Preset::decode(&bytes).unwrap(), preset);
        }
    }
}
