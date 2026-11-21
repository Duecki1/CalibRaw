use super::*;
use crate::app::{AiConsentAnswer, AiConsentCheck, AiConsentRequester, ErrorKind};
use crate::i18n::{tr, tr_fmt};
use crate::pipeline::LoadedRaw;
use crate::services::ai_denoise::{install_ai_denoise, AiDenoiseStatus};
use crate::ui::background_tasks::{BackgroundTaskId, BackgroundTaskView};
use anyhow::{Context, Result};
use calibraw_ai::{AiDownloads, AiFeature};
use calibraw_gpu::pipeline::panorama::ProjectionChoice;
use calibraw_gpu::pipeline::{
    merge_hdr_bracket, merge_panorama, stack_focus, MergeOptions, PanoramaOptions, SourceDenoiser,
};
use std::fs::File;

/// Library actions that merge several RAW files into one floating-point TIFF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MergeKind {
    Hdr,
    FocusStack,
    Panorama,
}

impl MergeKind {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Hdr => tr("HDR merge"),
            Self::FocusStack => tr("Focus stack"),
            Self::Panorama => tr("Panorama"),
        }
    }

    pub(crate) fn error_kind(self) -> ErrorKind {
        match self {
            Self::Hdr => ErrorKind::HdrMerge,
            Self::FocusStack => ErrorKind::FocusStack,
            Self::Panorama => ErrorKind::Panorama,
        }
    }

    /// Appended to the first source's file stem.
    fn file_suffix(self) -> &'static str {
        match self {
            Self::Hdr => "HDR",
            Self::FocusStack => "FocusStack",
            Self::Panorama => "Pano",
        }
    }
}

/// What the merge dialog chose. The last confirmed choice opens the dialog
/// again until CalibRaw quits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct MergeChoices {
    options: MergeOptions,
    panorama: PanoramaOptions,
    /// AI-denoise every source with RawNIND before it develops.
    ai_denoise: bool,
}

/// Width of the panorama projection choice.
const PROJECTION_WIDTH: f32 = 340.0;

/// Whether a merge AI-denoises its sources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MergeAiDenoise {
    Off,
    /// `downloads` is what the user accepted to download in the consent
    /// dialog: the RawNIND models and the ONNX Runtime.
    On {
        downloads: AiDownloads,
    },
}

/// A merge the dialog confirmed.
struct MergeRequest {
    kind: MergeKind,
    options: MergeOptions,
    panorama: PanoramaOptions,
    ai_denoise: MergeAiDenoise,
    /// Open the result in Develop once it is saved.
    open: bool,
    assets: Vec<LibraryAsset>,
}

