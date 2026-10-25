//! The LUT library and the LUT of the open photo.
//!
//! Imported `.cube` files are copied into the library (`calibraw_core::luts`)
//! and edits name their LUT by id. The open photo's LUT table is read off the
//! UI thread; the previews render the photo without it until it is read, and
//! when the library does not hold it.
use super::*;
use crate::app::preview_visibility::PreviewVisibility;
use crate::luts::{
    CubeLut, LookLut, LutEntry, LutId, LutLibrary, LutLibraryError, LutStore, ResolvedLook,
};

/// The title of the Develop card that holds the LUT, which its preview eye
/// is keyed by.
pub(crate) const LOOK_LUT_CARD_TITLE: &str = "LUT";

#[derive(Default)]
pub(crate) struct LutState {
    /// `None` without a writable app data folder, or when its LUT list could
    /// not be read (`failure`).
    library: Option<LutLibrary>,
    /// Why the library could not be opened.
    failure: Option<String>,
    /// The table being read for the open photo's LUT.
    loading: Option<LutTableLoad>,
    /// Apply the first LUT of the next import to the open photo, for an
    /// import started from the Develop card.
    pub(crate) apply_next_import: bool,
    /// The LUT whose removal waits for confirmation in Settings.
    pub(crate) pending_remove: Option<LutId>,
    /// The LUT being renamed in Settings.
    pub(crate) rename: Option<LutRename>,
    /// The LUT the preview last could not upload, reported once.
    reported_upload_failure: Option<LutId>,
}

/// The Settings dialog that renames a library LUT.
pub(crate) struct LutRename {
    pub(crate) id: LutId,
    /// The name typed so far.
    pub(crate) name: String,
    pub(crate) focus_requested: bool,
}

struct LutTableLoad {
    id: LutId,
    receiver: mpsc::Receiver<Result<Arc<CubeLut>, LutLibraryError>>,
}

impl LutState {
    pub(crate) fn open(folder: Option<PathBuf>) -> Self {
        let Some(folder) = folder else {
            return Self::default();
        };
        match LutLibrary::open(folder) {
            Ok(library) => Self {
                library: Some(library),
                ..Self::default()
            },
            Err(error) => {
                log::warn!("could not read the LUT library: {error}");
                Self {
                    failure: Some(format!("Could not read the LUT library: {error}")),
                    ..Self::default()
                }
            }
        }
    }

    pub(crate) fn is_available(&self) -> bool {
        self.library.is_some()
    }

    pub(crate) fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// Every library LUT, sorted by name.
    pub(crate) fn entries(&self) -> &[LutEntry] {
        self.library.as_ref().map_or(&[], LutLibrary::entries)
    }

    pub(crate) fn entry(&self, id: LutId) -> Option<&LutEntry> {
        self.library.as_ref()?.entry(id)
    }

    /// Reads library LUTs on any thread.
    pub(crate) fn store(&self) -> Option<LutStore> {
        self.library.as_ref().map(|library| library.store().clone())
    }
}

/// Whether both render the same LUT at the same amount.
fn same_look(left: Option<&ResolvedLook>, right: Option<&ResolvedLook>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            Arc::ptr_eq(&left.lut, &right.lut) && left.amount == right.amount
        }
        _ => false,
    }
}

/// The notice for a LUT the library does not hold.
pub(crate) fn missing_look_lut_notice(look: &LookLut) -> String {
    format!(
        "The LUT “{}” is not in the LUT library; the photo is shown without it.",
        look.name
    )
}

/// `look` loaded for an export, which fails rather than leave out a LUT the
/// library does not hold.
pub(in crate::app) fn resolve_look_for_export(
    store: Option<&LutStore>,
    look: &LookLut,
) -> Result<ResolvedLook, String> {
    let missing = || {
        format!(
            "The LUT “{}” is not in the LUT library. Import it, or choose another LUT, to export this photo.",
            look.name
        )
    };
    let lut = store
        .ok_or_else(missing)?
        .load(look.id)
        .map_err(|error| match error {
            LutLibraryError::NotFound(_) => missing(),
            error => format!("Could not read the LUT “{}”: {error}", look.name),
        })?;
    Ok(ResolvedLook {
        lut,
        amount: look.amount,
    })
}

impl CalibRawApp {
    /// The edit's LUT for an export of the open photo.
    pub(in crate::app) fn look_for_export(&self) -> Result<Option<ResolvedLook>, String> {
        let Some(look) = &self.develop.look_lut else {
            return Ok(None);
        };
        if let Some(resolved) = self.resolved_look() {
            return Ok(Some(resolved));
        }
        resolve_look_for_export(self.luts.store().as_ref(), look).map(Some)
    }

