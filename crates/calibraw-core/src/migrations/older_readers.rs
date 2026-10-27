//! Writing sidecars and presets that v1.3 and older can still open.
//!
//! migration: remove in v2.0.0
//!
//! Devices share sidecars and may run different versions, so a file saved
//! here stays readable by those versions where it can:
//! - Up to v1.3, readers require the `ca_red` and `ca_blue` fields of the
//!   retired demosaic chromatic-aberration shift. No release could set them,
//!   so they are written as 0 and ignored when read.
//! - Up to v1.3, readers require a `feather` in colour-range masks, which
//!   now soften their edge as part of Tolerance. It is written as those
//!   versions' default and ignored when read.
//! - Up to v1.2, readers require the Effects-card slider fields that edits no
//!   longer hold. They are written at rest, so those readers decode the edit
//!   and show the effects that replaced the sliders as effect components,
//!   while [`super::LegacyEffectSliders`] reads them back as nothing to
//!   migrate.
//! - Those readers cannot decode a Relight effect or a custom crop aspect
//!   ratio and would drop a manual lens correction or chromatic aberration
//!   removal. Edits with any of them are written with schema 2, which they
//!   report as newer instead of as corrupt, so they never replace the file
//!   with an unedited one. Every other edit is written with schema 1, whose
//!   layout schema 2 shares.
//! - Up to v1.3, readers would drop a LUT and cannot decode a Select People
//!   mask. Edits with either are written with schema 3, which shares the
//!   same layout and which those readers report as newer.

use super::effect_sliders::GlobalSliders;
use crate::pipeline::{CropAspectRatio, ExposureParams, MaskEffect, MaskRgbImage};
use crate::sidecar::EditState;
use serde::{Serialize, Serializer};

/// The sidecar and preset schema that v1.2 and older read.
pub(crate) const V1_SCHEMA_VERSION: u32 = 1;
/// The newest sidecar and preset schema that v1.3 reads.
pub(crate) const V2_SCHEMA_VERSION: u32 = 2;

/// Serializes `exposure` with the chromatic-aberration shift and the
/// Effects-card sliders at rest.
pub(crate) fn serialize_exposure<S: Serializer>(
    exposure: &ExposureParams,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct WithRestingFields<'a> {
        #[serde(flatten)]
        exposure: &'a ExposureParams,
        ca_red: f32,
        ca_blue: f32,
        #[serde(flatten)]
        sliders: GlobalSliders,
    }
    WithRestingFields {
        exposure,
        ca_red: 0.0,
        ca_blue: 0.0,
        sliders: GlobalSliders::default(),
    }
    .serialize(serializer)
}

/// Colour-range feather written for readers up to v1.3: their default.
const RESTING_COLOR_FEATHER: f32 = 0.12;

/// Serializes the fields of a colour-range mask with the retired `feather`
/// at rest. The mask source is never stored.
pub(crate) fn serialize_color_range<S: Serializer>(
    _source: &Option<MaskRgbImage>,
    sample: &[f32; 3],
    tolerance: &f32,
    grow: &f32,
    sampled: &bool,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct ColorRange<'a> {
        sample: &'a [f32; 3],
        tolerance: f32,
        grow: f32,
        feather: f32,
        sampled: bool,
    }
    ColorRange {
        sample,
        tolerance: *tolerance,
        grow: *grow,
        feather: RESTING_COLOR_FEATHER,
        sampled: *sampled,
    }
    .serialize(serializer)
}

