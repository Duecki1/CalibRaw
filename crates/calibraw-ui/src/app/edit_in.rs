//! Edit In: renders a photo into the Edit In folder and opens the file in
//! another editor, or hands that editor the photo's own file. The render is an
//! ordinary export with the Edit In settings, so it shares the export slot;
//! the editor starts once the file is saved. Nothing comes back into
//! CalibRaw.

use super::*;
use crate::edit_in::{EditInFormat, EditInTarget, ExternalEditor};
use crate::export_folders::EditInFolder;
use crate::i18n::{tr, tr_fmt};
use crate::pipeline::OutputColorSpace;
use crate::platform::EditInSupport;

/// How long a file stays in the app-cache Edit In folder after it last
/// changed. Counting from the last change keeps an image an editor saved over
/// for as long again.
const CACHE_FILE_LIFETIME: Duration = Duration::from_secs(7 * 24 * 60 * 60);

impl CalibRawApp {
    pub(crate) fn edit_in_support(&self) -> EditInSupport {
        self.platform.edit_in_support()
    }

    /// The editors the Edit In menus offer, in order.
    pub(crate) fn edit_in_targets(&self) -> Vec<EditInTarget> {
        match self.edit_in_support() {
            EditInSupport::Unavailable => Vec::new(),
            EditInSupport::SystemChooser => vec![EditInTarget::SystemChooser],
            EditInSupport::Editors => self
                .preferences
                .edit_in
                .editors
                .iter()
                .cloned()
                .map(EditInTarget::Editor)
                .collect(),
        }
    }

    /// Edit In for the open photo, with its current edit.
    pub(crate) fn edit_open_photo_in(&mut self, frame: &eframe::Frame, target: EditInTarget) {
        let Some(photo) = self.develop.current_path.clone() else {
            return;
        };
        if target.sends_original() {
            self.open_in_editor(&target, &photo);
            return;
        }
        if !self.can_export() {
            return;
        }
        let directory = match self.edit_in_directory(&photo) {
            Ok(directory) => directory,
            Err(error) => {
                self.report_error(ErrorKind::EditIn, error);
                return;
            }
        };
        let format = self.preferences.edit_in.format.export_format();
        let destination = ExportDestination::NewFile {
            directory,
            stem: edit_in_stem(&photo),
            extension: format.extension().to_owned(),
        };
        let mut request = match self.capture_export_task_request(
            ExportItemOutput::Single(destination),
            frame,
            format,
            None,
        ) {
            Ok(Some(request)) => request,
            Ok(None) => return,
            Err(error) => {
                self.report_error(ErrorKind::EditIn, error);
                return;
            }
        };
        request.settings = self.preferences.edit_in.export_settings();
        match self.start_export_task(request, ExportTaskKind::Single) {
            Ok(()) => self.export.edit_in = Some(target),
            Err(error) => self.report_error(
                ErrorKind::EditIn,
                tr_fmt("Export failed: {error}", &[("error", &error)]),
            ),
        }
    }

    /// Edit In for a library photo, with its saved edit.
    pub(crate) fn edit_library_photo_in(
        &mut self,
        frame: &eframe::Frame,
        photo: PathBuf,
        target: EditInTarget,
    ) {
        if target.sends_original() {
            self.open_in_editor(&target, &photo);
            return;
        }
        let directory = match self.edit_in_directory(&photo) {
            Ok(directory) => directory,
            Err(error) => {
                self.report_error(ErrorKind::EditIn, error);
                return;
            }
        };
        let settings = self.preferences.edit_in.export_settings();
        let stem = edit_in_stem(&photo);
        self.start_library_export_jobs(
            vec![(
                photo,
                LibraryExportDestination::InFolder {
                    folder: directory,
                    stem,
                },
            )],
            LibraryExportSettings {
                chosen: settings.clone(),
                rendered: settings,
            },
            self.preferences.edit_in.format.export_format(),
            ExportOverride::default(),
            Some(target),
            frame,
        );
    }

