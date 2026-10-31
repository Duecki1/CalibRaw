use super::*;

use super::batch::batch_export_overall_fraction;
use super::preview::DETAIL_ZOOM_START;
use crate::i18n::{tr, tr_fmt};

mod ai_denoise;
mod task_view;

use ai_denoise::{export_needs_ai_denoise, install_export_ai_denoise};

impl ExportTask {
    pub(super) fn new(
        kind: ExportTaskKind,
        cancellation: Arc<std::sync::atomic::AtomicBool>,
        receiver: Option<ExportTaskReceiver>,
        output: Option<ExportItemOutput>,
        total: usize,
    ) -> Self {
        Self {
            kind,
            cancellation,
            receiver,
            output,
            progress: 0.0,
            phase: match kind {
                ExportTaskKind::LibraryBatch => tr("Preparing batch export…").to_owned(),
                ExportTaskKind::Replay => tr("Preparing edit replay…").to_owned(),
                ExportTaskKind::Single => tr("Preparing tiled export…").to_owned(),
            },
            completed: 0,
            total,
            completed_tiles: 0,
            total_tiles: 0,
            cancelling: false,
        }
    }

    fn start_item(&mut self, receiver: mpsc::Receiver<ExportItemEvent>, output: ExportItemOutput) {
        self.receiver = Some(ExportTaskReceiver::Tiled(receiver));
        self.output = Some(output);
        self.completed_tiles = 0;
        self.total_tiles = 0;
        self.phase = tr("Preparing tiled export…").to_owned();
    }

    pub(super) fn request_cancel(&mut self) {
        use std::sync::atomic::Ordering;
        self.cancellation.store(true, Ordering::Release);
        self.cancelling = true;
        self.phase = match self.kind {
            ExportTaskKind::LibraryBatch => tr("Cancelling batch export…").to_owned(),
            ExportTaskKind::Replay => tr("Cancelling edit replay…").to_owned(),
            ExportTaskKind::Single => tr("Cancelling export…").to_owned(),
        };
    }
}

pub(in crate::app) fn export_source_stem(
    current_path: Option<&std::path::Path>,
    current_label: Option<&str>,
) -> String {
    current_label
        .and_then(|label| std::path::Path::new(label).file_stem())
        .and_then(std::ffi::OsStr::to_str)
        .or_else(|| {
            current_path
                .and_then(std::path::Path::file_stem)
                .and_then(std::ffi::OsStr::to_str)
        })
        .filter(|stem| !stem.is_empty())
        .unwrap_or("calibraw-export")
        .to_owned()
}

/// Renders one export on a worker, after installing the AI denoise the edit
/// needs ([`install_export_ai_denoise`]).
pub(in crate::app) fn spawn_export_item(
    request: ExportItemRequest,
    cancellation: Arc<std::sync::atomic::AtomicBool>,
) -> mpsc::Receiver<ExportItemEvent> {
    let ExportItemRequest {
        device,
        queue,
        source,
        output,
        format,
        settings,
    } = request;
    let PreparedExportSource {
        raw,
        geometry,
        mut exposure,
        masks,
        remove,
        look,
        source_file_name,
        gpu_export_prewarm,
        ai_denoise_result,
    } = source;
    // Rendered photos cannot be AI-denoised and show without it in Develop.
    exposure.ai_denoise_enabled &= !raw.is_pre_demosaiced_raster();
    let metadata = ExportMetadata::from_raw(&raw, source_file_name);
    let files = output.files();
    let (sender, receiver) = mpsc::channel();
    let worker_sender = sender.clone();
    let spawn_result = std::thread::Builder::new()
        .name("calibraw-export-item".to_owned())
        .spawn(move || {
            if export_needs_ai_denoise(&raw, &exposure) {
                let _ = worker_sender.send(ExportItemEvent::Denoising {
                    completed: 0,
                    total: 0,
                });
                if let Err(error) = install_export_ai_denoise(
                    &raw,
                    ai_denoise_result.as_deref(),
                    &device,
                    &queue,
                    &cancellation,
                    |completed, total| {
                        let _ = worker_sender.send(ExportItemEvent::Denoising { completed, total });
                    },
                ) {
                    let _ = worker_sender.send(ExportItemEvent::Render(ExportEvent::Finished(
                        Err(tr_fmt("AI denoise failed: {error}", &[("error", &error)])),
                    )));
                    return;
                }
            }
            let render = spawn_tiled_export(
                format,
                TiledExportJob {
                    device,
                    queue,
                    raw,
                    geometry,
                    exposure,
                    masks,
                    remove,
                    look,
                    files,
                    tile_spec: TileSpec::default(),
                    settings,
                    metadata,
                    cancellation,
                    program_prewarm: gpu_export_prewarm,
                },
            );
            for event in render {
                if worker_sender.send(ExportItemEvent::Render(event)).is_err() {
                    break;
                }
            }
        });
    if let Err(error) = spawn_result {
        let _ = sender.send(ExportItemEvent::Render(ExportEvent::Finished(Err(tr_fmt(
            "could not start export worker: {error}",
            &[("error", &error)],
        )))));
    }
    receiver
}