/// The schema to write `edits` with: [`V1_SCHEMA_VERSION`] when readers up to
/// v1.2 can decode them, [`V2_SCHEMA_VERSION`] when readers up to v1.3 can,
/// otherwise `current`.
pub(crate) fn schema_version(edits: &EditState, current: u32) -> u32 {
    let masks = &edits.masks;
    if edits.look_lut.is_some() || masks.has_people_masks() {
        return current;
    }
    let uses_relight = masks
        .global_effects
        .iter()
        .chain(masks.masks.iter().flat_map(|mask| &mask.effect_components))
        .any(|component| component.effect == MaskEffect::Relight)
        || masks
            .masks
            .iter()
            .any(|mask| mask.effect == MaskEffect::Relight);
    let custom_aspect = matches!(edits.geometry.aspect_ratio, CropAspectRatio::Custom { .. });
    let removes_chromatic_aberration = edits.exposure.chromatic_aberration != 0.0;
    if uses_relight || edits.lens.manual.is_some() || custom_aspect || removes_chromatic_aberration
    {
        V2_SCHEMA_VERSION
    } else {
        V1_SCHEMA_VERSION
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::luts::{LookLut, LutId};
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
        // The fields v1.3 and v1.2 require, at the defaults they gave them.
        let exposure = &document["edits"]["exposure"];
        for (field, value) in [
            ("ca_red", 0.0),
            ("ca_blue", 0.0),
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
    fn relight_anywhere_is_written_as_schema_2() {
        for in_mask in [false, true] {
            let edits = edits(Some(in_mask));
            let bytes = encode(edits.clone()).unwrap();
            assert_eq!(document(&bytes)["schema_version"], V2_SCHEMA_VERSION);
            let loaded = decode(&bytes).unwrap();
            assert!(!loaded.migrated);
            assert_eq!(loaded.edits, edits, "in_mask={in_mask}");
        }
    }

    #[test]
    fn manual_lens_correction_is_written_as_schema_2() {
        let mut edits = edits(None);
        edits.lens.enabled = true;
        edits.lens.manual = Some(crate::pipeline::ManualLensCorrection {
            distortion: 12.0,
            vignetting: -4.0,
        });
        let bytes = encode(edits.clone()).unwrap();
        assert_eq!(document(&bytes)["schema_version"], V2_SCHEMA_VERSION);
        assert_eq!(decode(&bytes).unwrap().edits, edits);
    }

    #[test]
    fn chromatic_aberration_removal_is_written_as_schema_2() {
        let mut edits = edits(None);
        edits.exposure.chromatic_aberration = 60.0;
        let bytes = encode(edits.clone()).unwrap();
        assert_eq!(document(&bytes)["schema_version"], V2_SCHEMA_VERSION);
        assert_eq!(decode(&bytes).unwrap().edits, edits);
    }

    #[test]
    fn a_custom_crop_ratio_is_written_as_schema_2() {
        let mut edits = edits(None);
        edits.geometry.aspect_ratio = CropAspectRatio::Custom {
            width: 5,
            height: 4,
        };
        let bytes = encode(edits.clone()).unwrap();
        assert_eq!(document(&bytes)["schema_version"], V2_SCHEMA_VERSION);
        assert_eq!(decode(&bytes).unwrap().edits, edits);

        edits.geometry.aspect_ratio = CropAspectRatio::FourThree;
        let bytes = encode(edits).unwrap();
        assert_eq!(document(&bytes)["schema_version"], V1_SCHEMA_VERSION);
    }

    #[test]
    fn a_lut_is_written_as_the_newest_schema() {
        let mut edits = edits(Some(false));
        edits.look_lut = Some(LookLut {
            id: LutId::of_bytes(b"film"),
            name: "Film".to_owned(),
            amount: 0.5,
        });
        let bytes = encode(edits.clone()).unwrap();
        assert_eq!(document(&bytes)["schema_version"], SIDECAR_SCHEMA_VERSION);
        let loaded = decode(&bytes).unwrap();
        assert!(!loaded.migrated);
        assert_eq!(loaded.edits, edits);

        let selection = EditSelection {
            lut: true,
            ..EditSelection::default()
        };
        let preset = Preset::new("Film", "", selection, &edits).unwrap();
        let bytes = preset.encode().unwrap();
        assert_eq!(document(&bytes)["schema_version"], PRESET_SCHEMA_VERSION);
        assert_eq!(Preset::decode(&bytes).unwrap(), preset);
    }

    #[test]
    fn presets_choose_their_schema_like_sidecars() {
        let selection = EditSelection {
            adjustment_groups: AdjustmentGroupSet::from_iter([AdjustmentGroup::Effects]),
            masks: true,
            ..EditSelection::default()
        };
        for (relight, expected) in [(None, V1_SCHEMA_VERSION), (Some(false), V2_SCHEMA_VERSION)] {
            let preset = Preset::new("Light", "", selection, &edits(relight)).unwrap();
            let bytes = preset.encode().unwrap();
            assert_eq!(document(&bytes)["schema_version"], expected);
            assert_eq!(Preset::decode(&bytes).unwrap(), preset);
        }
    }
}
