use super::*;
use crate::app::ErrorKind;
use crate::i18n::{tr, tr_fmt};
use crate::pipeline::LoadedRaw;
use crate::services::ai_denoise::{ai_denoise_models_installed, install_ai_denoise};
use crate::ui::background_tasks::{BackgroundTaskId, BackgroundTaskView};
use anyhow::{Context, Result};
use calibraw_gpu::pipeline::{
    merge_hdr_bracket, stack_focus, MergeOptions, MergedImage, SourceDenoiser,
};

/// Library actions that merge several RAW files into one floating-point TIFF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MergeKind {
    Hdr,
    FocusStack,
}

impl MergeKind {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Hdr => tr("HDR merge"),
            Self::FocusStack => tr("Focus stack"),
        }
    }

    pub(crate) fn error_kind(self) -> ErrorKind {
        match self {
            Self::Hdr => ErrorKind::HdrMerge,
            Self::FocusStack => ErrorKind::FocusStack,
        }
    }

    /// Appended to the first source's file stem.
    fn file_suffix(self) -> &'static str {
        match self {
            Self::Hdr => "HDR",
            Self::FocusStack => "FocusStack",
        }
    }
}

/// What the merge dialog chose. The last confirmed choice opens the dialog
/// again until CalibRaw quits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct MergeChoices {
    options: MergeOptions,
    /// AI-denoise every source with RawNIND before it develops.
    ai_denoise: bool,
}

/// The options dialog a merge asks before it starts.
pub(super) struct MergeDialog {
    kind: MergeKind,
    assets: Vec<LibraryAsset>,
    ai_models: AiModels,
}

/// Whether the RawNIND models AI denoise needs are installed, which takes a
/// worker seconds to check ([`ai_denoise_models_installed`]).
enum AiModels {
    Checking(mpsc::Receiver<bool>),
    Installed,
    Missing,
}

impl AiModels {
    fn check(context: &egui::Context) -> Self {
        let (sender, receiver) = mpsc::channel();
        let repaint = context.clone();
        let spawned = std::thread::Builder::new()
            .name("raw-merge-ai-models".to_owned())
            .spawn(move || {
                let _ = sender.send(ai_denoise_models_installed());
                repaint.request_repaint();
            });
        match spawned {
            Ok(_) => Self::Checking(receiver),
            Err(error) => {
                log::warn!("could not check the RawNIND models: {error}");
                Self::Missing
            }
        }
    }

    fn poll(&mut self) {
        if let Self::Checking(receiver) = self {
            *self = match receiver.try_recv() {
                Ok(true) => Self::Installed,
                Ok(false) | Err(mpsc::TryRecvError::Disconnected) => Self::Missing,
                Err(mpsc::TryRecvError::Empty) => return,
            };
        }
    }
}

/// How the merge dialog was closed.
enum MergeDialogChoice {
    Cancel,
    /// Start the merge, and open its result in Develop once it is saved when
    /// `open` is set.
    Merge {
        open: bool,
    },
}

/// Asks for the options of a `kind` merge of `assets`; [`show_dialog`] starts
/// it once they are confirmed.
pub(super) fn ask(
    app: &mut CalibRawApp,
    kind: MergeKind,
    assets: Vec<LibraryAsset>,
    context: &egui::Context,
) {
    if assets.len() >= 2 {
        app.library.merge_dialog = Some(MergeDialog {
            kind,
            assets,
            ai_models: AiModels::check(context),
        });
    }
}