/// The export phase while [`ExportItemEvent::Denoising`] reports progress.
pub(super) fn ai_denoise_phase(completed: usize, total: usize) -> String {
    if total == 0 {
        tr("Applying AI denoise").to_owned()
    } else {
        tr_fmt(
            "Applying AI denoise: tile {completed}/{total}",
            &[("completed", &completed), ("total", &total)],
        )
    }
}

/// Runs one export to completion on the calling thread, passing on its
/// progress.
pub(in crate::app) fn run_export_item(
    request: ExportItemRequest,
    cancellation: Arc<std::sync::atomic::AtomicBool>,
    mut progress: impl FnMut(ExportItemEvent),
) -> Result<(), String> {
    let receiver = spawn_export_item(request, cancellation);
    while let Ok(event) = receiver.recv() {
        match event {
            ExportItemEvent::Render(ExportEvent::Finished(result)) => {
                return result.map(|_| ());
            }
            event => progress(event),
        }
    }
    Err(tr("export worker stopped unexpectedly").to_owned())
}

impl CalibRawApp {
    pub(crate) fn export_task_active(&self) -> bool {
        self.export.task.is_some()
    }

    pub(crate) fn can_export(&self) -> bool {
        self.develop.loaded_raw.is_some()
            && self.develop.preview_raw.is_some()
            && self.export.task.is_none()
            && !self.export.publish_pending
            && self.develop.load_receiver.is_none()
    }

    pub(super) fn templated_export_stem(&self) -> Option<String> {
        let raw = self.develop.loaded_raw.as_deref()?;
        let original_name = export_source_stem(
            self.develop.current_path.as_deref(),
            self.develop.current_label.as_deref(),
        );
        // Media-library photos open from a temporary copy without the
        // original's edit time.
        let edited_at = if self.caps().has_folders() {
            self.develop
                .current_path
                .as_deref()
                .and_then(crate::export_naming::edited_time_for_path)
        } else {
            None
        };
        let context =
            crate::export_naming::ExportNameContext::from_raw(original_name, raw, edited_at);
        Some(crate::export_naming::render_export_stem_or_default(
            &self.preferences.export_name_template,
            &context,
        ))
    }

    /// Exports the open photo: to a file chosen in a save dialog, or into the
    /// platform's shared media store.
    fn export_with_format(&mut self, frame: &eframe::Frame, format: ExportFormat) {
        if let Some(carousel) = self.export.carousel.active() {
            self.export_carousel(frame, carousel, format, false);
        } else if self.caps().has_folders() {
            self.export_desktop(frame, format);
        } else {
            self.export_android(frame, format);
        }
    }

    fn export_desktop(&mut self, frame: &eframe::Frame, format: ExportFormat) {
        if !self.can_export() {
            return;
        }

        let Some(stem) = self.templated_export_stem() else {
            return;
        };
        let initial_directory = self
            .develop
            .current_path
            .as_deref()
            .and_then(|path| path.parent());
        let default_name = crate::export_naming::free_export_file_name(
            initial_directory,
            &stem,
            format.extension(),
        );
        let Some(path) =
            crate::ui::choose_export_file_path(format, &default_name, initial_directory)
        else {
            return;
        };
        self.start_export(path, frame, format);
    }

