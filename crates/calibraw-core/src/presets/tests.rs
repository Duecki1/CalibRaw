use super::*;
use crate::pipeline::{AdjustmentGroup, AdjustmentGroupSet, MaskImage};
use crate::sidecar::LensEditState;
use std::time::{SystemTime, UNIX_EPOCH};

fn edited_photo() -> EditState {
    let mut edits = default_edit_state();
    edits.exposure.exposure = 0.7;
    edits.exposure.clarity = 25.0;
    edits.exposure.temperature = 12.0;
    edits.exposure.demosaic_mode = crate::pipeline::DemosaicMode::Dual;
    edits.camera_profile = Some(PathBuf::from("Canon/EOS R5 Standard.dcp"));
    edits.lens = LensEditState {
        enabled: true,
        maker: "Test Optics".to_owned(),
        model: "35 mm f/2".to_owned(),
    };
    let masks = Arc::make_mut(&mut edits.masks);
    masks.add_mask(MaskKind::Linear).unwrap();
    masks.add_mask(MaskKind::Brush).unwrap();
    masks.add_mask(MaskKind::Sky).unwrap();
    let MaskGeometry::Ai { mask: sky, .. } = &mut masks.masks[2].components[0].geometry else {
        panic!("sky masks are generated");
    };
    *sky = MaskImage::new(2, 1, vec![0, 255]);
    masks.scene_depth = MaskImage::new(2, 1, vec![10, 20]);
    edits
}

fn selection(groups: &[AdjustmentGroup]) -> EditSelection {
    EditSelection {
        adjustment_groups: groups.iter().copied().collect(),
        ..EditSelection::default()
    }
}

#[test]
fn presets_capture_only_the_selected_groups() {
    let preset = Preset::new(
        "Punchy",
        "",
        selection(&[AdjustmentGroup::Effects]),
        &edited_photo(),
    )
    .unwrap();

    assert_eq!(preset.group(), DEFAULT_PRESET_GROUP);
    assert_eq!(preset.edits().exposure.clarity, 25.0);
    assert_eq!(preset.edits().exposure.exposure, 0.0);
    assert_eq!(
        preset.edits().exposure.demosaic_mode,
        crate::pipeline::ExposureParams::default().demosaic_mode
    );
    assert_eq!(preset.edits().camera_profile, None);
    assert!(preset.edits().masks.masks.is_empty());
}

#[test]
fn presets_drop_photo_specific_mask_data() {
    let preset = Preset::new(
        "Sky and gradient",
        "Landscape",
        EditSelection {
            masks: true,
            ai_masks: true,
            ..EditSelection::default()
        },
        &edited_photo(),
    )
    .unwrap();

    let kinds: Vec<_> = preset
        .edits()
        .masks
        .masks
        .iter()
        .flat_map(|mask| &mask.components)
        .map(|component| component.kind)
        .collect();
    assert_eq!(kinds, vec![MaskKind::Linear, MaskKind::Sky]);
    assert!(preset.edits().masks.scene_depth.is_none());
    assert!(matches!(
        &preset.edits().masks.masks[1].components[0].geometry,
        MaskGeometry::Ai { mask: None, .. }
    ));
    assert!(!preset.edits().ai_masks_need_update);
}

#[test]
fn applying_a_preset_keeps_unselected_settings_and_adds_masks() {
    let preset = Preset::new(
        "Look",
        "",
        EditSelection {
            adjustment_groups: [AdjustmentGroup::Effects].into_iter().collect(),
            masks: true,
            ai_masks: true,
            ..EditSelection::default()
        },
        &edited_photo(),
    )
    .unwrap();

    let mut destination = default_edit_state();
    destination.exposure.exposure = -1.0;
    destination.exposure.clarity = -50.0;
    Arc::make_mut(&mut destination.masks)
        .add_mask(MaskKind::Radial)
        .unwrap();
    preset.apply_to(&mut destination);

    assert_eq!(destination.exposure.exposure, -1.0);
    assert_eq!(destination.exposure.clarity, 25.0);
    let kinds: Vec<_> = destination
        .masks
        .masks
        .iter()
        .map(|mask| mask.components[0].kind)
        .collect();
    assert_eq!(
        kinds,
        vec![MaskKind::Radial, MaskKind::Linear, MaskKind::Sky]
    );
    assert!(destination.ai_masks_need_update);
}

