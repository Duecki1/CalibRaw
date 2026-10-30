use super::*;
use crate::app::ErrorKind;
use crate::i18n::{tr, tr_fmt};
use crate::ui::background_tasks::{BackgroundTaskId, BackgroundTaskView};
use anyhow::{Context, Result};
use calibraw_gpu::pipeline::{merge_hdr_bracket, stack_focus, GpuProgramPrewarm, MergedImage};
use eframe::wgpu;

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

    fn run(
        self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        paths: &[PathBuf],
        prewarm: Option<Arc<GpuProgramPrewarm>>,
        progress: impl FnMut(String) -> bool,
    ) -> Result<MergedImage> {
        match self {
            Self::Hdr => merge_hdr_bracket(device, queue, paths, prewarm, progress),
            Self::FocusStack => stack_focus(device, queue, paths, prewarm, progress),
        }
    }
}

pub(super) struct MergeTask {
    kind: MergeKind,
    /// The saved file name, or the error.
    receiver: mpsc::Receiver<Result<String, String>>,
    progress: Arc<Mutex<String>>,
    cancelled: Arc<AtomicBool>,
}

pub(super) fn start(
    app: &mut CalibRawApp,
    frame: &eframe::Frame,
    kind: MergeKind,
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
            // The merge reads files: media-library photos are staged as local copies.
            let mut staged = Vec::with_capacity(assets.len());
            let result = (|| -> Result<String> {
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
                let merged = kind.run(&device, &queue, &paths, prewarm, report)?;
                anyhow::ensure!(!cancelled.load(Ordering::Relaxed), tr("Merge cancelled"));
                report(tr("Saving floating-point TIFF…").to_owned());
                save_merged(&merged, &assets[0], &paths[0], kind, &*platform, &cancelled)
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
/// photo. Returns the saved file name.
fn save_merged(
    merged: &MergedImage,
    source: &LibraryAsset,
    staged: &Path,
    kind: MergeKind,
    platform: &dyn Platform,
    cancelled: &AtomicBool,
) -> Result<String> {
    let parent = staged
        .parent()
        .context(tr("Merge source has no parent folder"))?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".calibraw-merge-")
        .suffix(".tmp")
        .tempfile_in(parent)?;
    merged.write_tiff(temporary.as_file_mut())?;
    temporary.as_file().sync_all()?;
    anyhow::ensure!(!cancelled.load(Ordering::Relaxed), tr("Merge cancelled"));
    match &source.locator {
        LibraryLocator::Desktop(path) => {
            let saved = save_unique(temporary, path, kind.file_suffix())?;
            Ok(saved
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| saved.display().to_string()))
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
            Ok(imported.display_name)
        }
    }
}

/// Moves `temporary` to `{stem}-{suffix}.tif` beside `source`, or the first free
/// `{stem}-{suffix}-N.tif`, never replacing a file.
fn save_unique(
    mut temporary: tempfile::NamedTempFile,
    source: &Path,
    suffix: &str,
) -> Result<PathBuf> {
    let parent = source
        .parent()
        .context(tr("Merge source has no parent folder"))?;
    let stem = source
        .file_stem()
        .context(tr("Merge source has no file name"))?;
    for number in 0..10_000 {
        let mut name = stem.to_os_string();
        name.push(if number == 0 {
            format!("-{}.tif", suffix)
        } else {
            format!("-{}-{}.tif", suffix, number)
        });
        let path = parent.join(name);
        match temporary.persist_noclobber(&path) {
            Ok(_) => return Ok(path),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                temporary = error.file
            }
            Err(error) => return Err(error.error).context(tr("save merged master")),
        }
    }
    anyhow::bail!(tr("Could not find an unused output name"))
}

/// How a merge ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MergeOutcome {
    /// The saved file name.
    Saved(String),
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
            Ok(Ok(name)) => MergeOutcome::Saved(name),
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
