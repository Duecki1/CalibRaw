//! Preset storage, editing and application for the open photo.
use super::*;
use crate::presets::{Preset, StoredPreset};

mod preview;
use preview::PresetHoverPreview;

/// The folder that stores presets, next to the app settings file.
pub(crate) fn preset_folder_for_settings(settings_path: &Path) -> Option<PathBuf> {
    settings_path.parent().map(|folder| folder.join("presets"))
}

#[derive(Default)]
pub(crate) struct PresetState {
    /// `None` when there is no writable app data folder.
    folder: Option<PathBuf>,
    presets: Vec<StoredPreset>,
    /// Every group, including empty ones, sorted by name.
    groups: Vec<String>,
    /// Files in the preset folder that could not be read.
    pub(crate) load_failures: Vec<String>,
    pub(crate) editor: Option<PresetEditor>,
    pub(crate) pending_delete: Option<PathBuf>,
    pub(crate) group_dialog: Option<GroupDialog>,
    pub(crate) pending_group_delete: Option<String>,
    pub(crate) hover: PresetHoverPreview,
}

impl PresetState {
    pub(crate) fn load(folder: Option<PathBuf>) -> Self {
        let mut state = Self {
            folder,
            ..Self::default()
        };
        if let Err(error) = state.reload() {
            log::warn!("could not read presets: {error}");
        }
        state
    }

    pub(crate) fn reload(&mut self) -> Result<(), String> {
        let Some(folder) = self.folder.as_deref() else {
            return Ok(());
        };
        let contents = crate::presets::load_preset_folder(folder)
            .map_err(|error| format!("Could not read presets in {}: {error}", folder.display()))?;
        self.presets = contents.presets;
        self.groups = contents.groups;
        self.load_failures = contents.failures;
        Ok(())
    }

    pub(crate) fn is_available(&self) -> bool {
        self.folder.is_some()
    }

    /// Every preset, sorted by group and then name.
    pub(crate) fn all(&self) -> &[StoredPreset] {
        &self.presets
    }

    pub(crate) fn get(&self, path: &Path) -> Option<&Preset> {
        self.presets
            .iter()
            .find(|stored| stored.path == path)
            .map(|stored| &stored.preset)
    }

    /// Every group, including empty ones, in display order.
    pub(crate) fn groups(&self) -> &[String] {
        &self.groups
    }

    /// The preset listed as `name` in `group`, other than the one at `except`.
    pub(crate) fn find_named(
        &self,
        name: &str,
        group: &str,
        except: Option<&Path>,
    ) -> Option<&StoredPreset> {
        self.presets.iter().find(|stored| {
            Some(stored.path.as_path()) != except && stored.preset.is_named(name, group)
        })
    }

    pub(crate) fn dialog_open(&self) -> bool {
        self.editor.is_some()
            || self.pending_delete.is_some()
            || self.group_dialog.is_some()
            || self.pending_group_delete.is_some()
    }

    fn folder(&self) -> Result<&Path, String> {
        self.folder.as_deref().ok_or_else(|| {
            "Presets are unavailable because CalibRaw has no settings folder.".to_owned()
        })
    }

    #[cfg(not(target_os = "android"))]
    /// A name for `preset` that does not collide with an existing preset in
    /// its group: "Name", then "Name 2", "Name 3", …
    fn unique_name(&self, preset: &Preset) -> String {
        let base = preset.name();
        std::iter::once(base.to_owned())
            .chain((2..).map(|index| format!("{base} {index}")))
            .find(|name| self.find_named(name, preset.group(), None).is_none())
            .unwrap_or_else(|| base.to_owned())
    }
}

pub(crate) enum PresetEditorMode {
    /// Save settings of the open photo as a new preset.
    Create { edits: Box<SidecarEditState> },
    /// Change the name and group of a saved preset.
    Rename { path: PathBuf },
}