    /// The override `choice` describes, with the preset and LUT it names
    /// looked up in their libraries.
    pub(crate) fn export_override(&self, choice: &ExportOverrideChoice) -> ExportOverride {
        let preset = choice
            .preset
            .as_deref()
            .and_then(|path| self.presets.get(path));
        let look_lut = choice
            .look_lut
            .and_then(|id| self.luts.entry(id))
            .map(|entry| LookLut {
                amount: choice.look_amount,
                ..LookLut::new(entry)
            });
        ExportOverride::new(preset, look_lut)
    }

    /// Selects library LUT `id`, or no LUT, as a change of the edit. The
    /// amount carries over from the previous LUT.
    pub(crate) fn select_look_lut(&mut self, id: Option<LutId>) {
        let amount = self
            .develop
            .look_lut
            .as_ref()
            .map_or(LookLut::DEFAULT_AMOUNT, |look| look.amount);
        let look = id.and_then(|id| self.luts.entry(id)).map(|entry| LookLut {
            amount,
            ..LookLut::new(entry)
        });
        self.set_look_lut(look);
    }

    /// Sets the amount of the edit's LUT, 0–1.
    pub(crate) fn set_look_lut_amount(&mut self, amount: f32) {
        if let Some(look) = &self.develop.look_lut {
            let look = LookLut {
                amount: amount.clamp(0.0, 1.0),
                ..look.clone()
            };
            self.set_look_lut(Some(look));
        }
    }

    fn set_look_lut(&mut self, look_lut: Option<LookLut>) {
        if self.develop.look_lut == look_lut {
            return;
        }
        self.install_look_lut(look_lut);
        self.note_edit_changed();
    }

    /// Makes `look_lut` the open photo's LUT and reads its table unless it
    /// is the one already read.
    pub(crate) fn install_look_lut(&mut self, look_lut: Option<LookLut>) {
        self.develop.look_lut = look_lut;
        let wanted = self.develop.look_lut.as_ref().map(|look| look.id);
        if self.develop.look_lut_table.as_ref().map(|table| table.id()) != wanted {
            self.develop.look_lut_table = None;
            self.read_look_lut_table();
        }
        self.sync_preview_look();
    }

    /// Installs the LUT a document load read with the photo, or the reason
    /// it could not, without reading it again.
    pub(in crate::app) fn install_loaded_look_lut(
        &mut self,
        look_lut: Option<LookLut>,
        table: Option<Arc<CubeLut>>,
    ) {
        self.luts.loading = None;
        self.develop.look_lut = look_lut;
        self.develop.look_lut_table = table;
        self.develop.target_look = self.preview_look();
    }

    fn read_look_lut_table(&mut self) {
        let Some(id) = self.develop.look_lut.as_ref().map(|look| look.id) else {
            self.luts.loading = None;
            return;
        };
        if self.luts.loading.as_ref().is_some_and(|load| load.id == id) {
            return;
        }
        let Some(store) = self.luts.store() else {
            self.luts.loading = None;
            self.report_missing_look_lut();
            return;
        };
        if let Some(table) = store.cached(id) {
            self.luts.loading = None;
            self.develop.look_lut_table = Some(table);
            return;
        }
        let receiver = spawn_ui_worker(&self.egui_ctx, move || store.load(id));
        self.luts.loading = Some(LutTableLoad { id, receiver });
    }