#[test]
fn applying_adjustment_groups_does_not_request_ai_updates() {
    let preset = Preset::new(
        "Warm",
        "",
        selection(&[AdjustmentGroup::Color]),
        &edited_photo(),
    )
    .unwrap();
    let mut destination = default_edit_state();
    preset.apply_to(&mut destination);
    assert_eq!(destination.exposure.temperature, 12.0);
    assert!(destination.masks.masks.is_empty());
    assert!(!destination.ai_masks_need_update);
}

#[test]
fn presets_round_trip_through_their_file_format() {
    let preset = Preset::new(
        "  Film look ",
        " Film ",
        EditSelection {
            adjustment_groups: AdjustmentGroupSet::ALL,
            camera_profile: true,
            masks: true,
            lens_correction: true,
            ..EditSelection::default()
        },
        &edited_photo(),
    )
    .unwrap();
    assert_eq!(preset.name(), "Film look");
    assert_eq!(preset.group(), "Film");

    let decoded = Preset::decode(&preset.encode().unwrap()).unwrap();
    assert_eq!(decoded, preset);
}

#[test]
fn decoding_filters_hand_edited_files_like_new_presets() {
    let mut photo = edited_photo();
    Arc::make_mut(&mut photo.masks).scene_depth = None;
    let document = PresetDocument {
        format: PRESET_FORMAT.to_owned(),
        schema_version: PRESET_SCHEMA_VERSION,
        name: "Edited".to_owned(),
        group: String::new(),
        selection: selection(&[AdjustmentGroup::Light]),
        edits: photo,
    };
    let preset = Preset::decode(&serde_json::to_vec(&document).unwrap()).unwrap();
    assert!(preset.edits().masks.masks.is_empty());
    assert_eq!(preset.edits().lens, LensEditState::default());
    assert_eq!(preset.edits().exposure.exposure, 0.7);
}

#[test]
fn decoding_rejects_other_formats_and_newer_schemas() {
    let preset = Preset::new(
        "Plain",
        "",
        selection(&[AdjustmentGroup::Light]),
        &edited_photo(),
    )
    .unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&preset.encode().unwrap()).unwrap();

    value["schema_version"] = (PRESET_SCHEMA_VERSION + 1).into();
    assert!(matches!(
        Preset::decode(&serde_json::to_vec(&value).unwrap()),
        Err(PresetError::Unsupported(_))
    ));

    value["schema_version"] = PRESET_SCHEMA_VERSION.into();
    value["format"] = "CalibRaw edit sidecar".into();
    assert!(matches!(
        Preset::decode(&serde_json::to_vec(&value).unwrap()),
        Err(PresetError::Invalid(_))
    ));
}

#[test]
fn names_are_validated_and_compared_as_displayed() {
    let edits = edited_photo();
    let light = selection(&[AdjustmentGroup::Light]);
    assert!(Preset::new("   ", "", light, &edits).is_err());
    assert!(Preset::new("a\nb", "", light, &edits).is_err());
    assert!(Preset::new(&"x".repeat(MAX_PRESET_NAME_CHARS + 1), "", light, &edits).is_err());
    assert!(Preset::new("Empty", "", EditSelection::default(), &edits).is_err());

    let preset = Preset::new("Matte", "Film", light, &edits).unwrap();
    assert!(preset.is_named(" matte ", "FILM"));
    assert!(!preset.is_named("Matte", ""));
    let renamed = preset.renamed("Faded", "").unwrap();
    assert!(renamed.is_named("Faded", DEFAULT_PRESET_GROUP));
    assert_eq!(renamed.edits(), preset.edits());
}

#[test]
fn suggested_selection_includes_only_edited_portable_categories() {
    let suggested = suggested_selection(&edited_photo());
    let groups: Vec<_> = suggested.adjustment_groups.iter().collect();
    assert_eq!(
        groups,
        vec![
            AdjustmentGroup::Light,
            AdjustmentGroup::Color,
            AdjustmentGroup::Effects
        ]
    );
    assert!(suggested.camera_profile);
    assert!(suggested.masks);
    assert!(suggested.ai_masks);
    assert!(!suggested.geometry);
    assert!(!suggested.lens_correction);
    assert!(!suggested.raw_processing);
    assert!(suggested_selection(&default_edit_state()).is_empty());
}

fn temporary_folder(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "calibraw-presets-{label}-{}-{nanos}",
        std::process::id()
    ))
}

