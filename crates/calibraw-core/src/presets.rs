//! Presets: named, reusable selections of develop settings.
//!
//! A preset keeps only the categories chosen when it was created, and never
//! data tied to the photo it came from: inpainting, hand-drawn and object
//! masks, generated mask rasters and scene depth. Applying a preset leaves
//! every category it does not include untouched and adds its masks next to
//! the photo's own.
//!
//! Presets live one per file in a folder, so they can be shared by copying
//! files and a damaged file never hides the others.

use crate::file_ops::write_bytes_atomically;
use crate::pipeline::{ExposureParams, MaskGeometry, MaskKind, MaskStack, MAX_LOCAL_MASKS};
use crate::sidecar::{
    default_edit_state, transfer_edits, validate_edit_state, AdjustmentPasteMode, EditSelection,
    EditState,
};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const PRESET_SUFFIX: &str = ".calibraw-preset";
/// Bump for every incompatible change to the serialized layout.
pub const PRESET_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_PRESET_GROUP: &str = "User Presets";
pub const MAX_PRESET_NAME_CHARS: usize = 64;
/// Presets hold no image data, so anything larger is not a preset.
pub const MAX_PRESET_BYTES: u64 = 4 * 1024 * 1024;

const PRESET_FORMAT: &str = "CalibRaw preset";
const MAX_PRESET_FILES: usize = 4096;
const MAX_FILE_STEM_CHARS: usize = 48;

#[derive(Debug)]
pub enum PresetError {
    Io(std::io::Error),
    Invalid(String),
    Unsupported(String),
    TooLarge(u64),
}

impl fmt::Display for PresetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Invalid(message) | Self::Unsupported(message) => formatter.write_str(message),
            Self::TooLarge(bytes) => write!(
                formatter,
                "preset file is {bytes} bytes; the limit is {MAX_PRESET_BYTES} bytes"
            ),
        }
    }
}

impl std::error::Error for PresetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Invalid(_) | Self::Unsupported(_) | Self::TooLarge(_) => None,
        }
    }
}

impl From<std::io::Error> for PresetError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

fn invalid<T>(message: impl Into<String>) -> Result<T, PresetError> {
    Err(PresetError::Invalid(message.into()))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    name: String,
    group: String,
    selection: EditSelection,
    edits: EditState,
}