    pub(in crate::app) fn poll_look_lut_table(&mut self) {
        let Some(load) = self.luts.loading.as_ref() else {
            return;
        };
        let result = match load.receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err(LutLibraryError::InvalidIndex(
                "the LUT reader stopped unexpectedly".to_owned(),
            )),
        };
        let id = load.id;
        self.luts.loading = None;
        if self.develop.look_lut.as_ref().map(|look| look.id) != Some(id) {
            return;
        }
        match result {
            Ok(table) => {
                self.develop.look_lut_table = Some(table);
                self.sync_preview_look();
            }
            Err(LutLibraryError::NotFound(_)) => self.report_missing_look_lut(),
            Err(error) => self.report_error(
                ErrorKind::Luts,
                format!("Could not read the photo's LUT: {error}"),
            ),
        }
    }

    /// Reports that the preview could not upload its LUT, once per LUT. The
    /// preview renders without it (`RawGpuPipeline::prepare_look_lut`).
    pub(in crate::app) fn report_look_lut_upload_failure(&mut self, error: &anyhow::Error) {
        let id = self.develop.target_look.as_ref().map(|look| look.lut.id());
        if id.is_none() || self.luts.reported_upload_failure == id {
            return;
        }
        self.luts.reported_upload_failure = id;
        self.report_error(
            ErrorKind::Preview,
            format!("The preview is shown without its LUT: {error:#}"),
        );
    }

    fn report_missing_look_lut(&mut self) {
        if let Some(look) = &self.develop.look_lut {
            self.ui.notice = Some(missing_look_lut_notice(look));
        }
    }

    /// The edit's LUT at its amount, once its table is read.
    pub(crate) fn resolved_look(&self) -> Option<ResolvedLook> {
        let look = self.develop.look_lut.as_ref()?;
        let lut = self.develop.look_lut_table.clone()?;
        Some(ResolvedLook {
            lut,
            amount: look.amount,
        })
    }

    /// The LUT the edited previews render: the edit's, unless the LUT card's
    /// preview eye hides it.
    pub(in crate::app) fn preview_look(&self) -> Option<ResolvedLook> {
        if !PreviewVisibility::globally_visible(&self.egui_ctx, LOOK_LUT_CARD_TITLE) {
            return None;
        }
        self.resolved_look()
    }

    /// Brings the previews' LUT up to date, rendering the Output stage again
    /// when it changed.
    pub(in crate::app) fn sync_preview_look(&mut self) {
        let look = self.preview_look();
        if same_look(look.as_ref(), self.develop.target_look.as_ref()) {
            return;
        }
        self.develop.target_look = look;
        self.queue_preview_processing(ProcessingStage::Output);
    }

    /// Asks for `.cube` files to add to the library. With `apply`, the first
    /// imported LUT becomes the open photo's.
    pub(crate) fn choose_lut_files_to_import(&mut self, apply: bool) {
        if !self.luts.is_available() {
            return;
        }
        if self.caps().has_folders() {
            if self.ui.desktop_picker_receiver.is_some() {
                return;
            }
            let dialog = crate::platform::FileDialog::new().add_filter("3D LUTs", &["cube"]);
            self.luts.apply_next_import = apply;
            self.ui.desktop_picker_receiver = Some(spawn_ui_worker(&self.egui_ctx, move || {
                DesktopPickerEvent::LutFiles(dialog.pick_files_on_worker())
            }));
        } else {
            match self.platform.open_lut_files() {
                Ok(()) => self.luts.apply_next_import = apply,
                Err(error) => self.report_error(ErrorKind::Luts, error),
            }
        }
    }

    /// Adds the `.cube` files at `paths` to the library.
    pub(crate) fn import_lut_files(&mut self, paths: &[PathBuf]) {
        let apply = std::mem::take(&mut self.luts.apply_next_import);
        let Some(library) = self.luts.library.as_mut() else {
            return;
        };
        let mut imported = Vec::new();
        let mut failures = Vec::new();
        for path in paths {
            match library.import_file(path) {
                Ok(import) => imported.push(import.entry),
                Err(error) => failures.push(format!(
                    "{}: {error}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                )),
            }
        }
        if apply && self.develop.loaded_raw.is_some() {
            if let Some(entry) = imported.first() {
                self.select_look_lut(Some(entry.id));
            }
        }
        let summary = format!(
            "Imported {} {}.",
            imported.len(),
            if imported.len() == 1 { "LUT" } else { "LUTs" }
        );
        if failures.is_empty() {
            self.ui.notice = Some(summary);
        } else {
            self.report_error(
                ErrorKind::Luts,
                format!("{summary} {}", failures.join(" · ")),
            );
        }
    }

    /// Imports the files a platform picker copied into `folder`, then deletes it.
    pub(in crate::app) fn import_staged_lut_files(&mut self, folder: &Path) {
        let mut paths: Vec<PathBuf> = match std::fs::read_dir(folder) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .collect(),
            Err(error) => {
                self.luts.apply_next_import = false;
                self.report_error(
                    ErrorKind::Luts,
                    format!("Could not read the picked LUT files: {error}"),
                );
                return;
            }
        };
        // Staged names start with their position in the selection.
        paths.sort();
        self.import_lut_files(&paths);
        if let Err(error) = std::fs::remove_dir_all(folder) {
            log::warn!("could not delete the LUT import folder: {error}");
        }
    }

    /// Opens the rename dialog for LUT `id`.
    pub(crate) fn start_lut_rename(&mut self, id: LutId) {
        self.luts.rename = self.luts.entry(id).map(|entry| LutRename {
            id,
            name: entry.name.clone(),
            focus_requested: false,
        });
    }

    pub(crate) fn rename_lut(&mut self, id: LutId, name: &str) {
        let Some(library) = self.luts.library.as_mut() else {
            return;
        };
        match library.rename(id, name) {
            Ok(name) => {
                self.luts.rename = None;
                self.ui.notice = Some(format!("Renamed the LUT to “{name}”."));
            }
            Err(error) => self.report_error(
                ErrorKind::Luts,
                format!("Could not rename the LUT: {error}"),
            ),
        }
    }

    /// Removes LUT `id` from the library. Edits that use it keep naming it
    /// and render without it.
    pub(crate) fn remove_lut(&mut self, id: LutId) {
        self.luts.pending_remove = None;
        let Some(library) = self.luts.library.as_mut() else {
            return;
        };
        let name = library.entry(id).map(|entry| entry.name.clone());
        match library.remove(id) {
            Ok(()) => {
                if self.develop.look_lut.as_ref().map(|look| look.id) == Some(id) {
                    self.develop.look_lut_table = None;
                    self.sync_preview_look();
                }
                self.ui.notice = name.map(|name| format!("Removed the LUT “{name}”."));
            }
            Err(error) => self.report_error(
                ErrorKind::Luts,
                format!("Could not remove the LUT: {error}"),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    fn warm_cube(size: u32) -> String {
        let mut text = format!("TITLE \"Warm\"\nLUT_3D_SIZE {size}\n");
        let last = (size - 1) as f32;
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    let [r, g, b] = [r as f32 / last, g as f32 / last, b as f32 / last];
                    writeln!(text, "{} {} {}", (r * 1.1).min(1.0), g, b * 0.9).unwrap();
                }
            }
        }
        text
    }

    /// An app with a LUT library in `folder` holding one imported LUT.
    fn app_with_lut(folder: &Path) -> (CalibRawApp, LutId) {
        let mut app = CalibRawApp::empty(&egui::Context::default());
        app.luts = LutState::open(Some(folder.join("luts")));
        let file = folder.join("warm.cube");
        std::fs::write(&file, warm_cube(3)).unwrap();
        app.import_lut_files(&[file]);
        let id = app.luts.entries()[0].id;
        (app, id)
    }

    #[test]
    fn a_selected_lut_is_saved_with_its_amount_and_undone() {
        let folder = tempfile::tempdir().unwrap();
        let (mut app, id) = app_with_lut(folder.path());
        assert_eq!(app.luts.entries()[0].name, "Warm");

        app.select_look_lut(Some(id));
        app.set_look_lut_amount(0.25);
        app.commit_edit_history_now();
        let saved = app.capture_sidecar_edit_state().look_lut.unwrap();
        assert_eq!(
            (saved.id, saved.name.as_str(), saved.amount),
            (id, "Warm", 0.25)
        );
        // An imported LUT is read already, so the previews render it at once.
        let look = app.develop.target_look.as_ref().unwrap();
        assert_eq!((look.lut.id(), look.amount), (id, 0.25));

        app.undo_edit();
        assert_eq!(app.develop.look_lut, None);
        assert!(app.develop.target_look.is_none());
        app.redo_edit();
        assert_eq!(
            app.develop.look_lut.as_ref().map(|look| look.amount),
            Some(0.25)
        );
    }

    #[test]
    fn a_lut_missing_from_the_library_stays_in_the_edit_and_fails_exports() {
        let folder = tempfile::tempdir().unwrap();
        let (mut app, id) = app_with_lut(folder.path());
        app.select_look_lut(Some(id));
        app.remove_lut(id);

        assert!(app.capture_sidecar_edit_state().look_lut.is_some());
        assert!(app.develop.target_look.is_none());
        let error = app.look_for_export().unwrap_err();
        assert!(
            error.contains("“Warm” is not in the LUT library"),
            "{error}"
        );
    }

    #[test]
    fn the_export_override_applies_the_preset_then_the_lut() {
        let folder = tempfile::tempdir().unwrap();
        let (app, id) = app_with_lut(folder.path());
        let mut source = crate::sidecar::default_edit_state();
        source.exposure.exposure = 1.25;
        source.exposure.ai_denoise_enabled = true;
        let preset = crate::presets::Preset::new(
            "Bright",
            "",
            EditSelection {
                adjustment_groups: [
                    crate::pipeline::AdjustmentGroup::Light,
                    crate::pipeline::AdjustmentGroup::Detail,
                ]
                .into_iter()
                .collect(),
                ..EditSelection::default()
            },
            &source,
        )
        .unwrap();

        let export_override = ExportOverride::new(
            Some(&preset),
            app.luts.entry(id).map(|entry| LookLut {
                amount: 0.5,
                ..LookLut::new(entry)
            }),
        );
        let mut edits = crate::sidecar::default_edit_state();
        export_override.apply(&mut edits);
        assert_eq!(edits.exposure.exposure, 1.25);
        assert!(
            !edits.exposure.ai_denoise_enabled,
            "the photo keeps its AI denoise"
        );
        assert_eq!(
            edits.look_lut.map(|look| (look.id, look.amount)),
            Some((id, 0.5))
        );
    }
}