    /// Opens the saved `file` in `target`'s editor and says so.
    pub(in crate::app) fn open_in_editor(&mut self, target: &EditInTarget, file: &Path) {
        let file_name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.display().to_string());
        let (program, editor) = match target {
            EditInTarget::Editor(editor) => (Some(editor.program.as_path()), Some(&editor.name)),
            EditInTarget::SystemChooser => (None, None),
        };
        match self.platform.open_in_editor(program, file) {
            Ok(()) => {
                self.ui.notice = Some(match editor {
                    Some(editor) => tr_fmt(
                        "Opened {file} in {editor}",
                        &[("file", &file_name), ("editor", editor)],
                    ),
                    None => tr_fmt("Opened {file}", &[("file", &file_name)]),
                });
            }
            Err(error) => self.report_error(
                ErrorKind::EditIn,
                tr_fmt(
                    "Could not open {file}: {error}",
                    &[("file", &file_name), ("error", &error)],
                ),
            ),
        }
    }

    /// The folder Edit In saves `photo`'s copy in, created when it is the
    /// app's cache folder.
    fn edit_in_directory(&self, photo: &Path) -> Result<PathBuf, String> {
        match &self.preferences.edit_in.folder {
            EditInFolder::AppCache => {
                let folder = self.platform.edit_in_cache_dir().ok_or_else(|| {
                    tr_fmt(
                        "The Edit In folder {folder} is not available.",
                        &[("folder", &tr("App cache"))],
                    )
                })?;
                std::fs::create_dir_all(&folder).map_err(|error| {
                    tr_fmt(
                        "Could not create the Edit In folder {folder}: {error}",
                        &[("folder", &folder.display()), ("error", &error)],
                    )
                })?;
                Ok(folder)
            }
            EditInFolder::PhotoFolder => photo
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(Path::to_path_buf)
                .ok_or_else(|| {
                    tr_fmt(
                        "The Edit In folder {folder} is not available.",
                        &[("folder", &photo.display())],
                    )
                }),
            EditInFolder::Folder(folder) if folder.is_dir() => Ok(folder.clone()),
            EditInFolder::Folder(folder) => Err(tr_fmt(
                "The Edit In folder {folder} is not available.",
                &[("folder", &folder.display())],
            )),
        }
    }

    // --- Settings ---

    pub(crate) fn set_edit_in_folder(&mut self, folder: EditInFolder) {
        if self.preferences.edit_in.folder != folder {
            self.preferences.edit_in.folder = folder;
            self.persist_performance_settings();
        }
    }

    /// Asks for the folder Edit In saves to; it applies once chosen.
    pub(crate) fn choose_edit_in_folder(&mut self) {
        let current = match &self.preferences.edit_in.folder {
            EditInFolder::Folder(folder) => Some(folder.clone()),
            EditInFolder::AppCache | EditInFolder::PhotoFolder => None,
        };
        self.choose_settings_folder(current, DesktopPickerEvent::EditInFolder);
    }

    pub(crate) fn set_edit_in_format(&mut self, format: EditInFormat) {
        if self.preferences.edit_in.format != format {
            self.preferences.edit_in.format = format;
            self.persist_performance_settings();
        }
    }

    pub(crate) fn set_edit_in_color_space(&mut self, color_space: OutputColorSpace) {
        if self.preferences.edit_in.color_space != color_space {
            self.preferences.edit_in.color_space = color_space;
            self.persist_performance_settings();
        }
    }

    /// Adds `editor` at the end of the list, unless its program is listed.
    pub(crate) fn add_editor(&mut self, editor: ExternalEditor) {
        let editors = &mut self.preferences.edit_in.editors;
        if editors
            .iter()
            .any(|listed| listed.program == editor.program)
        {
            return;
        }
        self.ui.notice = Some(tr_fmt("Added {editor}", &[("editor", &editor.name)]));
        editors.push(editor);
        self.persist_performance_settings();
    }

    pub(crate) fn remove_editor(&mut self, index: usize) {
        if index < self.preferences.edit_in.editors.len() {
            self.preferences.edit_in.editors.remove(index);
            self.persist_performance_settings();
        }
    }

    pub(crate) fn set_editor_sends_original(&mut self, index: usize, send_original: bool) {
        let Some(editor) = self.preferences.edit_in.editors.get_mut(index) else {
            return;
        };
        if editor.send_original != send_original {
            editor.send_original = send_original;
            self.persist_performance_settings();
        }
    }

    /// Asks for a program to add as an editor.
    pub(crate) fn choose_editor_program(&mut self) {
        if self.ui.desktop_picker_receiver.is_some() {
            return;
        }
        let dialog = self.platform.editor_program_dialog();
        self.ui.desktop_picker_receiver = Some(spawn_ui_worker(&self.egui_ctx, move || {
            DesktopPickerEvent::EditorProgram(dialog.pick_file_on_worker())
        }));
    }

    pub(in crate::app) fn add_editor_program(&mut self, program: &Path) {
        match self.platform.editor_for_program(program) {
            Some(editor) => self.add_editor(editor),
            None => self.report_error(
                ErrorKind::EditIn,
                tr_fmt(
                    "{program} is not an application that can open images.",
                    &[("program", &program.display())],
                ),
            ),
        }
    }

    /// The editors installed here, once found; the first call starts looking.
    pub(crate) fn installed_editors(&mut self) -> Option<&[ExternalEditor]> {
        if self.ui.installed_editors.is_none() {
            match self
                .ui
                .installed_editors_receiver
                .as_ref()
                .map(mpsc::Receiver::try_recv)
            {
                None => {
                    let platform = Arc::clone(&self.platform);
                    self.ui.installed_editors_receiver =
                        Some(spawn_ui_worker(&self.egui_ctx, move || {
                            platform.installed_editors()
                        }));
                }
                Some(Ok(editors)) => {
                    self.ui.installed_editors = Some(editors);
                    self.ui.installed_editors_receiver = None;
                }
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    self.ui.installed_editors = Some(Vec::new());
                    self.ui.installed_editors_receiver = None;
                }
                Some(Err(mpsc::TryRecvError::Empty)) => {}
            }
        }
        self.ui.installed_editors.as_deref()
    }
}

/// The copy's name: the photo's, so the editor shows and saves it under a name
/// the person knows. `NewFile` numbers it when that name is taken.
fn edit_in_stem(photo: &Path) -> String {
    photo
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "calibraw-edit".to_owned())
}

/// Deletes the files in the app-cache Edit In folder `folder` that have not
/// changed for [`CACHE_FILE_LIFETIME`], on a thread of its own. A file an
/// editor still holds open (Windows) is left for the next start.
pub(in crate::app) fn spawn_edit_in_cache_cleanup(folder: PathBuf) {
    let result = std::thread::Builder::new()
        .name("calibraw-edit-in-cleanup".to_owned())
        .spawn(move || {
            let Ok(entries) = std::fs::read_dir(&folder) else {
                return;
            };
            let now = std::time::SystemTime::now();
            for entry in entries.flatten() {
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                let expired = metadata.is_file()
                    && metadata
                        .modified()
                        .ok()
                        .and_then(|modified| now.duration_since(modified).ok())
                        .is_some_and(|age| age >= CACHE_FILE_LIFETIME);
                if expired {
                    if let Err(error) = std::fs::remove_file(entry.path()) {
                        log::info!("kept the Edit In file {}: {error}", entry.path().display());
                    }
                }
            }
        });
    if let Err(error) = result {
        log::warn!("could not start the Edit In cleanup: {error}");
    }
}