    pub(crate) fn export_png(&mut self, frame: &eframe::Frame) {
        self.export_with_format(frame, ExportFormat::Png);
    }

    pub(crate) fn export_jpeg(&mut self, frame: &eframe::Frame) {
        self.export_with_format(frame, ExportFormat::Jpeg);
    }

    pub(crate) fn export_tiff(&mut self, frame: &eframe::Frame) {
        self.export_with_format(frame, ExportFormat::Tiff);
    }

    pub(crate) fn export_jxl(&mut self, frame: &eframe::Frame) {
        self.export_with_format(frame, ExportFormat::JpegXl);
    }

    /// Exports in the selected format, then opens the Android share sheet.
    pub(crate) fn export_and_share(&mut self, frame: &eframe::Frame) {
        let format = self.export.format;
        if let Some(carousel) = self.export.carousel.active() {
            self.export_carousel(frame, carousel, format, true);
        } else {
            self.start_android_export(frame, format, true);
        }
    }

    pub(in crate::app) fn export_android(&mut self, frame: &eframe::Frame, format: ExportFormat) {
        self.start_android_export(frame, format, false);
    }

    fn start_android_export(&mut self, frame: &eframe::Frame, format: ExportFormat, share: bool) {
        if !self.can_export() {
            return;
        }

        let Some(stem) = self.templated_export_stem() else {
            return;
        };
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let display_name = format!("{stem}.{}", format.extension());
        let cache_file_name = format!("{stem}-{timestamp}.{}", format.extension());
        let destination =
            match self.prepare_android_export_destination(display_name, cache_file_name, format) {
                Ok(destination) => destination,
                Err(error) => {
                    self.report_error(ErrorKind::Export, error);
                    return;
                }
            };
        let cleanup = destination.clone();
        if self
            .start_export_output(ExportItemOutput::Single(destination), frame, format)
            .is_none()
        {
            self.cancel_android_export_destination(&cleanup);
            return;
        }
        self.export.share_mime_type = share.then_some(format.mime_type());
    }

    /// Opens the share sheet when the finished single export or replay asked for it.
    pub(in crate::app) fn share_finished_export(
        &mut self,
        published: &crate::platform::PublishedExport,
    ) {
        let Some(mime_type) = self.export.share_mime_type.take() else {
            return;
        };
        if published.uri.is_empty() {
            self.report_error(
                ErrorKind::Share,
                tr_fmt(
                    "Exported to {location}, but Android did not provide a link to share it.",
                    &[("location", &published.location)],
                ),
            );
            return;
        }
        if let Err(error) = self.platform.share_export(&published.uri, mime_type) {
            self.report_error(
                ErrorKind::Share,
                tr_fmt(
                    "Exported to {location}, but sharing failed: {error}",
                    &[("location", &published.location), ("error", &error)],
                ),
            );
        }
    }

    pub(in crate::app) fn prepare_android_export_destination(
        &self,
        display_name: String,
        cache_file_name: String,
        format: ExportFormat,
    ) -> Result<ExportDestination, String> {
        let Some(data_dir) = self.platform.internal_data_dir() else {
            return Err(tr("Android did not provide an app data directory.").to_owned());
        };
        let export_dir = data_dir.join("cache").join("exports");
        std::fs::create_dir_all(&export_dir).map_err(|error| {
            tr_fmt(
                "Could not prepare Android export cache: {error}",
                &[("error", &error)],
            )
        })?;
        match self
            .platform
            .prepare_direct_export(&display_name, format.mime_type())
        {
            Ok(Some(path)) => Ok(ExportDestination::AndroidDirect {
                path,
                staging_dir: export_dir,
            }),
            Ok(None) => Ok(ExportDestination::AndroidGallery {
                path: export_dir.join(cache_file_name),
                display_name,
                format,
            }),
            Err(error) => {
                log::warn!("direct Android export unavailable, falling back to cache: {error}");
                Ok(ExportDestination::AndroidGallery {
                    path: export_dir.join(cache_file_name),
                    display_name,
                    format,
                })
            }
        }
    }