pub(crate) struct PresetEditor {
    pub(crate) mode: PresetEditorMode,
    pub(crate) name: String,
    pub(crate) group: String,
    /// Only used when creating; a rename keeps the preset's settings.
    pub(crate) selection: EditSelection,
    pub(crate) error: Option<String>,
    pub(crate) focus_requested: bool,
}

pub(crate) enum GroupDialogMode {
    Create,
    Rename { group: String },
}

/// Names a new group or renames one. Shared by the Presets panel and the
/// preset editor, where it opens on top.
pub(crate) struct GroupDialog {
    pub(crate) mode: GroupDialogMode,
    pub(crate) name: String,
    pub(crate) error: Option<String>,
    pub(crate) focus_requested: bool,
}

impl CalibRawApp {
    pub(crate) fn preset_at(&self, path: &Path) -> Option<&Preset> {
        self.presets.get(path)
    }

    pub(crate) fn can_create_preset(&self) -> bool {
        self.presets.is_available() && self.develop.loaded_raw.is_some()
    }

    /// Opens the preset editor for the open photo with `group` chosen.
    pub(crate) fn open_new_preset_editor(&mut self, group: String) {
        if !self.can_create_preset() {
            return;
        }
        self.finish_mask_geometry_interaction();
        self.commit_edit_history_now();
        let edits = self.capture_sidecar_edit_state();
        self.presets.editor = Some(PresetEditor {
            selection: crate::presets::suggested_selection(&edits),
            mode: PresetEditorMode::Create {
                edits: Box::new(edits),
            },
            name: String::new(),
            group,
            error: None,
            focus_requested: false,
        });
    }

    pub(crate) fn open_rename_preset_editor(&mut self, path: &Path) {
        let Some(preset) = self.presets.get(path) else {
            return;
        };
        self.presets.editor = Some(PresetEditor {
            name: preset.name().to_owned(),
            group: preset.group().to_owned(),
            selection: preset.selection(),
            mode: PresetEditorMode::Rename {
                path: path.to_owned(),
            },
            error: None,
            focus_requested: false,
        });
    }

    /// Saves the open preset editor. On failure the editor stays open and
    /// shows the error.
    pub(crate) fn confirm_preset_editor(&mut self) {
        let Some(editor) = self.presets.editor.take() else {
            return;
        };
        match self.save_preset_editor(&editor) {
            Ok(message) => {
                self.ui.notice = Some(message);
                if let Err(error) = self.presets.reload() {
                    self.ui.notice = Some(error);
                }
            }
            Err(error) => {
                self.presets.editor = Some(PresetEditor {
                    error: Some(error),
                    ..editor
                });
            }
        }
    }

    fn save_preset_editor(&self, editor: &PresetEditor) -> Result<String, String> {
        let folder = self.presets.folder()?;
        match &editor.mode {
            PresetEditorMode::Create { edits } => {
                let preset = Preset::new(&editor.name, &editor.group, editor.selection, edits)
                    .map_err(|error| sentence_case(&error.to_string()))?;
                // Saving under an existing name replaces that preset; the
                // dialog labels its button "Replace" in that case.
                let existing = self
                    .presets
                    .find_named(preset.name(), preset.group(), None)
                    .map(|stored| stored.path.clone());
                match existing {
                    Some(path) => {
                        crate::presets::write_preset_file(&path, &preset)
                            .map_err(|error| format!("Could not save the preset: {error}"))?;
                        Ok(format!("Replaced preset “{}”.", preset.name()))
                    }
                    None => {
                        crate::presets::save_new_preset(folder, &preset)
                            .map_err(|error| format!("Could not save the preset: {error}"))?;
                        Ok(format!("Saved preset “{}”.", preset.name()))
                    }
                }
            }
            PresetEditorMode::Rename { path } => {
                let preset = self
                    .presets
                    .get(path)
                    .ok_or_else(|| "That preset no longer exists.".to_owned())?
                    .renamed(&editor.name, &editor.group)
                    .map_err(|error| sentence_case(&error.to_string()))?;
                if self
                    .presets
                    .find_named(preset.name(), preset.group(), Some(path))
                    .is_some()
                {
                    return Err(format!(
                        "A preset named “{}” already exists in {}.",
                        preset.name(),
                        preset.group()
                    ));
                }
                crate::presets::write_preset_file(path, &preset)
                    .map_err(|error| format!("Could not rename the preset: {error}"))?;
                Ok(format!("Renamed preset to “{}”.", preset.name()))
            }
        }
    }