/// The options dialog a merge asks before it starts.
pub(super) struct MergeDialog {
    kind: MergeKind,
    assets: Vec<LibraryAsset>,
    /// The confirmed merge waits for the AI consent dialog, which asks before
    /// the models AI denoise needs are downloaded; `open` as confirmed.
    awaiting_consent: Option<bool>,
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
pub(super) fn ask(app: &mut CalibRawApp, kind: MergeKind, assets: Vec<LibraryAsset>) {
    if assets.len() >= 2 {
        app.library.merge_dialog = Some(MergeDialog {
            kind,
            assets,
            awaiting_consent: None,
        });
    }
}

/// The open merge dialog: one switch per option, each starting from the
/// choice of the previous merge. A merge with AI denoise first passes the AI
/// consent dialog, which asks when RawNIND or the ONNX Runtime must be
/// downloaded; declining it returns to this dialog.
pub(super) fn show_dialog(ui: &mut Ui, app: &mut CalibRawApp, frame: &eframe::Frame) {
    let Some(dialog) = &app.library.merge_dialog else {
        return;
    };
    if let Some(open) = dialog.awaiting_consent {
        match app.take_ai_consent_answer(AiConsentRequester::Merge) {
            Some(AiConsentAnswer::Accepted { downloads }) => {
                confirm(app, frame, open, MergeAiDenoise::On { downloads }, ui.ctx());
                return;
            }
            Some(AiConsentAnswer::Declined) => {
                if let Some(dialog) = &mut app.library.merge_dialog {
                    dialog.awaiting_consent = None;
                }
            }
            None => {}
        }
    }
    let Some(dialog) = &app.library.merge_dialog else {
        return;
    };
    let kind = dialog.kind;
    let choices = &mut app.library.merge_choices;
    let mut choice = None;
    moduwu_design::Dialog::new("library-merge-options", kind.title()).show_with_footer(
        ui.ctx(),
        |ui| {
            let options = &mut choices.options;
            moduwu_design::toggle_row(ui, &mut options.lens_correction, tr("Lens correction"));
            // A panorama is always aligned: its frames are taken from different directions.
            if kind == MergeKind::Panorama {
                moduwu_design::form_row(ui, tr("Projection"), PROJECTION_WIDTH, |ui, width| {
                    moduwu_design::segmented_control(
                        ui,
                        &mut choices.panorama.projection,
                        &[
                            (ProjectionChoice::Auto, tr("Auto")),
                            (ProjectionChoice::Spherical, tr("Spherical")),
                            (ProjectionChoice::Cylindrical, tr("Cylindrical")),
                            (ProjectionChoice::Perspective, tr("Perspective")),
                        ],
                        width,
                    );
                });
            } else {
                moduwu_design::toggle_row(ui, &mut options.align, tr("Auto-align"));
            }
            if kind == MergeKind::Hdr {
                moduwu_design::toggle_with_help(
                    ui,
                    &mut options.reject_motion,
                    tr("Deghost"),
                    tr("Leaves out a frame where something moved between the shots."),
                );
            }
            moduwu_design::toggle_row(ui, &mut options.remove_hot_pixels, tr("Remove hot pixels"));
            moduwu_design::toggle_row(ui, &mut choices.ai_denoise, tr("AI denoise"));
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
    match choice {
        None => {}
        Some(MergeDialogChoice::Cancel) => app.library.merge_dialog = None,
        Some(MergeDialogChoice::Merge { open }) if !app.library.merge_choices.ai_denoise => {
            confirm(app, frame, open, MergeAiDenoise::Off, ui.ctx());
        }
        Some(MergeDialogChoice::Merge { open }) => {
            match app.request_ai_consent(AiFeature::Denoise, AiConsentRequester::Merge) {
                AiConsentCheck::Ready => {
                    let ai_denoise = MergeAiDenoise::On {
                        downloads: AiDownloads::NONE,
                    };
                    confirm(app, frame, open, ai_denoise, ui.ctx());
                }
                AiConsentCheck::Asking => {
                    if let Some(dialog) = &mut app.library.merge_dialog {
                        dialog.awaiting_consent = Some(open);
                    }
                }
                // Reported; the dialog stays open to turn AI denoise off.
                AiConsentCheck::Unavailable => {}
            }
        }
    }
}

/// Closes the dialog and starts its merge with the chosen options.
fn confirm(
    app: &mut CalibRawApp,
    frame: &eframe::Frame,
    open: bool,
    ai_denoise: MergeAiDenoise,
    context: &egui::Context,
) {
    let Some(dialog) = app.library.merge_dialog.take() else {
        return;
    };
    let request = MergeRequest {
        kind: dialog.kind,
        options: app.library.merge_choices.options,
        panorama: app.library.merge_choices.panorama,
        ai_denoise,
        open,
        assets: dialog.assets,
    };
    start(app, frame, request, context);
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
    /// A panorama was reduced below its frames' resolution to stay within the
    /// largest photo CalibRaw opens.
    pub(crate) reduced: bool,
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
    request: MergeRequest,
    context: &egui::Context,
) {
    let MergeRequest {
        kind,
        options,
        panorama,
        ai_denoise,
        open,
        assets,
    } = request;
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
            let downloads = match ai_denoise {
                MergeAiDenoise::On { downloads } => downloads,
                MergeAiDenoise::Off => AiDownloads::NONE,
            };
            let mut denoise_source = |raw: &Arc<LoadedRaw>| -> Result<()> {
                install_ai_denoise(
                    raw,
                    None,
                    downloads,
                    &device,
                    &queue,
                    &cancelled,
                    |status| match status {
                        AiDenoiseStatus::Downloading(download) => {
                            report(format!(
                                "{} ({:.1} / {:.1} MB)",
                                tr("Downloading verified RawNIND model package"),
                                download.downloaded as f64 / 1_000_000.0,
                                download.total as f64 / 1_000_000.0,
                            ));
                        }
                        AiDenoiseStatus::Denoising { completed, total } if total > 0 => {
                            report(tr_fmt(
                                "Applying AI denoise: tile {completed}/{total}",
                                &[("completed", &completed), ("total", &total)],
                            ));
                        }
                        AiDenoiseStatus::Denoising { .. } => {}
                    },
                )
                .map_err(anyhow::Error::msg)
            };
            let denoise = (ai_denoise != MergeAiDenoise::Off)
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
                let saving = || report(tr("Saving floating-point TIFF…").to_owned());
                let (name, locator, lens_corrected, reduced) = match kind {
                    MergeKind::Hdr | MergeKind::FocusStack => {
                        let merged = if kind == MergeKind::Hdr {
                            merge_hdr_bracket(
                                &device, &queue, &paths, options, denoise, prewarm, report,
                            )
                        } else {
                            stack_focus(&device, &queue, &paths, options, denoise, prewarm, report)
                        }?;
                        anyhow::ensure!(!cancelled.load(Ordering::Relaxed), tr("Merge cancelled"));
                        saving();
                        let (name, locator) = save_merged(
                            |file| merged.write_tiff(file),
                            &assets[0],
                            &paths[0],
                            kind,
                            &*platform,
                            &cancelled,
                        )?;
                        (name, locator, merged.lens_corrected, false)
                    }
                    MergeKind::Panorama => {
                        let staging = paths[0]
                            .parent()
                            .context(tr("Merge source has no parent folder"))?
                            .to_path_buf();
                        let master = merge_panorama(
                            &device, &queue, &paths, options, panorama, denoise, prewarm, &staging,
                            report,
                        )?;
                        anyhow::ensure!(!cancelled.load(Ordering::Relaxed), tr("Merge cancelled"));
                        saving();
                        // Compositing runs while the file is written.
                        let (name, locator) = save_merged(
                            |file| master.write_tiff(file, report),
                            &assets[0],
                            &paths[0],
                            kind,
                            &*platform,
                            &cancelled,
                        )?;
                        (name, locator, master.lens_corrected(), master.reduced())
                    }
                };
                Ok(SavedMerge {
                    name,
                    locator,
                    open,
                    lens_uncorrected: options.lens_correction && !lens_corrected,
                    reduced,
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

/// Saves the TIFF `write` produces as `{stem}-{suffix}.tif` for the first
/// `source`, staged at `staged`: beside a folder photo, or into the media library
/// for a media-library photo. Returns the saved file name and where it is.
fn save_merged(
    write: impl FnOnce(&mut File) -> Result<()>,
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
    write(temporary.as_file_mut())?;
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