/// The open merge dialog: one switch per option, each starting from the
/// choice of the previous merge. AI denoise can be chosen once its models are
/// known to be installed; turning it on in Develop installs them.
pub(super) fn show_dialog(ui: &mut Ui, app: &mut CalibRawApp, frame: &eframe::Frame) {
    let Some(dialog) = &mut app.library.merge_dialog else {
        return;
    };
    dialog.ai_models.poll();
    let kind = dialog.kind;
    let models = &dialog.ai_models;
    let choices = &mut app.library.merge_choices;
    let mut choice = None;
    moduwu_design::Dialog::new("library-merge-options", kind.title()).show_with_footer(
        ui.ctx(),
        |ui| {
            let options = &mut choices.options;
            moduwu_design::toggle_row(ui, &mut options.lens_correction, tr("Lens correction"));
            moduwu_design::toggle_row(ui, &mut options.align, tr("Auto-align"));
            if kind == MergeKind::Hdr {
                moduwu_design::toggle_with_help(
                    ui,
                    &mut options.reject_motion,
                    tr("Deghost"),
                    tr("Leaves out a frame where something moved between the shots."),
                );
            }
            moduwu_design::toggle_row(ui, &mut options.remove_hot_pixels, tr("Remove hot pixels"));
            let installed = matches!(models, AiModels::Installed);
            // Shown off while it cannot run, without forgetting the choice.
            let mut ai_denoise = choices.ai_denoise && installed;
            let mut toggle = ui
                .add_enabled_ui(installed, |ui| {
                    moduwu_design::toggle_row(ui, &mut ai_denoise, tr("AI denoise"))
                })
                .inner;
            if matches!(models, AiModels::Missing) {
                toggle = toggle.on_disabled_hover_text(tr(
                    "Turn on AI denoise for a photo in Develop once to install its models.",
                ));
            }
            if toggle.changed() {
                choices.ai_denoise = ai_denoise;
            }
        },
        |ui| {
            if moduwu_design::primary_action_button(ui, tr("Merge")).clicked() {
                choice = Some(MergeDialogChoice::Merge { open: false });
            }
            if moduwu_design::secondary_button(ui, tr("Merge and open")).clicked() {
                choice = Some(MergeDialogChoice::Merge { open: true });
            }
            if moduwu_design::secondary_button(ui, tr("Cancel")).clicked() {
                choice = Some(MergeDialogChoice::Cancel);
            }
            // Enter also flips a focused switch, so it does not confirm.
            if choice.is_none()
                && moduwu_design::dialog_keyboard_action(
                    ui,
                    moduwu_design::DialogKeyboard::CLOSE_ONLY,
                    false,
                ) == moduwu_design::DialogAction::Cancel
            {
                choice = Some(MergeDialogChoice::Cancel);
            }
        },
    );
    let Some(choice) = choice else {
        return;
    };
    let Some(dialog) = app.library.merge_dialog.take() else {
        return;
    };
    if let MergeDialogChoice::Merge { open } = choice {
        let mut choices = app.library.merge_choices;
        // Still checking: the models were installed when AI denoise was last
        // chosen, and the first source fails clearly if they are gone.
        choices.ai_denoise &= !matches!(dialog.ai_models, AiModels::Missing);
        start(
            app,
            frame,
            dialog.kind,
            choices,
            open,
            dialog.assets,
            ui.ctx(),
        );
    }
}

/// A saved merge.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SavedMerge {
    /// The saved file's name.
    pub(crate) name: String,
    /// Where it was saved: beside the first source, or in the media library.
    pub(crate) locator: LibraryLocator,
    /// Open it in Develop, as the user asked.
    pub(crate) open: bool,
    /// Lens correction was asked for but no profile matched, so the merge is
    /// not lens corrected.
    pub(crate) lens_uncorrected: bool,
}

pub(super) struct MergeTask {
    kind: MergeKind,
    receiver: mpsc::Receiver<Result<SavedMerge, String>>,
    progress: Arc<Mutex<String>>,
    cancelled: Arc<AtomicBool>,
}