    pub(crate) fn open_group_dialog(&mut self, mode: GroupDialogMode) {
        let name = match &mode {
            GroupDialogMode::Create => String::new(),
            GroupDialogMode::Rename { group } => group.clone(),
        };
        self.presets.group_dialog = Some(GroupDialog {
            mode,
            name,
            error: None,
            focus_requested: false,
        });
    }

    /// Creates or renames the group in the open group dialog. A preset editor
    /// underneath follows the change. On failure the dialog shows the error.
    pub(crate) fn confirm_group_dialog(&mut self) {
        let Some(dialog) = self.presets.group_dialog.as_ref() else {
            return;
        };
        let result = self.presets.folder().and_then(|folder| {
            match &dialog.mode {
                GroupDialogMode::Create => crate::presets::create_group(folder, &dialog.name),
                GroupDialogMode::Rename { group } => {
                    crate::presets::rename_group(folder, group, &dialog.name)
                }
            }
            .map_err(|error| sentence_case(&error.to_string()))
        });
        let Some(dialog) = self.presets.group_dialog.take() else {
            return;
        };
        let name = match result {
            Ok(name) => name,
            Err(error) => {
                self.presets.group_dialog = Some(GroupDialog {
                    error: Some(error),
                    ..dialog
                });
                return;
            }
        };
        if let Some(editor) = self.presets.editor.as_mut() {
            let follows = match &dialog.mode {
                GroupDialogMode::Create => true,
                GroupDialogMode::Rename { group } => {
                    editor.group.to_lowercase() == group.to_lowercase()
                }
            };
            if follows {
                editor.group.clone_from(&name);
            }
        }
        if let Err(error) = self.presets.reload() {
            self.ui.notice = Some(error);
        }
    }

    pub(crate) fn delete_preset_group(
        &mut self,
        group: &str,
        presets: crate::presets::DeletedGroupPresets,
    ) {
        let result = self.presets.folder().and_then(|folder| {
            crate::presets::delete_group(folder, group, presets)
                .map_err(|error| sentence_case(&error.to_string()))
        });
        self.ui.notice = Some(match result {
            Ok(()) => format!("Deleted group “{group}”."),
            Err(error) => format!("Could not delete group “{group}”: {error}"),
        });
        if let Err(error) = self.presets.reload() {
            self.ui.notice = Some(error);
        }
    }

    pub(crate) fn delete_preset(&mut self, path: &Path) {
        let name = self
            .presets
            .get(path)
            .map(|preset| preset.name().to_owned())
            .unwrap_or_default();
        self.ui.notice = Some(match std::fs::remove_file(path) {
            Ok(()) => format!("Deleted preset “{name}”."),
            Err(error) => format!("Could not delete preset “{name}”: {error}"),
        });
        if let Err(error) = self.presets.reload() {
            self.ui.notice = Some(error);
        }
    }

    /// Applies a preset to the open photo as one undoable edit.
    pub(crate) fn apply_preset_to_current(&mut self, path: &Path, frame: &eframe::Frame) {
        let Some(preset) = self.presets.get(path).cloned() else {
            self.ui.notice = Some("That preset no longer exists.".to_owned());
            return;
        };
        if self.develop.load_receiver.is_some() {
            self.ui.notice = Some("Wait for the current photo to finish opening.".to_owned());
            return;
        }
        self.end_preset_hover_preview_for(path);
        self.ui.notice = Some(
            match self.apply_edit_transfer_to_current(
                EditTransfer::Preset(&preset),
                EditTransferOrigin::Develop,
                frame,
            ) {
                Ok(_) => format!("Applied preset “{}”.", preset.name()),
                Err(error) => error,
            },
        );
    }