#[test]
fn preset_folders_save_list_and_report_damaged_files() {
    let folder = temporary_folder("folder");
    assert!(load_preset_folder(&folder).unwrap().presets.is_empty());

    let edits = edited_photo();
    let light = selection(&[AdjustmentGroup::Light]);
    let zebra = Preset::new("Zebra", "B", light, &edits).unwrap();
    let alpha = Preset::new("Alpha / Beta", "A", light, &edits).unwrap();
    let zebra_path = save_new_preset(&folder, &zebra).unwrap();
    let alpha_path = save_new_preset(&folder, &alpha).unwrap();
    let duplicate_path = save_new_preset(&folder, &alpha).unwrap();
    assert_eq!(
        alpha_path.file_name().unwrap(),
        "a-alpha-beta.calibraw-preset"
    );
    assert_eq!(
        duplicate_path.file_name().unwrap(),
        "a-alpha-beta-2.calibraw-preset"
    );
    std::fs::write(folder.join("broken.calibraw-preset"), b"{").unwrap();
    std::fs::write(folder.join("notes.txt"), b"ignored").unwrap();

    let contents = load_preset_folder(&folder).unwrap();
    let names: Vec<_> = contents
        .presets
        .iter()
        .map(|stored| stored.preset.name())
        .collect();
    assert_eq!(names, vec!["Alpha / Beta", "Alpha / Beta", "Zebra"]);
    assert_eq!(contents.failures.len(), 1);
    assert!(contents.failures[0].starts_with("broken.calibraw-preset"));
    assert!(contents
        .presets
        .iter()
        .any(|stored| stored.path == zebra_path));

    std::fs::remove_dir_all(folder).unwrap();
}

fn depth_fog() -> crate::pipeline::EffectComponent {
    let mut fog = crate::pipeline::EffectComponent::new(crate::pipeline::MaskEffect::Fog);
    fog.settings.fog.amount = 50.0;
    fog.settings.fog.density = 50.0;
    fog.settings.fog.depth_enabled = true;
    fog
}

#[test]
fn previews_show_adjustments_and_hand_placed_masks_only() {
    let mut photo = edited_photo();
    photo.exposure.ai_denoise_enabled = true;
    let masks = Arc::make_mut(&mut photo.masks);
    masks.add_mask(MaskKind::Radial).unwrap();
    masks
        .add_component(MaskKind::Sky, crate::pipeline::MaskCombineMode::Subtract)
        .unwrap();
    let preset = Preset::new(
        "Everything",
        "",
        EditSelection {
            adjustment_groups: AdjustmentGroupSet::ALL,
            camera_profile: true,
            masks: true,
            ai_masks: true,
            geometry: true,
            lens_correction: true,
            ..EditSelection::default()
        },
        &photo,
    )
    .unwrap();

    let mut exposure = crate::pipeline::ExposureParams::default();
    assert!(!exposure.ai_denoise_enabled);
    preset.preview_adjustments_on(&mut exposure);
    assert_eq!(exposure.exposure, 0.7);
    assert!(!exposure.ai_denoise_enabled, "AI denoise runs a model");
    assert_eq!(exposure.clarity, 25.0);
    assert_eq!(
        exposure.demosaic_mode,
        crate::pipeline::ExposureParams::default().demosaic_mode
    );

    let mut stack = MaskStack::default();
    stack.add_mask(MaskKind::Fullscreen).unwrap();
    preset.preview_masks_on(&mut stack);
    // The sky mask and the radial mask that subtracts sky both need AI.
    let kinds: Vec<_> = stack
        .masks
        .iter()
        .map(|mask| mask.components[0].kind)
        .collect();
    assert_eq!(kinds, vec![MaskKind::Fullscreen, MaskKind::Linear]);
    assert!(stack.content_dependencies().is_empty());
}

#[test]
fn previews_skip_masks_the_preset_does_not_include() {
    let preset = Preset::new(
        "Tone only",
        "",
        selection(&[AdjustmentGroup::Light]),
        &edited_photo(),
    )
    .unwrap();
    let mut stack = MaskStack::default();
    preset.preview_masks_on(&mut stack);
    assert!(stack.masks.is_empty());
}

#[test]
fn previews_leave_out_effects_that_need_missing_scene_depth() {
    let mut photo = default_edit_state();
    Arc::make_mut(&mut photo.masks)
        .global_effects
        .push(depth_fog());
    let preset = Preset::new(
        "Fog",
        "",
        EditSelection {
            masks: true,
            ..EditSelection::default()
        },
        &photo,
    )
    .unwrap();

    let mut without_depth = MaskStack::default();
    preset.preview_masks_on(&mut without_depth);
    assert!(without_depth.global_effects.is_empty());

    let mut with_depth = MaskStack {
        scene_depth: MaskImage::new(2, 1, vec![0, 255]),
        ..MaskStack::default()
    };
    preset.preview_masks_on(&mut with_depth);
    assert_eq!(with_depth.global_effects, vec![depth_fog()]);
}