    pub(in crate::app) fn cancel_android_export_destination(
        &self,
        destination: &ExportDestination,
    ) {
        if let ExportDestination::AndroidDirect { path, .. } = destination {
            self.platform.cancel_direct_export(path);
        }
    }

    /// The request to export the open photo, with `export_override` applied
    /// on top of its edit when given. `Ok(None)` while the photo cannot be
    /// exported yet; `Err` for a failure to report.
    pub(in crate::app) fn capture_export_task_request(
        &mut self,
        output: ExportItemOutput,
        frame: &eframe::Frame,
        format: ExportFormat,
        export_override: Option<&ExportOverride>,
    ) -> Result<Option<ExportItemRequest>, String> {
        if self.develop.loaded_raw.is_none()
            || self.develop.preview_raw.is_none()
            || self.export.publish_pending
            || self.develop.load_receiver.is_some()
            || self.inpaint_processing()
        {
            return Ok(None);
        }

        let Some(raw) = self.develop.loaded_raw.as_ref().map(Arc::clone) else {
            return Ok(None);
        };
        let Some(render_state) = frame.wgpu_render_state() else {
            return Err(tr("eframe is not running with the wgpu backend.").to_owned());
        };
        let (exposure, geometry, masks, look) = match export_override {
            None => (
                self.develop.exposure,
                self.develop.geometry,
                self.masks.stack.clone(),
                self.look_for_export(),
            ),
            Some(export_override) => {
                let mut edits = SidecarEditState {
                    exposure: self.develop.exposure,
                    geometry: self.develop.geometry,
                    masks: Arc::new(self.masks.stack.clone()),
                    look_lut: self.develop.look_lut.clone(),
                    ..crate::sidecar::default_edit_state()
                };
                export_override.apply(&mut edits);
                let look = edits
                    .look_lut
                    .as_ref()
                    .map(|look| {
                        crate::app::luts::resolve_look_for_export(self.luts.store().as_ref(), look)
                    })
                    .transpose();
                (
                    edits.exposure,
                    edits.geometry,
                    Arc::unwrap_or_clone(edits.masks),
                    look,
                )
            }
        };
        let look = look?;
        let source_file_name = self
            .develop
            .current_path
            .as_ref()
            .and_then(|source| source.file_name())
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .or_else(|| self.develop.current_label.clone());
        Ok(Some(ExportItemRequest {
            device: render_state.device.clone(),
            queue: render_state.queue.clone(),
            source: PreparedExportSource {
                raw,
                geometry,
                exposure,
                masks,
                remove: self.inpaint.edits.as_ref().clone(),
                look,
                source_file_name,
                gpu_export_prewarm: self.export.gpu_prewarm.as_ref().map(Arc::clone),
                ai_denoise_result: self.ai_denoise_result_path(),
            },
            output,
            format,
            settings: self.export.watermark.export_settings(&self.export.settings),
        }))
    }

    pub(in crate::app) fn start_export(
        &mut self,
        path: PathBuf,
        frame: &eframe::Frame,
        format: ExportFormat,
    ) -> Option<()> {
        self.start_export_output(
            ExportItemOutput::Single(ExportDestination::File(path)),
            frame,
            format,
        )
    }

    pub(super) fn start_export_output(
        &mut self,
        output: ExportItemOutput,
        frame: &eframe::Frame,
        format: ExportFormat,
    ) -> Option<()> {
        if !self.can_export() {
            return None;
        }
        let request = match self.capture_export_task_request(output, frame, format, None) {
            Ok(request) => request?,
            Err(error) => {
                self.report_error(ErrorKind::Export, error);
                return None;
            }
        };
        if let Err(error) = self.start_export_task(request, ExportTaskKind::Single) {
            self.report_error(
                ErrorKind::Export,
                tr_fmt("Export failed: {error}", &[("error", &error)]),
            );
            return None;
        }
        Some(())
    }