    #[cfg(not(target_os = "android"))]
    /// Copies preset files into the preset folder. Imported presets whose
    /// name is taken in their group get a numbered name.
    pub(crate) fn import_preset_files(&mut self, paths: &[PathBuf]) {
        let folder = match self.presets.folder() {
            Ok(folder) => folder.to_owned(),
            Err(error) => {
                self.ui.notice = Some(error);
                return;
            }
        };
        let mut imported = 0usize;
        let mut failures = Vec::new();
        for path in paths {
            let label = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let result = crate::presets::read_preset_file(path).and_then(|preset| {
                let name = self.presets.unique_name(&preset);
                let preset = preset.renamed(&name, preset.group())?;
                crate::presets::save_new_preset(&folder, &preset)
            });
            match result {
                Ok(_) => {
                    imported += 1;
                    // Later files in this import must see the new names.
                    if let Err(error) = self.presets.reload() {
                        failures.push(error);
                    }
                }
                Err(error) => failures.push(format!("{label}: {error}")),
            }
        }
        let summary = format!(
            "Imported {imported} {}.",
            if imported == 1 { "preset" } else { "presets" }
        );
        self.ui.notice = Some(if failures.is_empty() {
            summary
        } else {
            format!("{summary} {}", failures.join(" · "))
        });
    }

    #[cfg(not(target_os = "android"))]
    pub(crate) fn choose_preset_files_to_import(&mut self) {
        if self.ui.desktop_picker_receiver.is_some() || !self.presets.is_available() {
            return;
        }
        let extension = PRESET_FILE_EXTENSIONS[0];
        let dialog = rfd::AsyncFileDialog::new().add_filter("CalibRaw presets", &[extension]);
        self.ui.desktop_picker_receiver = Some(spawn_ui_worker(&self.egui_ctx, move || {
            let paths = pollster::block_on(dialog.pick_files()).map(|handles| {
                handles
                    .into_iter()
                    .map(|handle| handle.path().to_path_buf())
                    .collect()
            });
            DesktopPickerEvent::PresetFiles(paths)
        }));
    }

    #[cfg(not(target_os = "android"))]
    pub(crate) fn export_preset(&mut self, path: &Path) {
        let Some(preset) = self.presets.get(path).cloned() else {
            return;
        };
        let file_name: String = preset
            .name()
            .chars()
            .map(|character| match character {
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
                character => character,
            })
            .collect();
        let Some(destination) = crate::ui::choose_save_path(
            "CalibRaw preset".to_owned(),
            PRESET_FILE_EXTENSIONS,
            &format!("{file_name}{}", crate::presets::PRESET_SUFFIX),
            None,
        ) else {
            return;
        };
        self.ui.notice = Some(
            match crate::presets::write_preset_file(&destination, &preset) {
                Ok(()) => format!(
                    "Exported preset “{}” to {}.",
                    preset.name(),
                    destination.display()
                ),
                Err(error) => format!("Could not export the preset: {error}"),
            },
        );
    }
}

/// [`crate::presets::PRESET_SUFFIX`] without its leading dot, for file pickers.
#[cfg(not(target_os = "android"))]
const PRESET_FILE_EXTENSIONS: &[&str] = &["calibraw-preset"];

pub(crate) fn sentence_case(message: &str) -> String {
    let mut characters = message.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_file_extension_matches_the_preset_suffix() {
        #[cfg(not(target_os = "android"))]
        assert_eq!(
            format!(".{}", PRESET_FILE_EXTENSIONS[0]),
            crate::presets::PRESET_SUFFIX
        );
        assert_eq!(
            preset_folder_for_settings(Path::new("/config/calibraw/performance.json")),
            Some(PathBuf::from("/config/calibraw/presets"))
        );
    }

    #[test]
    fn errors_are_shown_as_sentences() {
        assert_eq!(sentence_case("enter a preset name"), "Enter a preset name");
        assert_eq!(sentence_case(""), "");
    }
}
