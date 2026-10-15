use super::*;
use crate::app::ErrorKind;
use crate::ui::background_tasks::{BackgroundTaskId, BackgroundTaskView};
use anyhow::{Context, Result};
use calibraw_gpu::pipeline::merge_hdr_bracket;

pub(super) struct HdrMergeTask {
    receiver: mpsc::Receiver<Result<PathBuf, String>>,
    progress: Arc<Mutex<String>>,
    cancelled: Arc<AtomicBool>,
}

pub(super) fn start(
    app: &mut CalibRawApp,
    frame: &eframe::Frame,
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
        app.report_error(ErrorKind::HdrMerge, "HDR merge requires GPU rendering.");
        return;
    };
    let paths: Vec<_> = assets
        .into_iter()
        .map(|asset| {
            let LibraryLocator::Desktop(path) = asset.locator;
            path
        })
        .collect();
    let device = render_state.device.clone();
    let queue = render_state.queue.clone();
    let prewarm = app.export.gpu_prewarm.clone();
    let (sender, receiver) = mpsc::channel();
    let progress = Arc::new(Mutex::new("HDR: preparing bracket…".to_owned()));
    let cancelled = Arc::new(AtomicBool::new(false));
    let task = HdrMergeTask {
        receiver,
        progress: Arc::clone(&progress),
        cancelled: Arc::clone(&cancelled),
    };
    let repaint = context.clone();
    let worker = std::thread::Builder::new()
        .name("hdr-merge".to_owned())
        .spawn(move || {
            let result = (|| -> Result<PathBuf> {
                let merged = merge_hdr_bracket(&device, &queue, &paths, prewarm, |message| {
                    if let Ok(mut status) = progress.lock() {
                        *status = message;
                    }
                    repaint.request_repaint();
                    !cancelled.load(Ordering::Relaxed)
                })?;
                anyhow::ensure!(!cancelled.load(Ordering::Relaxed), "HDR merge cancelled");
                if let Ok(mut status) = progress.lock() {
                    *status = "HDR: saving floating-point TIFF…".to_owned();
                }
                repaint.request_repaint();
                let parent = paths[0]
                    .parent()
                    .context("HDR source has no parent folder")?;
                let mut temporary = tempfile::Builder::new()
                    .prefix(".calibraw-hdr-")
                    .suffix(".tmp")
                    .tempfile_in(parent)?;
                merged.write_tiff(temporary.as_file_mut())?;
                temporary.as_file().sync_all()?;
                anyhow::ensure!(!cancelled.load(Ordering::Relaxed), "HDR merge cancelled");
                save_unique(temporary, &paths[0])
            })()
            .map_err(|error| format!("{error:#}"));
            let _ = sender.send(result);
            repaint.request_repaint();
        });
    match worker {
        Ok(_) => app.library.hdr_merge = Some(task),
        Err(error) => app.report_error(
            ErrorKind::HdrMerge,
            format!("Could not start HDR merge: {error}"),
        ),
    }
}

fn save_unique(mut temporary: tempfile::NamedTempFile, source: &Path) -> Result<PathBuf> {
    let parent = source.parent().context("HDR source has no parent folder")?;
    let stem = source.file_stem().context("HDR source has no file name")?;
    for number in 0..10_000 {
        let mut name = stem.to_os_string();
        name.push(if number == 0 {
            "-HDR.tif".to_owned()
        } else {
            format!("-HDR-{number}.tif")
        });
        let path = parent.join(name);
        match temporary.persist_noclobber(&path) {
            Ok(_) => return Ok(path),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                temporary = error.file
            }
            Err(error) => return Err(error.error).context("save HDR master"),
        }
    }
    anyhow::bail!("Could not find an unused HDR output name")
}

/// How an HDR merge ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum HdrMergeOutcome {
    Saved(PathBuf),
    Cancelled,
    Failed(String),
}

impl LibraryState {
    /// Takes the result of a finished merge. A saved merge also refreshes the
    /// library so the new TIFF appears.
    pub(crate) fn poll_hdr_merge(&mut self, context: &egui::Context) -> Option<HdrMergeOutcome> {
        let task = self.hdr_merge.as_ref()?;
        let cancelled = task.cancelled.load(Ordering::Relaxed);
        let outcome = match task.receiver.try_recv() {
            Ok(Ok(path)) => HdrMergeOutcome::Saved(path),
            Ok(Err(_)) if cancelled => HdrMergeOutcome::Cancelled,
            Ok(Err(error)) => HdrMergeOutcome::Failed(error),
            Err(mpsc::TryRecvError::Disconnected) => {
                HdrMergeOutcome::Failed("HDR merge stopped unexpectedly.".to_owned())
            }
            Err(mpsc::TryRecvError::Empty) => return None,
        };
        self.hdr_merge = None;
        if matches!(outcome, HdrMergeOutcome::Saved(_)) {
            self.refresh(context);
        }
        Some(outcome)
    }

    /// The running merge for the toolbar task indicator.
    pub(crate) fn hdr_merge_task(&self) -> Option<BackgroundTaskView> {
        let task = self.hdr_merge.as_ref()?;
        let detail = task
            .progress
            .lock()
            .map(|progress| {
                let step = progress.strip_prefix("HDR: ").unwrap_or(&progress);
                let mut chars = step.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().chain(chars).collect())
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        Some(BackgroundTaskView {
            id: BackgroundTaskId::HdrMerge,
            title: "HDR merge".to_owned(),
            detail,
            progress: None,
            cancelling: task.cancelled.load(Ordering::Relaxed),
        })
    }

    pub(crate) fn cancel_hdr_merge(&self) {
        if let Some(task) = &self.hdr_merge {
            task.cancelled.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saves_without_overwriting_existing_master() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("bracket-HDR.tif");
        fs::write(&existing, b"original").unwrap();
        let temp = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        fs::write(temp.path(), b"merged").unwrap();
        let output = save_unique(temp, &dir.path().join("bracket.dng")).unwrap();
        assert_eq!(output, dir.path().join("bracket-HDR-1.tif"));
        assert_eq!(fs::read(existing).unwrap(), b"original");
        assert_eq!(fs::read(output).unwrap(), b"merged");
    }
    #[test]
    fn hdr_master_reopens_in_the_editor_without_clipping() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("test-HDR.tif");
        let master = calibraw_gpu::pipeline::HdrMergeResult {
            width: 3,
            height: 1,
            rgb: vec![4.0, 2.0, 0.5, -0.01, 0.0001, 16.0, 0.0001, 0.0001, 0.0001],
        };
        master
            .write_tiff(fs::File::create(&output).unwrap())
            .unwrap();
        let raw = crate::pipeline::load_raw_file(&output).unwrap();
        let loaded = raw.scene_linear_raster().unwrap();
        for (pixel, expected) in loaded.chunks_exact(3).zip(master.rgb.chunks_exact(3)) {
            // The ICC round trip adapts through D50 and uses fixed-point matrix tags.
            let tolerance =
                expected.iter().copied().map(f32::abs).fold(0.0, f32::max) * 0.0005 + 1e-8;
            for (actual, expected) in pixel.iter().zip(expected) {
                assert!(
                    (actual - expected).abs() < tolerance,
                    "{actual} != {expected}"
                );
                if *expected < 0.0 {
                    assert!(*actual < 0.0);
                }
            }
        }
    }
}
