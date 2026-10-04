//! Edit replay commands: starting the export, polling its worker and
//! publishing the result. Rendering and encoding live in
//! `crate::services::replay`.

use super::*;

use crate::services::replay::{render_edit_replay, ReplayError, ReplayRequest};

impl CalibRawApp {
    pub(crate) fn edit_replay_progress_state(&self) -> Option<(f32, String)> {
        self.export.task.as_ref().and_then(|task| {
            (task.kind == ExportTaskKind::Replay)
                .then(|| (task.progress.clamp(0.0, 1.0), task.phase.clone()))
        })
    }

    pub(crate) fn create_edit_replay(&mut self, frame: &eframe::Frame) {
        if !self.can_export() || self.inpaint_processing() {
            return;
        }
        let Some(stem) = self.templated_export_stem() else {
            return;
        };
        let default_name = format!("{stem}-edit-replay.mp4");
        #[cfg(not(target_os = "android"))]
        let initial_directory = self
            .develop
            .current_path
            .as_deref()
            .and_then(|path| path.parent());
        #[cfg(not(target_os = "android"))]
        let Some(destination) =
            crate::ui::choose_edit_replay_file_path(&default_name, initial_directory)
        else {
            return;
        };
        #[cfg(target_os = "android")]
        let destination = {
            let Some(data_dir) = self.android.android_app.internal_data_path() else {
                self.ui.notice = Some("Android did not provide an app data directory.".to_owned());
                return;
            };
            let export_dir = data_dir.join("cache").join("exports");
            if let Err(error) = std::fs::create_dir_all(&export_dir) {
                self.ui.notice = Some(format!("Could not prepare replay cache: {error}"));
                return;
            }
            export_dir.join(default_name)
        };
        let Some(render_state) = frame.wgpu_render_state() else {
            self.ui.notice = Some("eframe is not running with the wgpu backend.".to_owned());
            return;
        };
        let Some(raw) = self.develop.loaded_raw.as_ref().map(Arc::clone) else {
            return;
        };
        let request = ReplayRequest {
            device: render_state.device.clone(),
            queue: render_state.queue.clone(),
            raw,
            original_exposure: self.preview.original_exposure,
            final_exposure: self.develop.exposure,
            final_geometry: self.develop.geometry,
            final_masks: self.masks.stack.clone(),
            final_remove: self.inpaint.edits.as_ref().clone(),
            gpu_export_prewarm: self.export.gpu_prewarm.as_ref().map(Arc::clone),
            #[cfg(target_os = "android")]
            android_app: self.android.android_app.clone(),
        };
        let cancellation = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let repaint = self.egui_ctx.clone();
        let worker_cancellation = Arc::clone(&cancellation);
        match std::thread::Builder::new()
            .name("calibraw-edit-replay".to_owned())
            .spawn(move || {
                let progress_sender = sender.clone();
                let progress_repaint = repaint.clone();
                let result = render_edit_replay(
                    request,
                    destination,
                    &worker_cancellation,
                    &mut |progress| {
                        let _ = progress_sender.send(ReplayExportEvent::Progress(progress));
                        progress_repaint.request_repaint();
                    },
                );
                let _ = sender.send(ReplayExportEvent::Finished(result));
                repaint.request_repaint();
            }) {
            Ok(_) => {
                self.export.task = Some(ExportTask::new(
                    ExportTaskKind::Replay,
                    cancellation,
                    Some(ExportTaskReceiver::Replay(receiver)),
                    None,
                    1,
                ));
                if let Some(task) = self.export.task.as_mut() {
                    task.phase = "Preparing edit replay…".to_owned();
                }
                self.ui.notice = None;
                self.egui_ctx.request_repaint();
            }
            Err(error) => {
                self.ui.notice = Some(format!("Could not start edit replay export: {error}"));
            }
        }
    }

    pub(super) fn poll_edit_replay_worker(&mut self) {
        let (events, disconnected) = match self
            .export
            .task
            .as_ref()
            .and_then(|task| task.receiver.as_ref())
        {
            Some(ExportTaskReceiver::Replay(receiver)) => {
                drain_worker_events(Some(receiver), |event| {
                    matches!(event, ReplayExportEvent::Finished(_))
                })
            }
            _ => return,
        };

        let mut finished = false;
        for event in events {
            match event {
                ReplayExportEvent::Progress(progress) => {
                    if let Some(task) = self.export.task.as_mut() {
                        // Stay below 100 % until the result has been handled.
                        task.progress =
                            progress.fraction.clamp(0.0, EXPORT_MAX_INCOMPLETE_FRACTION);
                        task.phase = progress.phase;
                        task.completed_tiles = progress.completed_frames;
                        task.total_tiles = progress.total_frames;
                    }
                }
                ReplayExportEvent::Finished(result) => {
                    finished = true;
                    let was_cancelled = self
                        .export
                        .task
                        .as_ref()
                        .is_some_and(|task| task.cancelling);
                    match result {
                        Ok(path) => {
                            #[cfg(not(target_os = "android"))]
                            {
                                self.ui.notice =
                                    Some(format!("Created edit replay {}", path.display()));
                            }
                            #[cfg(target_os = "android")]
                            {
                                let name = path.file_name().unwrap_or_default().to_string_lossy();
                                match calibraw_ffi::publish_image(
                                    &self.android.android_app,
                                    &path,
                                    &name,
                                    "video/mp4",
                                ) {
                                    Ok(()) => {
                                        self.export.publish_pending = true;
                                        self.ui.notice =
                                            Some("Saving edit replay to gallery…".to_owned());
                                    }
                                    Err(error) => {
                                        let _ = std::fs::remove_file(&path);
                                        self.ui.notice =
                                            Some(format!("Could not save edit replay: {error}"));
                                    }
                                }
                            }
                        }
                        Err(ReplayError::Cancelled) => {
                            self.ui.notice = Some("Edit replay cancelled.".to_owned());
                        }
                        Err(ReplayError::Failed(_)) if was_cancelled => {
                            self.ui.notice = Some("Edit replay cancelled.".to_owned());
                        }
                        Err(ReplayError::Failed(error)) => {
                            self.ui.notice = Some(format!("Edit replay failed: {error}"));
                            log::error!("edit replay failed: {error}");
                        }
                    }
                    self.export.task = None;
                }
            }
        }
        if disconnected && !finished {
            self.ui.notice = Some("Edit replay worker stopped unexpectedly.".to_owned());
            self.export.task = None;
        }
    }
}
