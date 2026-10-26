//! Preset storage, editing and application for the open photo.
use super::*;
use crate::presets::{Preset, PresetError, StoredPreset};

mod amount;
mod preview;
use amount::AppliedPreset;
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
    /// The preset applied last, whose amount may still change.
    applied: Option<AppliedPreset>,
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

    /// The native preset listed as `name` in `group`, other than the one at
    /// `except`. A Lightroom preset with the same name is not replaceable.
    pub(crate) fn find_named_native(
        &self,
        name: &str,
        group: &str,
        except: Option<&Path>,
    ) -> Option<&StoredPreset> {
        self.presets.iter().find(|stored| {
            crate::presets::is_native_preset_file(&stored.path)
                && Some(stored.path.as_path()) != except
                && stored.preset.is_named(name, group)
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
                    self.report_error(ErrorKind::Presets, error);
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
                // Saving under an existing native name replaces that preset;
                // the dialog labels its button "Replace" in that case. A
                // Lightroom preset is never replaced by a native one, so a
                // shared name gets a numbered native preset instead.
                let existing = self
                    .presets
                    .find_named_native(preset.name(), preset.group(), None)
                    .map(|stored| stored.path.clone());
                match existing {
                    Some(path) => {
                        crate::presets::write_preset_file(&path, &preset)
                            .map_err(|error| format!("Could not save the preset: {error}"))?;
                        Ok(format!("Replaced preset “{}”.", preset.name()))
                    }
                    None => {
                        let name = self.presets.unique_name(&preset);
                        let preset = preset
                            .renamed(&name, preset.group())
                            .map_err(|error| sentence_case(&error.to_string()))?;
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
            self.report_error(ErrorKind::Presets, error);
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
        match result {
            Ok(()) => self.ui.notice = Some(format!("Deleted group “{group}”.")),
            Err(error) => self.report_error(
                ErrorKind::Presets,
                format!("Could not delete group “{group}”: {error}"),
            ),
        }
        if let Err(error) = self.presets.reload() {
            self.report_error(ErrorKind::Presets, error);
        }
        self.forget_deleted_new_photo_preset();
    }

    pub(crate) fn delete_preset(&mut self, path: &Path) {
        let name = self
            .presets
            .get(path)
            .map(|preset| preset.name().to_owned())
            .unwrap_or_default();
        match std::fs::remove_file(path) {
            Ok(()) => self.ui.notice = Some(format!("Deleted preset “{name}”.")),
            Err(error) => self.report_error(
                ErrorKind::Presets,
                format!("Could not delete preset “{name}”: {error}"),
            ),
        }
        if let Err(error) = self.presets.reload() {
            self.report_error(ErrorKind::Presets, error);
        }
        self.forget_deleted_new_photo_preset();
    }

    pub(crate) fn new_photo_preset_path(&self) -> Option<PathBuf> {
        let file = self.preferences.new_photo_preset.as_ref()?;
        Some(self.presets.folder().ok()?.join(file))
    }

    pub(crate) fn apply_new_photo_preset(&mut self, frame: &eframe::Frame) {
        let Some(path) = self.new_photo_preset_path() else {
            return;
        };
        let Some(preset) = self.presets.get(&path).cloned() else {
            log::warn!("the preset for new photos is missing: {}", path.display());
            return;
        };
        if let Err(error) = self.apply_edit_transfer_to_current(
            EditTransfer::Preset(&preset),
            EditTransferOrigin::Develop,
            frame,
        ) {
            self.report_error(
                ErrorKind::Presets,
                format!("Could not apply preset “{}”: {error}", preset.name()),
            );
        }
    }

    fn forget_deleted_new_photo_preset(&mut self) {
        if self
            .new_photo_preset_path()
            .is_some_and(|path| !path.exists())
        {
            self.set_new_photo_preset(None);
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
        let result = self.apply_preset_with_amount(&preset, |app| {
            app.apply_edit_transfer_to_current(
                EditTransfer::Preset(&preset),
                EditTransferOrigin::Develop,
                frame,
            )
        });
        self.ui.notice = Some(match result {
            Ok(_) => format!("Applied preset “{}”.", preset.name()),
            Err(error) => error,
        });
    }

    /// Copies preset files into the preset folder. Imported presets whose
    /// name is taken in their group get a numbered name. A Lightroom XMP is
    /// copied as an XMP, keeping settings CalibRaw cannot translate yet.
    /// Settings without an equivalent are reported as warnings in the notice.
    pub(crate) fn import_preset_files(&mut self, paths: &[PathBuf]) {
        let folder = match self.presets.folder() {
            Ok(folder) => folder.to_owned(),
            Err(error) => {
                self.report_error(ErrorKind::Presets, error);
                return;
            }
        };
        let mut imported = 0usize;
        let mut failures = Vec::new();
        let mut warnings = Vec::new();
        for path in paths {
            let label = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let result = read_preset_for_import(path).and_then(|imported| {
                let name = self.presets.unique_name(&imported.preset);
                let preset = imported.preset.renamed(&name, imported.preset.group())?;
                match imported.source {
                    Some(source) => {
                        crate::presets::save_new_lightroom_preset(&folder, &preset, &source)?;
                    }
                    None => {
                        crate::presets::save_new_preset(&folder, &preset)?;
                    }
                }
                Ok(imported.warnings)
            });
            match result {
                Ok(file_warnings) => {
                    imported += 1;
                    for warning in file_warnings {
                        log::warn!("{label}: {warning}");
                        warnings.push(format!("{label}: {warning}"));
                    }
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
        let warning_text = warnings.join(" ");
        if failures.is_empty() {
            self.ui.notice = Some(if warning_text.is_empty() {
                summary
            } else {
                format!("{summary} {warning_text}")
            });
        } else {
            let mut message = format!("{summary} {}", failures.join(" · "));
            if !warning_text.is_empty() {
                message.push(' ');
                message.push_str(&warning_text);
            }
            self.report_error(ErrorKind::Presets, message);
        }
    }

    pub(crate) fn choose_preset_files_to_import(&mut self) {
        if self.ui.desktop_picker_receiver.is_some() || !self.presets.is_available() {
            return;
        }
        let dialog = crate::platform::FileDialog::new()
            .add_filter("CalibRaw and Lightroom presets", PRESET_IMPORT_EXTENSIONS);
        self.ui.desktop_picker_receiver = Some(spawn_ui_worker(&self.egui_ctx, move || {
            DesktopPickerEvent::PresetFiles(dialog.pick_files_on_worker())
        }));
    }

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
        let is_lightroom = crate::presets::lightroom::is_lightroom_preset_file(path);
        let (filter, extensions, suffix) = if is_lightroom {
            (
                "Lightroom preset",
                LIGHTROOM_FILE_EXTENSIONS,
                crate::presets::lightroom::LIGHTROOM_PRESET_SUFFIX,
            )
        } else {
            (
                "CalibRaw preset",
                PRESET_FILE_EXTENSIONS,
                crate::presets::PRESET_SUFFIX,
            )
        };
        let Some(destination) = crate::ui::choose_save_path(
            filter.to_owned(),
            extensions,
            &format!("{file_name}{suffix}"),
            None,
        ) else {
            return;
        };
        match crate::presets::export_preset_file(path, &destination, &preset) {
            Ok(()) => {
                self.ui.notice = Some(format!(
                    "Exported preset “{}” to {}.",
                    preset.name(),
                    destination.display()
                ));
            }
            Err(error) => self.report_error(
                ErrorKind::Presets,
                format!("Could not export the preset: {error}"),
            ),
        }
    }
}

/// [`crate::presets::PRESET_SUFFIX`] without its leading dot, for file pickers.
const PRESET_FILE_EXTENSIONS: &[&str] = &["calibraw-preset"];
/// [`crate::presets::lightroom::LIGHTROOM_PRESET_SUFFIX`] without its dot.
const LIGHTROOM_FILE_EXTENSIONS: &[&str] = &["xmp"];
/// Both preset formats in one import filter, so users see every preset file
/// at once. Some tools export `.XMP`; Linux pickers match case-sensitively.
const PRESET_IMPORT_EXTENSIONS: &[&str] = &["calibraw-preset", "xmp", "XMP"];

struct ImportedPreset {
    preset: Preset,
    warnings: Vec<String>,
    /// The XMP bytes of a Lightroom preset, kept as the copied source.
    source: Option<Vec<u8>>,
}

/// Reads a CalibRaw or Lightroom preset file, with the warnings of a
/// Lightroom translation and the XMP source when there is one.
fn read_preset_for_import(path: &Path) -> Result<ImportedPreset, PresetError> {
    if crate::presets::lightroom::is_lightroom_preset_file(path) {
        let (import, source) =
            crate::presets::lightroom::read_lightroom_preset_file_with_bytes(path)?;
        Ok(ImportedPreset {
            preset: import.preset,
            warnings: import.warnings,
            source: Some(source),
        })
    } else {
        Ok(ImportedPreset {
            preset: crate::presets::read_preset_file(path)?,
            warnings: Vec::new(),
            source: None,
        })
    }
}

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
    use crate::pipeline::{AdjustmentGroup, AdjustmentGroupSet, MaskKind};

    #[test]
    fn deleting_the_preset_for_new_photos_stops_applying_it() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = CalibRawApp::empty(&egui::Context::default());
        app.presets = PresetState::load(Some(folder.path().to_owned()));
        let selection = EditSelection {
            adjustment_groups: AdjustmentGroupSet::from_iter([AdjustmentGroup::Color]),
            ..EditSelection::default()
        };
        let edits = crate::sidecar::default_edit_state();
        let save = |name| {
            let preset = Preset::new(name, "", selection, &edits).unwrap();
            crate::presets::save_new_preset(folder.path(), &preset).unwrap()
        };
        let warm = save("Warm");
        let cool = save("Cool");
        app.presets.reload().unwrap();

        app.set_new_photo_preset(warm.file_name().map(PathBuf::from));
        assert_eq!(app.new_photo_preset_path(), Some(warm.clone()));

        app.delete_preset(&cool);
        assert_eq!(app.new_photo_preset_path(), Some(warm.clone()));

        app.delete_preset(&warm);
        assert_eq!(app.preferences.new_photo_preset, None);
        assert_eq!(app.new_photo_preset_path(), None);
    }

    #[test]
    fn preset_file_extension_matches_the_preset_suffix() {
        assert_eq!(
            format!(".{}", PRESET_FILE_EXTENSIONS[0]),
            crate::presets::PRESET_SUFFIX
        );
        assert_eq!(crate::presets::lightroom::LIGHTROOM_PRESET_SUFFIX, ".xmp");
        assert!(PRESET_IMPORT_EXTENSIONS.contains(&PRESET_FILE_EXTENSIONS[0]));
        assert!(PRESET_IMPORT_EXTENSIONS.contains(&LIGHTROOM_FILE_EXTENSIONS[0]));
        assert_eq!(
            preset_folder_for_settings(Path::new("/config/calibraw/performance.json")),
            Some(PathBuf::from("/config/calibraw/presets"))
        );
    }

    #[test]
    fn importing_a_lightroom_preset_copies_the_xmp() {
        let folder = tempfile::tempdir().unwrap();
        let source_folder = tempfile::tempdir().unwrap();
        let mut app = CalibRawApp::empty(&egui::Context::default());
        app.presets = PresetState::load(Some(folder.path().to_owned()));

        let xmp = source_folder.path().join("Warm Look.xmp");
        let original = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
 crs:WhiteBalance="As Shot" crs:Exposure2012="0.5" crs:Vibrance="20" />
</rdf:RDF>
</x:xmpmeta>"#;
        std::fs::write(&xmp, original).unwrap();

        app.import_preset_files(std::slice::from_ref(&xmp));

        assert_eq!(app.presets.all().len(), 1);
        let stored = &app.presets.all()[0];
        assert_eq!(stored.preset.name(), "Warm Look");
        assert_eq!(stored.preset.group(), crate::presets::DEFAULT_PRESET_GROUP);
        assert_eq!(stored.preset.edits().exposure.exposure, 0.5);
        assert_eq!(stored.preset.edits().exposure.vibrance, 20.0);
        assert_eq!(
            stored
                .path
                .extension()
                .and_then(|extension| extension.to_str()),
            Some("xmp")
        );
        assert_ne!(stored.path, xmp);
        assert_eq!(std::fs::read_to_string(&xmp).unwrap(), original);
        assert!(stored.path.exists());
        assert_eq!(app.ui.notice.as_deref(), Some("Imported 1 preset."));
    }

    #[test]
    fn importing_a_lightroom_preset_keeps_warnings_out_of_failures() {
        let folder = tempfile::tempdir().unwrap();
        let source_folder = tempfile::tempdir().unwrap();
        let mut app = CalibRawApp::empty(&egui::Context::default());
        app.presets = PresetState::load(Some(folder.path().to_owned()));

        let xmp = source_folder.path().join("Corrected.xmp");
        std::fs::write(
            &xmp,
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
 crs:Exposure2012="0" crs:LensProfileEnable="1" />
</rdf:RDF>
</x:xmpmeta>"#,
        )
        .unwrap();

        app.import_preset_files(&[xmp]);

        assert_eq!(app.presets.all().len(), 1);
        let notice = app.ui.notice.as_deref().unwrap();
        assert!(notice.starts_with("Imported 1 preset."), "{notice}");
        assert!(notice.contains("Lens corrections were not imported."));
    }

    #[test]
    fn importing_a_mask_only_lightroom_preset_saves_an_ai_mask() {
        let folder = tempfile::tempdir().unwrap();
        let source_folder = tempfile::tempdir().unwrap();
        let mut app = CalibRawApp::empty(&egui::Context::default());
        app.presets = PresetState::load(Some(folder.path().to_owned()));

        let xmp = source_folder.path().join("Subject Look.xmp");
        std::fs::write(
            &xmp,
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/">
<crs:MaskGroupBasedCorrections><rdf:Seq><rdf:li>
<rdf:Description crs:What="Correction" crs:CorrectionName="Subject">
<crs:CorrectionMasks><rdf:Seq>
<rdf:li crs:What="Mask/Image" crs:MaskName="Subject 1" crs:MaskSubType="1"/>
</rdf:Seq></crs:CorrectionMasks>
</rdf:Description>
</rdf:li></rdf:Seq></crs:MaskGroupBasedCorrections>
</rdf:Description>
</rdf:RDF>
</x:xmpmeta>"#,
        )
        .unwrap();

        app.import_preset_files(&[xmp]);

        assert_eq!(app.presets.all().len(), 1);
        let stored = &app.presets.all()[0];
        assert!(stored.preset.selection().ai_masks);
        assert_eq!(stored.preset.edits().masks.masks.len(), 1);
        assert_eq!(
            stored.preset.edits().masks.masks[0].components[0].kind,
            MaskKind::Subject
        );
        assert_eq!(app.ui.notice.as_deref(), Some("Imported 1 preset."));
    }

    #[test]
    fn importing_an_unsupported_lightroom_preset_keeps_it_listed() {
        let folder = tempfile::tempdir().unwrap();
        let source_folder = tempfile::tempdir().unwrap();
        let mut app = CalibRawApp::empty(&egui::Context::default());
        app.presets = PresetState::load(Some(folder.path().to_owned()));

        let xmp = source_folder.path().join("Lens only.xmp");
        std::fs::write(
            &xmp,
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
 crs:LensProfileEnable="1" />
</rdf:RDF>
</x:xmpmeta>"#,
        )
        .unwrap();

        app.import_preset_files(&[xmp]);

        assert_eq!(app.presets.all().len(), 1);
        let stored = &app.presets.all()[0];
        assert!(stored.preset.selection().is_empty());
        assert_eq!(stored.preset.name(), "Lens only");
        assert_eq!(
            stored
                .path
                .extension()
                .and_then(|extension| extension.to_str()),
            Some("xmp")
        );
        let notice = app.ui.notice.as_deref().unwrap();
        assert!(notice.starts_with("Imported 1 preset."), "{notice}");
        assert!(notice.contains("No Lightroom settings"));
    }

    #[test]
    fn renaming_a_lightroom_preset_rewrites_only_the_copied_xmp_labels() {
        let folder = tempfile::tempdir().unwrap();
        let source_folder = tempfile::tempdir().unwrap();
        let mut app = CalibRawApp::empty(&egui::Context::default());
        app.presets = PresetState::load(Some(folder.path().to_owned()));

        let xmp = source_folder.path().join("Warm Look.xmp");
        let original = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
 crs:Name="Warm Look" crs:Group="Film" crs:Exposure2012="0.5" />
</rdf:RDF>
</x:xmpmeta>"#;
        std::fs::write(&xmp, original).unwrap();
        app.import_preset_files(std::slice::from_ref(&xmp));

        let path = app.presets.all()[0].path.clone();
        app.open_rename_preset_editor(&path);
        let editor = app.presets.editor.as_mut().unwrap();
        editor.name = "Hot Look".to_owned();
        editor.group = "Analog".to_owned();
        app.confirm_preset_editor();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(r#"crs:Name="Hot Look""#));
        assert!(text.contains(r#"crs:Group="Analog""#));
        assert!(text.contains(r#"crs:Exposure2012="0.5""#));
        assert_eq!(std::fs::read_to_string(&xmp).unwrap(), original);
        assert_eq!(app.presets.all()[0].preset.name(), "Hot Look");
        assert_eq!(app.presets.all()[0].preset.group(), "Analog");
    }

    #[test]
    fn creating_a_native_preset_does_not_replace_a_lightroom_preset() {
        let folder = tempfile::tempdir().unwrap();
        let source_folder = tempfile::tempdir().unwrap();
        let mut app = CalibRawApp::empty(&egui::Context::default());
        app.presets = PresetState::load(Some(folder.path().to_owned()));

        let xmp = source_folder.path().join("Warm.xmp");
        std::fs::write(
            &xmp,
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
 crs:Name="Warm" crs:Exposure2012="0.5" />
</rdf:RDF>
</x:xmpmeta>"#,
        )
        .unwrap();
        app.import_preset_files(&[xmp]);
        let xmp_path = app.presets.all()[0].path.clone();

        let edits = crate::sidecar::default_edit_state();
        let selection = EditSelection {
            adjustment_groups: AdjustmentGroupSet::from_iter([AdjustmentGroup::Color]),
            ..EditSelection::default()
        };
        app.presets.editor = Some(PresetEditor {
            mode: PresetEditorMode::Create {
                edits: Box::new(edits),
            },
            name: "Warm".to_owned(),
            group: String::new(),
            selection,
            error: None,
            focus_requested: false,
        });
        app.confirm_preset_editor();

        assert!(xmp_path.exists());
        assert_eq!(app.presets.all().len(), 2);
        assert!(app
            .presets
            .all()
            .iter()
            .any(|stored| { stored.path == xmp_path && stored.preset.name() == "Warm" }));
        assert!(app.presets.all().iter().any(|stored| {
            crate::presets::is_native_preset_file(&stored.path) && stored.preset.name() == "Warm 2"
        }));
    }

    #[test]
    fn errors_are_shown_as_sentences() {
        assert_eq!(sentence_case("enter a preset name"), "Enter a preset name");
        assert_eq!(sentence_case(""), "");
    }
}