fn start(
    app: &mut CalibRawApp,
    frame: &eframe::Frame,
    kind: MergeKind,
    choices: MergeChoices,
    open: bool,
    assets: Vec<LibraryAsset>,
    context: &egui::Context,
) {
    if assets.len() < 2
        || local_action_in_progress(app)
        || app.library_batch_export_progress().is_some()
    {
        return;
    }
    let Some(render_state) = frame.wgpu_render_state() else {
        app.report_error(
            kind.error_kind(),
            tr_fmt(
                "{title} requires GPU rendering.",
                &[("title", &kind.title())],
            ),
        );
        return;
    };
    let device = render_state.device.clone();
    let queue = render_state.queue.clone();
    let prewarm = app.export.gpu_prewarm.clone();
    let platform = Arc::clone(&app.library.platform);
    let (sender, receiver) = mpsc::channel();
    let progress = Arc::new(Mutex::new(tr("Preparing…").to_owned()));
    let cancelled = Arc::new(AtomicBool::new(false));
    let task = MergeTask {
        kind,
        receiver,
        progress: Arc::clone(&progress),
        cancelled: Arc::clone(&cancelled),
    };
    let repaint = context.clone();
    let worker = std::thread::Builder::new()
        .name("raw-merge".to_owned())
        .spawn(move || {
            let report = |message: String| {
                if let Ok(mut status) = progress.lock() {
                    *status = message;
                }
                repaint.request_repaint();
                !cancelled.load(Ordering::Relaxed)
            };
            // The result is not saved beside the RAW: the merge corrects and
            // white balances each source its own way, so a saved result would
            // not match the photo's edit, and restoring one deletes it.
            let mut denoise_source = |raw: &Arc<LoadedRaw>| -> Result<()> {
                install_ai_denoise(
                    raw,
                    None,
                    &device,
                    &queue,
                    &cancelled,
                    |completed, total| {
                        if total > 0 {
                            report(tr_fmt(
                                "Applying AI denoise: tile {completed}/{total}",
                                &[("completed", &completed), ("total", &total)],
                            ));
                        }
                    },
                )
                .map_err(anyhow::Error::msg)
            };
            let denoise = choices
                .ai_denoise
                .then_some(&mut denoise_source as &mut SourceDenoiser);
            // The merge reads files: media-library photos are staged as local copies.
            let mut staged = Vec::with_capacity(assets.len());
            let result = (|| -> Result<SavedMerge> {
                for (index, asset) in assets.iter().enumerate() {
                    anyhow::ensure!(
                        report(tr_fmt(
                            "Reading photo {index}/{count}",
                            &[("index", &(index + 1)), ("count", &assets.len())]
                        )),
                        tr("Merge cancelled")
                    );
                    staged.push(
                        materialize_library_asset(asset, &*platform).map_err(anyhow::Error::msg)?,
                    );
                }
                let paths: Vec<_> = staged
                    .iter()
                    .map(|staged| staged.raw_path.clone())
                    .collect();
                let options = choices.options;
                let merged = match kind {
                    MergeKind::Hdr => merge_hdr_bracket(
                        &device, &queue, &paths, options, denoise, prewarm, report,
                    ),
                    MergeKind::FocusStack => {
                        stack_focus(&device, &queue, &paths, options, denoise, prewarm, report)
                    }
                }?;
                anyhow::ensure!(!cancelled.load(Ordering::Relaxed), tr("Merge cancelled"));
                report(tr("Saving floating-point TIFF…").to_owned());
                let (name, locator) =
                    save_merged(&merged, &assets[0], &paths[0], kind, &*platform, &cancelled)?;
                Ok(SavedMerge {
                    name,
                    locator,
                    open,
                    lens_uncorrected: choices.options.lens_correction && !merged.lens_corrected,
                })
            })()
            .map_err(|error| format!("{error:#}"));
            for staged in &staged {
                staged.cleanup();
            }
            let _ = sender.send(result);
            repaint.request_repaint();
        });
    match worker {
        Ok(_) => app.library.merge = Some(task),
        Err(error) => app.report_error(
            kind.error_kind(),
            tr_fmt(
                "Could not start {title}: {error}",
                &[("title", &kind.title()), ("error", &error)],
            ),
        ),
    }
}

/// Saves `merged` as `{stem}-{suffix}.tif` for the first `source`, staged at
/// `staged`: beside a folder photo, or into the media library for a media-library
/// photo. Returns the saved file name and where it is.
fn save_merged(
    merged: &MergedImage,
    source: &LibraryAsset,
    staged: &Path,
    kind: MergeKind,
    platform: &dyn Platform,
    cancelled: &AtomicBool,
) -> Result<(String, LibraryLocator)> {
    let parent = staged
        .parent()
        .context(tr("Merge source has no parent folder"))?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".calibraw-merge-")
        .suffix(".tmp")
        .tempfile_in(calibraw_core::file_ops::staging_directory(parent))?;
    merged.write_tiff(temporary.as_file_mut())?;
    calibraw_core::file_ops::sync_file(temporary.as_file())?;
    anyhow::ensure!(!cancelled.load(Ordering::Relaxed), tr("Merge cancelled"));
    match &source.locator {
        LibraryLocator::Desktop(path) => {
            let saved = save_unique(temporary, path, kind.file_suffix())?;
            let name = saved
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| saved.display().to_string());
            Ok((name, LibraryLocator::Desktop(saved)))
        }
        // The library picks a free name; the staged copy is deleted when dropped.
        LibraryLocator::Android { .. } => {
            let stem = Path::new(&source.display_name)
                .file_stem()
                .context(tr("Merge source has no file name"))?
                .to_string_lossy();
            let name = format!("{}-{}.tif", stem, kind.file_suffix());
            let imported = platform
                .import_local_library_document(temporary.path(), &name)
                .map_err(anyhow::Error::msg)?;
            Ok((
                imported.display_name,
                LibraryLocator::Android { uri: imported.uri },
            ))
        }
    }
}