impl Preset {
    /// Captures the `selection` categories of `edits` as a preset.
    ///
    /// An empty `group` files the preset under [`DEFAULT_PRESET_GROUP`].
    pub fn new(
        name: &str,
        group: &str,
        selection: EditSelection,
        edits: &EditState,
    ) -> Result<Self, PresetError> {
        let name = normalize_name(name)?;
        let group = normalize_group(group)?;
        if selection.is_empty() {
            return invalid("a preset must include at least one setting");
        }
        let mut captured = default_edit_state();
        transfer_edits(&mut captured, edits, selection, AdjustmentPasteMode::Merge);
        captured.masks = Arc::new(portable_masks(&captured.masks));
        captured.subject_refinement = None;
        captured.ai_masks_need_update = false;
        Ok(Self {
            name,
            group,
            selection,
            edits: captured,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn group(&self) -> &str {
        &self.group
    }

    pub fn selection(&self) -> EditSelection {
        self.selection
    }

    /// The captured settings. Categories outside [`Self::selection`] hold defaults.
    pub fn edits(&self) -> &EditState {
        &self.edits
    }

    /// The same preset under a different name or group.
    pub fn renamed(&self, name: &str, group: &str) -> Result<Self, PresetError> {
        Ok(Self {
            name: normalize_name(name)?,
            group: normalize_group(group)?,
            ..self.clone()
        })
    }

    /// Whether this preset would be listed as `name` in `group`. Names are
    /// compared the way they are shown, so case and surrounding spaces do not
    /// make two presets distinct.
    pub fn is_named(&self, name: &str, group: &str) -> bool {
        let group = match normalize_group(group) {
            Ok(group) => group,
            Err(_) => return false,
        };
        self.name.to_lowercase() == name.trim().to_lowercase()
            && self.group.to_lowercase() == group.to_lowercase()
    }

    /// Applies the preset to `destination`.
    ///
    /// Included settings overwrite the destination's. Masks and global
    /// effects are added after the destination's own instead of replacing
    /// them. Masks that depend on image content set `ai_masks_need_update`.
    pub fn apply_to(&self, destination: &mut EditState) {
        let without_masks = EditSelection {
            masks: false,
            ai_masks: false,
            ..self.selection
        };
        transfer_edits(
            destination,
            &self.edits,
            without_masks,
            AdjustmentPasteMode::Merge,
        );

        let preset_masks = &self.edits.masks;
        if preset_masks.masks.is_empty() && preset_masks.global_effects.is_empty() {
            return;
        }
        let masks = Arc::make_mut(&mut destination.masks);
        masks.masks.extend(preset_masks.masks.iter().cloned());
        masks
            .global_effects
            .extend(preset_masks.global_effects.iter().cloned());
        destination.ai_masks_need_update |=
            !preset_masks.content_dependencies().is_empty() || masks.scene_depth_missing();
    }

    // A quick preview shows the part of a preset that renders without reloading
    // the photo or running AI models: the adjustment groups, plus masks and
    // effects placed by hand. Camera profile, lens correction, geometry, RAW
    // processing, AI denoise and AI masks only take effect when the preset is
    // applied.

    /// Applies the preset's adjustment groups to `exposure` for a quick preview.
    pub fn preview_adjustments_on(&self, exposure: &mut ExposureParams) {
        let ai_denoise_enabled = exposure.ai_denoise_enabled;
        for group in self.selection.adjustment_groups.iter() {
            exposure.copy_group_from(&self.edits.exposure, group);
        }
        exposure.ai_denoise_enabled = ai_denoise_enabled;
    }

    /// Adds the preset's hand-placed masks and global effects to `masks` for a
    /// quick preview. Masks with any AI component are left out, as are effects
    /// that need a scene depth this photo does not have yet.
    pub fn preview_masks_on(&self, masks: &mut MaskStack) {
        if !self.selection.masks {
            return;
        }
        let preset_masks = &self.edits.masks;
        masks.masks.extend(
            preset_masks
                .masks
                .iter()
                .filter(|mask| {
                    mask.components
                        .iter()
                        .all(|component| crate::sidecar::is_manual_mask_kind(component.kind))
                })
                .cloned(),
        );
        masks.masks.truncate(MAX_LOCAL_MASKS);

        let depth_was_missing = masks.scene_depth_missing();
        let own_effects = masks.global_effects.len();
        masks
            .global_effects
            .extend(preset_masks.global_effects.iter().cloned());
        if !depth_was_missing && masks.scene_depth_missing() {
            masks.global_effects.truncate(own_effects);
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, PresetError> {
        let document = PresetDocument {
            format: PRESET_FORMAT.to_owned(),
            schema_version: PRESET_SCHEMA_VERSION,
            name: self.name.clone(),
            group: self.group.clone(),
            selection: self.selection,
            edits: self.edits.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&document)
            .map_err(|error| PresetError::Invalid(format!("could not encode preset: {error}")))?;
        if bytes.len() as u64 > MAX_PRESET_BYTES {
            return Err(PresetError::TooLarge(bytes.len() as u64));
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, PresetError> {
        if bytes.len() as u64 > MAX_PRESET_BYTES {
            return Err(PresetError::TooLarge(bytes.len() as u64));
        }
        let header: PresetHeader = serde_json::from_slice(bytes)
            .map_err(|error| PresetError::Invalid(format!("invalid preset JSON: {error}")))?;
        if header.format != PRESET_FORMAT {
            return invalid("not a CalibRaw preset");
        }
        // Keep each future schema's decoder and migration in its own arm so an
        // older layout is never read as the current one by accident.
        let document = match header.schema_version {
            PRESET_SCHEMA_VERSION => serde_json::from_slice::<PresetDocument>(bytes)
                .map_err(|error| PresetError::Invalid(format!("invalid preset JSON: {error}")))?,
            version if version > PRESET_SCHEMA_VERSION => {
                return Err(PresetError::Unsupported(format!(
                    "preset schema {version} is newer than supported schema {PRESET_SCHEMA_VERSION}; update CalibRaw to use it"
                )))
            }
            version => {
                return Err(PresetError::Unsupported(format!(
                    "preset schema {version} has no migration path to supported schema {PRESET_SCHEMA_VERSION}"
                )))
            }
        };

        // Rebuild through `new` so a hand-edited file gets the same filtering
        // as a preset created in the app.
        let mut preset = Self::new(
            &document.name,
            &document.group,
            document.selection,
            &document.edits,
        )?;
        validate_edit_state(&preset.edits)
            .map_err(|error| PresetError::Invalid(error.to_string()))?;
        preset.edits.exposure.sanitize_tone_curves();
        for mask in &mut Arc::make_mut(&mut preset.edits.masks).masks {
            mask.adjustments.sanitize_tone_curves();
        }
        Ok(preset)
    }
}

#[derive(Deserialize)]
struct PresetHeader {
    format: String,
    schema_version: u32,
}

#[derive(Deserialize, Serialize)]
struct PresetDocument {
    format: String,
    schema_version: u32,
    name: String,
    #[serde(default)]
    group: String,
    selection: EditSelection,
    edits: EditState,
}

fn normalize_name(name: &str) -> Result<String, PresetError> {
    let name = name.trim();
    if name.is_empty() {
        return invalid("enter a preset name");
    }
    validate_label(name, "preset name")?;
    Ok(name.to_owned())
}

fn normalize_group(group: &str) -> Result<String, PresetError> {
    let group = group.trim();
    if group.is_empty() {
        return Ok(DEFAULT_PRESET_GROUP.to_owned());
    }
    validate_label(group, "group name")?;
    Ok(group.to_owned())
}

fn validate_label(label: &str, kind: &str) -> Result<(), PresetError> {
    if label.chars().count() > MAX_PRESET_NAME_CHARS {
        return invalid(format!(
            "the {kind} is longer than {MAX_PRESET_NAME_CHARS} characters"
        ));
    }
    if label.chars().any(char::is_control) {
        return invalid(format!("the {kind} contains control characters"));
    }
    Ok(())
}

/// Whether a mask component can be reproduced on another photo. Brush, path
/// and object components follow the content of the photo they were drawn on.
pub fn is_portable_mask_kind(kind: MaskKind) -> bool {
    !matches!(kind, MaskKind::Brush | MaskKind::Path | MaskKind::Object)
}

/// The masks of `masks` that a preset can carry: portable components only,
/// with every raster generated from the source photo removed.
pub fn portable_masks(masks: &MaskStack) -> MaskStack {
    let mut portable = MaskStack {
        masks: masks.masks.clone(),
        global_effects: masks.global_effects.clone(),
        ..MaskStack::default()
    };
    portable.masks.retain_mut(|mask| {
        mask.components
            .retain(|component| is_portable_mask_kind(component.kind));
        for component in &mut mask.components {
            match &mut component.geometry {
                MaskGeometry::Ai { mask, .. } => *mask = None,
                MaskGeometry::DepthRange { depth, .. } => *depth = None,
                MaskGeometry::LuminanceRange { source, .. }
                | MaskGeometry::ColorRange { source, .. } => *source = None,
                _ => {}
            }
        }
        !mask.components.is_empty()
    });
    portable
}

/// The categories a new preset from `edits` should include by default: every
/// edited adjustment group, a chosen camera profile and any portable masks.
/// Geometry, lens correction and RAW processing are photo-specific and left out.
pub fn suggested_selection(edits: &EditState) -> EditSelection {
    let portable = portable_masks(&edits.masks);
    let has_manual = !portable.global_effects.is_empty()
        || portable
            .masks
            .iter()
            .flat_map(|mask| &mask.components)
            .any(|component| crate::sidecar::is_manual_mask_kind(component.kind));
    let has_ai = portable
        .masks
        .iter()
        .flat_map(|mask| &mask.components)
        .any(|component| !crate::sidecar::is_manual_mask_kind(component.kind));
    EditSelection {
        adjustment_groups: crate::pipeline::AdjustmentGroup::ALL
            .into_iter()
            .filter(|group| edits.exposure.group_is_edited(*group))
            .collect(),
        camera_profile: edits.camera_profile.is_some(),
        masks: has_manual,
        ai_masks: has_ai,
        ..EditSelection::default()
    }
}

/// A preset and the file it was read from or saved to.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredPreset {
    pub path: PathBuf,
    pub preset: Preset,
}

#[derive(Debug, Default)]
pub struct PresetFolderContents {
    /// Sorted by group, then name.
    pub presets: Vec<StoredPreset>,
    /// One message per file that could not be read.
    pub failures: Vec<String>,
}

/// Reads every preset in `folder`. A missing folder holds no presets.
pub fn load_preset_folder(folder: &Path) -> Result<PresetFolderContents, PresetError> {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PresetFolderContents::default())
        }
        Err(error) => return Err(error.into()),
    };
    let mut contents = PresetFolderContents::default();
    for entry in entries.take(MAX_PRESET_FILES) {
        let path = entry?.path();
        if !is_preset_file(&path) {
            continue;
        }
        match read_preset_file(&path) {
            Ok(preset) => contents.presets.push(StoredPreset { path, preset }),
            Err(error) => contents.failures.push(format!(
                "{}: {error}",
                path.file_name().unwrap_or_default().to_string_lossy()
            )),
        }
    }
    contents.presets.sort_by_cached_key(|stored| {
        (
            stored.preset.group.to_lowercase(),
            stored.preset.name.to_lowercase(),
        )
    });
    Ok(contents)
}

pub fn is_preset_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(PRESET_SUFFIX) && !name.starts_with('.'))
}

pub fn read_preset_file(path: &Path) -> Result<Preset, PresetError> {
    let file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    if length > MAX_PRESET_BYTES {
        return Err(PresetError::TooLarge(length));
    }
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(MAX_PRESET_BYTES + 1).read_to_end(&mut bytes)?;
    Preset::decode(&bytes)
}

/// Writes `preset` to `path`, replacing any file there.
pub fn write_preset_file(path: &Path, preset: &Preset) -> Result<(), PresetError> {
    write_bytes_atomically(path, &preset.encode()?)?;
    Ok(())
}

/// Saves `preset` as a new file in `folder` and returns its path. The file
/// name is derived from the group and name and never replaces another file.
pub fn save_new_preset(folder: &Path, preset: &Preset) -> Result<PathBuf, PresetError> {
    let stem = preset_file_stem(preset);
    let path = (1..=MAX_PRESET_FILES)
        .map(|index| {
            let suffix = if index == 1 {
                String::new()
            } else {
                format!("-{index}")
            };
            folder.join(format!("{stem}{suffix}{PRESET_SUFFIX}"))
        })
        .find(|path| !path.exists())
        .ok_or_else(|| PresetError::Invalid("too many presets with the same name".to_owned()))?;
    write_preset_file(&path, preset)?;
    Ok(path)
}

fn preset_file_stem(preset: &Preset) -> String {
    let mut stem = String::new();
    for character in format!("{} {}", preset.group, preset.name).chars() {
        if character.is_alphanumeric() {
            stem.extend(character.to_lowercase());
        } else if !stem.is_empty() && !stem.ends_with('-') {
            stem.push('-');
        }
    }
    let stem: String = stem.chars().take(MAX_FILE_STEM_CHARS).collect();
    let stem = stem.trim_end_matches('-');
    if stem.is_empty() {
        "preset".to_owned()
    } else {
        stem.to_owned()
    }
}

#[cfg(test)]
mod tests;
