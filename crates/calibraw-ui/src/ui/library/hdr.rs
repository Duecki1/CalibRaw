use super::*;
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
        app.library.hdr_merge_message = Some("HDR merge requires GPU rendering.".to_owned());
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
        Ok(_) => {
            app.library.hdr_merge = Some(task);
            app.library.hdr_merge_message = None;
        }
        Err(error) => {
            app.library.hdr_merge_message = Some(format!("Could not start HDR merge: {error}"))
        }
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

impl LibraryState {
    pub(super) fn poll_hdr_merge(&mut self, context: &egui::Context) {
        let Some(task) = &self.hdr_merge else {
            return;
        };
        match task.receiver.try_recv() {
            Ok(result) => {
                self.hdr_merge = None;
                self.hdr_merge_message = Some(match result {
                    Ok(path) => {
                        self.refresh(context);
                        format!("HDR merge saved: {}", path.display())
                    }
                    Err(error) => format!("HDR merge: {error}"),
                });
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.hdr_merge = None;
                self.hdr_merge_message = Some("HDR merge stopped unexpectedly.".to_owned());
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    pub(super) fn show_hdr_merge_dialog(&mut self, ui: &mut Ui) {
        self.poll_hdr_merge(ui.ctx());
        if self.hdr_merge.is_none() && self.hdr_merge_message.is_none() {
            return;
        }
        let mut dismiss = false;
        crate::ui::responsive_popup(egui::Window::new("HDR merge"), ui.ctx(), 440.0)
            .id(egui::Id::new("library-hdr-merge-dialog"))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                if let Some(task) = &self.hdr_merge {
                    ui.horizontal_wrapped(|ui| {
                        ui.spinner();
                        if let Ok(progress) = task.progress.lock() {
                            ui.label(progress.as_str());
                        }
                    });
                    ui.add_space(8.0);
                    let cancelling = task.cancelled.load(Ordering::Relaxed);
                    if ui
                        .add_enabled(
                            !cancelling,
                            egui::Button::new(if cancelling {
                                "Cancelling…"
                            } else {
                                "Cancel"
                            }),
                        )
                        .clicked()
                    {
                        task.cancelled.store(true, Ordering::Relaxed);
                    }
                } else if let Some(message) = &self.hdr_merge_message {
                    ui.label(message);
                    ui.add_space(8.0);
                    dismiss = ui.button("Dismiss").clicked();
                }
            });
        if dismiss {
            self.hdr_merge_message = None;
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
            // The existing ICC path adapts through D50 and uses fixed-point matrix tags.
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