    pub(in crate::app) fn start_export_task(
        &mut self,
        request: ExportItemRequest,
        kind: ExportTaskKind,
    ) -> Result<(), String> {
        let cancellation = match (kind, self.export.task.as_ref()) {
            (ExportTaskKind::Single, None) => Arc::new(std::sync::atomic::AtomicBool::new(false)),
            (ExportTaskKind::Single, Some(_)) => {
                return Err(tr("another export is already active").to_owned());
            }
            (ExportTaskKind::LibraryBatch, Some(task))
                if task.kind == ExportTaskKind::LibraryBatch =>
            {
                Arc::clone(&task.cancellation)
            }
            (ExportTaskKind::LibraryBatch, _) => {
                return Err(tr("the library batch export is no longer active").to_owned());
            }
            (ExportTaskKind::Replay, _) => {
                return Err(tr("edit replay uses its dedicated export worker").to_owned());
            }
        };
        let output = request.output.clone();
        let receiver = spawn_export_item(request, Arc::clone(&cancellation));
        if kind == ExportTaskKind::Single {
            self.export.task = Some(ExportTask::new(
                kind,
                cancellation,
                Some(ExportTaskReceiver::Tiled(receiver)),
                Some(output),
                1,
            ));
        } else if let Some(task) = self.export.task.as_mut() {
            task.start_item(receiver, output);
        }
        self.ui.notice = None;
        self.egui_ctx.request_repaint();
        Ok(())
    }

    pub(crate) fn cancel_export_task(&mut self) -> bool {
        let Some(task) = self.export.task.as_mut() else {
            return false;
        };
        task.request_cancel();
        if let Some(batch) = self.export.batch.as_mut() {
            batch.cancel_requested = true;
            batch.pending.clear();
        }
        self.egui_ctx.request_repaint();
        true
    }