/// Moves `temporary` to `{stem}-{suffix}.tif` beside `source`, or the first free
/// `{stem}-{suffix}-N.tif`, never replacing a file.
fn save_unique(temporary: tempfile::NamedTempFile, source: &Path, suffix: &str) -> Result<PathBuf> {
    let parent = source
        .parent()
        .context(tr("Merge source has no parent folder"))?;
    let stem = source
        .file_stem()
        .context(tr("Merge source has no file name"))?;
    // Closes the file before it moves. The path is still removed on drop, so a
    // failed save leaves nothing behind.
    let temporary = temporary.into_temp_path();
    for number in 0..10_000 {
        let mut name = stem.to_os_string();
        name.push(if number == 0 {
            format!("-{}.tif", suffix)
        } else {
            format!("-{}-{}.tif", suffix, number)
        });
        let path = parent.join(name);
        // Not `persist_noclobber`: its exclusive rename fails with `ENOTSUP` on
        // SMB shares on macOS, where this falls back to checking the name.
        match calibraw_core::file_ops::move_file_without_replacing(&temporary, &path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error).context(tr("save merged master")),
        }
    }
    anyhow::bail!(tr("Could not find an unused output name"))
}

#[cfg(test)]
mod save_unique_tests {
    use super::*;
    use std::io::Write as _;

    fn staged(directory: &Path, contents: &[u8]) -> tempfile::NamedTempFile {
        let mut temporary = tempfile::Builder::new()
            .prefix(".calibraw-merge-")
            .suffix(".tmp")
            .tempfile_in(directory)
            .expect("create staged merge");
        temporary.write_all(contents).expect("write staged merge");
        temporary
    }

    #[test]
    fn takes_the_first_free_name_without_replacing_earlier_merges() {
        let directory = tempfile::tempdir().expect("create directory");
        let source = directory.path().join("IMG_1.CR3");

        let first = save_unique(staged(directory.path(), b"first"), &source, "HDR")
            .expect("save first merge");
        let second = save_unique(staged(directory.path(), b"second"), &source, "HDR")
            .expect("save second merge");

        assert_eq!(first, directory.path().join("IMG_1-HDR.tif"));
        assert_eq!(second, directory.path().join("IMG_1-HDR-1.tif"));
        assert_eq!(fs::read(&first).expect("read first merge"), b"first");
        assert_eq!(fs::read(&second).expect("read second merge"), b"second");
    }

    #[test]
    fn leaves_no_staging_file_behind() {
        let directory = tempfile::tempdir().expect("create directory");
        let source = directory.path().join("IMG_1.CR3");

        save_unique(staged(directory.path(), b"merged"), &source, "FocusStack")
            .expect("save merge");

        let names: Vec<_> = fs::read_dir(directory.path())
            .expect("list directory")
            .map(|entry| entry.expect("read entry").file_name())
            .collect();
        assert_eq!(names, ["IMG_1-FocusStack.tif"]);
    }
}

/// How a merge ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MergeOutcome {
    Saved(SavedMerge),
    Cancelled,
    Failed(String),
}

impl LibraryState {
    /// Whether an HDR merge or focus stack is running. The caller decides
    /// what a running merge means; its detail lives in [`Self::merge_task`].
    pub(crate) fn merge_running(&self) -> bool {
        self.merge.is_some()
    }

    /// Takes the result of a finished merge. A saved merge also refreshes the
    /// library so the new TIFF appears.
    pub(crate) fn poll_merge(
        &mut self,
        context: &egui::Context,
    ) -> Option<(MergeKind, MergeOutcome)> {
        let task = self.merge.as_ref()?;
        let cancelled = task.cancelled.load(Ordering::Relaxed);
        let outcome = match task.receiver.try_recv() {
            Ok(Ok(saved)) => MergeOutcome::Saved(saved),
            Ok(Err(_)) if cancelled => MergeOutcome::Cancelled,
            Ok(Err(error)) => MergeOutcome::Failed(error),
            Err(mpsc::TryRecvError::Disconnected) => MergeOutcome::Failed(tr_fmt(
                "{title} stopped unexpectedly.",
                &[("title", &task.kind.title())],
            )),
            Err(mpsc::TryRecvError::Empty) => return None,
        };
        let kind = task.kind;
        self.merge = None;
        if matches!(outcome, MergeOutcome::Saved(_)) {
            self.refresh(context);
        }
        Some((kind, outcome))
    }

    /// The running merge for the toolbar task indicator.
    pub(crate) fn merge_task(&self) -> Option<BackgroundTaskView> {
        let task = self.merge.as_ref()?;
        let detail = task
            .progress
            .lock()
            .map(|progress| progress.clone())
            .unwrap_or_default();
        Some(BackgroundTaskView {
            id: BackgroundTaskId::Merge,
            title: task.kind.title().to_owned(),
            detail,
            progress: None,
            cancelling: task.cancelled.load(Ordering::Relaxed),
        })
    }

    pub(crate) fn cancel_merge(&self) {
        if let Some(task) = &self.merge {
            task.cancelled.store(true, Ordering::Relaxed);
        }
    }
}