    pub(in crate::app) fn poll_export_worker(&mut self, _frame: &eframe::Frame) {
        if self
            .export
            .task
            .as_ref()
            .is_some_and(|task| task.kind == ExportTaskKind::Replay)
        {
            self.poll_edit_replay_worker();
            return;
        }

        let (events, disconnected) = match self
            .export
            .task
            .as_ref()
            .and_then(|task| task.receiver.as_ref())
        {
            Some(ExportTaskReceiver::Tiled(receiver)) => {
                drain_worker_events(Some(receiver), |event| {
                    matches!(event, ExportItemEvent::Render(ExportEvent::Finished(_)))
                })
            }
            _ => return,
        };

        let mut finished = false;
        // Media-library exports are published, or fail, before the batch moves on.
        let publishes = self.caps().has_media_library();
        let mut android_batch_result: Option<Result<(), String>> = None;
        for event in events {
            match event {
                ExportItemEvent::Denoising { completed, total } => {
                    if let Some(task) = self.export.task.as_mut() {
                        task.phase = ai_denoise_phase(completed, total);
                    }
                }
                ExportItemEvent::Render(ExportEvent::Progress {
                    completed_tiles,
                    total_tiles,
                }) => {
                    let batch_current = self
                        .export
                        .batch
                        .as_ref()
                        .is_some_and(|batch| batch.current.is_some());
                    if let Some(task) = self.export.task.as_mut() {
                        task.completed_tiles = completed_tiles;
                        task.total_tiles = total_tiles;
                        let tile_fraction = if total_tiles == 0 {
                            0.0
                        } else {
                            (completed_tiles as f32 / total_tiles as f32).clamp(0.0, 1.0)
                        };
                        if task.kind == ExportTaskKind::LibraryBatch {
                            task.progress = batch_export_overall_fraction(
                                task.completed,
                                task.total,
                                batch_current,
                                Some((completed_tiles, total_tiles)),
                            );
                        } else {
                            task.progress = (tile_fraction * EXPORT_TILE_PHASE_WEIGHT)
                                .min(EXPORT_MAX_INCOMPLETE_FRACTION);
                        }
                        task.phase = if total_tiles == 0 {
                            tr("Preparing tiled export…").to_owned()
                        } else if completed_tiles >= total_tiles {
                            tr("Finalizing export…").to_owned()
                        } else {
                            tr_fmt(
                                "Rendering tile {completed_tiles}/{total_tiles}",
                                &[
                                    ("completed_tiles", &completed_tiles),
                                    ("total_tiles", &total_tiles),
                                ],
                            )
                        };
                    }
                }
                ExportItemEvent::Render(ExportEvent::WaitingForGpu { waited }) => {
                    if let Some(task) = self.export.task.as_mut() {
                        let waiting = tr_fmt(
                            "waiting for the GPU ({seconds} s)",
                            &[("seconds", &waited.as_secs())],
                        );
                        task.phase = if task.total_tiles == 0 {
                            tr_fmt(
                                "Preparing tiled export: {waiting}",
                                &[("waiting", &waiting)],
                            )
                        } else {
                            tr_fmt(
                                "Rendering tile {completed_tiles}/{total_tiles}: {waiting}",
                                &[
                                    ("completed_tiles", &task.completed_tiles),
                                    ("total_tiles", &task.total_tiles),
                                    ("waiting", &waiting),
                                ],
                            )
                        };
                    }
                }
                ExportItemEvent::Render(ExportEvent::Finished(result)) => {
                    finished = true;
                    if let Some(task) = self.export.task.as_mut() {
                        task.receiver = None;
                        task.completed_tiles = 0;
                        task.total_tiles = 0;
                        task.progress = task.progress.min(EXPORT_MAX_INCOMPLETE_FRACTION);
                        task.phase = tr("Finalizing export…").to_owned();
                    }
                    let is_batch = self.export.batch.is_some();
                    let was_cancelled = self
                        .export
                        .task
                        .as_ref()
                        .is_some_and(|task| task.cancelling);

                    match result {
                        Ok(path) => {
                            let output = self
                                .export
                                .task
                                .as_ref()
                                .and_then(|task| task.output.clone());
                            if let Some(ExportItemOutput::Carousel { slides, .. }) = output {
                                self.finish_carousel_render(&path, slides);
                            } else if !publishes {
                                if !is_batch {
                                    self.ui.notice = Some(tr_fmt(
                                        "Exported {path}",
                                        &[("path", &path.display())],
                                    ));
                                    self.export.task = None;
                                }
                            } else {
                                match output.and_then(ExportItemOutput::into_single) {
                                    Some(ExportDestination::AndroidDirect {
                                        path: direct_path,
                                        ..
                                    }) => {
                                        debug_assert_eq!(path, direct_path);
                                        match self.platform.finalize_direct_export(&direct_path) {
                                            Ok(published) => {
                                                if is_batch {
                                                    android_batch_result = Some(Ok(()));
                                                } else {
                                                    self.ui.notice = Some(tr_fmt(
                                                        "Exported to {location}",
                                                        &[("location", &published.location)],
                                                    ));
                                                    self.share_finished_export(&published);
                                                    self.export.task = None;
                                                }
                                            }
                                            Err(error) => {
                                                if is_batch {
                                                    android_batch_result = Some(Err(error.clone()));
                                                } else {
                                                    self.export.task = None;
                                                }
                                                self.report_error(
                                                    ErrorKind::Export,
                                                    tr_fmt(
                                                        "Export failed: {error}",
                                                        &[("error", &error)],
                                                    ),
                                                );
                                            }
                                        }
                                    }
                                    Some(ExportDestination::AndroidGallery {
                                        path: cache_path,
                                        display_name,
                                        format,
                                    }) => {
                                        debug_assert_eq!(path, cache_path);
                                        match self.platform.publish_image(
                                            &cache_path,
                                            &display_name,
                                            format.mime_type(),
                                        ) {
                                            Ok(()) => {
                                                self.export.publish_pending = true;
                                                self.ui.notice = Some(tr_fmt(
                                                    "Saving to {location}…",
                                                    &[(
                                                        "location",
                                                        &self.platform.export_location_label(),
                                                    )],
                                                ));
                                                if let Some(task) = self.export.task.as_mut() {
                                                    task.phase = tr_fmt(
                                                        "Publishing to {location}…",
                                                        &[(
                                                            "location",
                                                            &self.platform.export_location_label(),
                                                        )],
                                                    );
                                                    task.progress = task
                                                        .progress
                                                        .max(EXPORT_MAX_INCOMPLETE_FRACTION);
                                                }
                                            }
                                            Err(error) => {
                                                let _ = std::fs::remove_file(&cache_path);
                                                if is_batch {
                                                    android_batch_result = Some(Err(error.clone()));
                                                } else {
                                                    self.export.task = None;
                                                }
                                                self.report_error(
                                                    ErrorKind::Export,
                                                    tr_fmt(
                                                        "Export failed: {error}",
                                                        &[("error", &error)],
                                                    ),
                                                );
                                            }
                                        }
                                    }
                                    Some(ExportDestination::File(_))
                                    | Some(ExportDestination::NewFile { .. })
                                    | None => {
                                        let error =
                                            tr("export destination state was lost").to_owned();
                                        if is_batch {
                                            android_batch_result = Some(Err(error.clone()));
                                        } else {
                                            self.export.task = None;
                                        }
                                        self.report_error(
                                            ErrorKind::Export,
                                            tr_fmt("Export failed: {error}", &[("error", &error)]),
                                        );
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            if publishes {
                                self.platform.cancel_all_direct_exports();
                                if is_batch {
                                    android_batch_result = Some(Err(error.clone()));
                                } else {
                                    self.export.task = None;
                                }
                            } else if !is_batch {
                                self.export.task = None;
                            }
                            if was_cancelled {
                                self.ui.notice = Some(tr("Export cancelled.").to_owned());
                                log::info!("export cancelled");
                            } else {
                                self.report_error(
                                    ErrorKind::Export,
                                    tr_fmt("Export failed: {error}", &[("error", &error)]),
                                );
                            }
                        }
                    }
                }
            }
        }

        if disconnected && !finished {
            if let Some(task) = self.export.task.as_mut() {
                task.receiver = None;
            }
            self.report_error(ErrorKind::Export, tr("Export worker stopped unexpectedly."));
            if publishes {
                self.platform.cancel_all_direct_exports();
                if self.export.batch.is_some() {
                    android_batch_result =
                        Some(Err(tr("export worker stopped unexpectedly").to_owned()));
                } else {
                    self.export.task = None;
                }
            } else if self.export.batch.is_none() {
                self.export.task = None;
            }
        }

        if let Some(result) = android_batch_result {
            self.complete_android_library_batch_export_item(result);
        }
    }

    pub(in crate::app) fn refresh_status(&mut self) {
        self.ui.status = if let Some(label) = &self.develop.loading_label {
            tr_fmt(
                "Decoding and preparing proxy for {label}…",
                &[("label", &label)],
            )
        } else if self.lens_correction_busy() {
            self.develop.lens_correction.catalog.status.clone()
        } else if let Some(task) = self.export.task.as_ref() {
            task.phase.clone()
        } else if self.export.publish_pending {
            tr_fmt(
                "Saving to {location}…",
                &[("location", &self.platform.export_location_label())],
            )
        } else if self.preview.zoom > DETAIL_ZOOM_START {
            if let Some(stage) = self.preview.detail_pending_stage {
                tr_fmt(
                    "Updating visible zoom crop — {stage}…",
                    &[("stage", &tr(stage.label()))],
                )
            } else if let Some(notice) = &self.ui.notice {
                notice.clone()
            } else {
                self.develop.image_status.clone()
            }
        } else if let Some(stage) = self.preview.pending_stage {
            tr_fmt(
                "Updating preview — {stage}…",
                &[("stage", &tr(stage.label()))],
            )
        } else if let Some(notice) = &self.ui.notice {
            notice.clone()
        } else {
            self.develop.image_status.clone()
        };
    }

    /// Restores the adjustments a first open of this photo starts with,
    /// including its camera- and ISO-dependent noise reduction, and removes
    /// its LUT.
    pub(crate) fn reset_develop_adjustments(&mut self) {
        let mut exposure = self.new_image_exposure();
        if let Some(raw) = self.develop.original_raw.as_deref() {
            raw.apply_adaptive_detail_defaults(&mut exposure);
        }
        self.select_look_lut(None);
        self.develop.exposure = exposure;
        self.develop_ui.point_color = Default::default();
        self.develop_ui.cancel_area_picker();
        self.mark_pipeline_dirty();
    }
}
