//! Rendering the saved edits of photos whose thumbnail does not show them,
//! once the library has loaded every preview of the folder, so the grid fills
//! first. Until a render arrives, its tile keeps its thumbnail with the
//! loading wave over it.

use super::*;
use crate::app::{EditedPhoto, EditedRender, EditedRenderJob, SavedEditsSettings};
use crate::i18n::{tr, tr_fmt};

/// Failures a report names one by one; the rest are counted.
const LISTED_FAILURES: usize = 5;

/// The edited-thumbnail renders that run, how many finished, and the failures
/// not reported yet.
#[derive(Default)]
pub(in crate::ui::library) struct DevelopedRenders {
    running: Vec<RunningRender>,
    /// Renders finished since no photo was waiting, for the progress bar.
    finished: usize,
    /// `display name: error` of each render that failed since the last report.
    failures: Vec<String>,
    /// Tells the textures of rendered thumbnails apart.
    revision: u64,
}

struct RunningRender {
    generation: u64,
    asset_id: LibraryAssetId,
    cancel: Arc<AtomicBool>,
    result: mpsc::Receiver<Result<EditedRender, String>>,
}

impl DevelopedRenders {
    /// Stops the running renders before their next step. Their photos keep
    /// waiting and render again later.
    pub(in crate::ui::library) fn cancel(&self) {
        for running in &self.running {
            running.cancel.store(true, Ordering::Release);
        }
    }
}

impl LibraryAsset {
    fn edited_photo(&self) -> EditedPhoto {
        match &self.locator {
            LibraryLocator::Desktop(path) => EditedPhoto::File(path.clone()),
            LibraryLocator::Android { uri } => EditedPhoto::MediaLibrary {
                uri: uri.clone(),
                display_name: self.display_name.clone(),
            },
        }
    }
}

impl LibraryState {
    /// Whether the platform renders the saved edits thumbnails do not show.
    pub(in crate::ui::library) fn renders_saved_edits(&self) -> bool {
        self.platform.caps().edited_thumbnails
    }

    /// How many photos' edits render at once. Each render holds a decoded
    /// RAW: where memory is constrained, and shared with the GPU and the
    /// editor's photo, one runs; elsewhere as many as indexing renders whole
    /// RAWs at once (`MAX_RENDERED_THUMBNAIL_WORKERS`), within the worker
    /// setting.
    fn developed_render_limit(&self) -> usize {
        if self.platform.caps().constrained_memory() {
            1
        } else {
            self.active_thumbnail_worker_count()
                .clamp(1, MAX_RENDERED_THUMBNAIL_WORKERS)
        }
    }

    /// The entry whose saved edits render next, when one may start: fewer
    /// renders run than the limit, every preview of the folder has loaded and
    /// the library is shown. The photo shown most recently goes first, then
    /// the grid's order.
    fn next_developed_render(&self) -> Option<usize> {
        if self.developed.running.len() >= self.developed_render_limit()
            || self.saved_edits_gpu.is_none()
            || self.decoding_paused.load(Ordering::Acquire)
            || self.scanning
            || !self.catalog_ready
            || !self.thumbnail_progress.is_complete()
        {
            return None;
        }
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.edits == ThumbnailEdits::Rendering
                    && !self
                        .developed
                        .running
                        .iter()
                        .any(|running| running.asset_id == entry.asset.id)
            })
            .max_by_key(|(index, entry)| (entry.last_used, std::cmp::Reverse(*index)))
            .map(|(index, _)| index)
    }

    pub(crate) fn developed_render_due(&self) -> bool {
        self.next_developed_render().is_some()
    }

    /// Starts rendering the saved edits of the next waiting photos, as many as
    /// may run.
    pub(crate) fn start_developed_renders(
        &mut self,
        settings: &SavedEditsSettings,
        context: &egui::Context,
    ) {
        while let Some(index) = self.next_developed_render() {
            self.start_developed_render(index, settings.clone(), context);
        }
    }

    fn start_developed_render(
        &mut self,
        index: usize,
        settings: SavedEditsSettings,
        context: &egui::Context,
    ) {
        let Some(gpu) = self.saved_edits_gpu.clone() else {
            return;
        };
        let generation = self.generation.load(Ordering::Acquire);
        let asset = &self.entries[index].asset;
        let asset_id = asset.id.clone();
        let photo = asset.edited_photo();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let platform = Arc::clone(&self.platform);
        let decode_gate = Arc::clone(&self.decode_gate);
        let repaint = context.clone();
        let (sender, result) = mpsc::channel();
        let spawn = std::thread::Builder::new()
            .name("calibraw-edited-thumbnail".to_owned())
            .spawn(move || {
                let render = EditedRenderJob {
                    photo: &photo,
                    maximum_edge: THUMBNAIL_EDGE,
                    lens_correction: false,
                    gpu: &gpu,
                    settings: &settings,
                    platform: &*platform,
                    decode_gate: &decode_gate,
                    cancel: &worker_cancel,
                }
                .run();
                // The library may have stopped waiting for this render.
                let _ = sender.send(render);
                repaint.request_repaint();
            });
        match spawn {
            Ok(_) => self.developed.running.push(RunningRender {
                generation,
                asset_id,
                cancel,
                result,
            }),
            Err(error) => self.fail_developed_render(
                index,
                tr_fmt(
                    "could not start the edited-thumbnail render: {error}",
                    &[("error", &error)],
                ),
            ),
        }
    }

    /// Shows finished renders in their tiles.
    pub(in crate::ui::library) fn poll_developed_renders(&mut self, context: &egui::Context) {
        // The next photos to wait count their progress from zero.
        if self.developed.running.is_empty() && self.developed_renders_waiting() == 0 {
            self.developed.finished = 0;
        }
        let mut finished = Vec::new();
        self.developed.running.retain(|running| {
            let result = match running.result.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => return true,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Err(tr("the edited-thumbnail render stopped unexpectedly").to_owned())
                }
            };
            finished.push((running.generation, running.asset_id.clone(), result));
            false
        });
        if finished.is_empty() {
            return;
        }
        // The next renders start on the next frame.
        context.request_repaint();
        for (generation, asset_id, result) in finished {
            self.show_developed_render(generation, &asset_id, result, context);
        }
    }

    fn show_developed_render(
        &mut self,
        generation: u64,
        asset_id: &LibraryAssetId,
        result: Result<EditedRender, String>,
        context: &egui::Context,
    ) {
        if generation != self.generation.load(Ordering::Acquire) {
            return;
        }
        let Some(index) = self.entry_indices.get(asset_id).copied() else {
            return;
        };
        // The editor installed a newer render meanwhile, or the edits were reset.
        if self.entries[index].edits != ThumbnailEdits::Rendering {
            return;
        }
        match result {
            Ok(EditedRender::Rendered {
                thumbnail,
                current: true,
            }) => {
                self.developed.finished += 1;
                self.developed.revision += 1;
                let revision = self.developed.revision;
                self.install_developed_thumbnail_at(index, thumbnail, context, revision);
            }
            // The sidecar changed while the edits rendered, or the library
            // gave way to Develop: the photo renders again.
            Ok(EditedRender::Rendered { current: false, .. } | EditedRender::Cancelled) => {}
            Ok(EditedRender::Unedited) => {
                self.developed.finished += 1;
                self.entries[index].edits = ThumbnailEdits::Unedited;
            }
            Err(error) => self.fail_developed_render(index, error),
        }
    }

    fn fail_developed_render(&mut self, index: usize, error: String) {
        self.developed.finished += 1;
        let entry = &mut self.entries[index];
        self.developed
            .failures
            .push(format!("{}: {error}", entry.asset.display_name));
        entry.edits = ThumbnailEdits::Failed(error);
    }

    /// Photos waiting for their saved edits to render, the running ones too.
    fn developed_renders_waiting(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.edits == ThumbnailEdits::Rendering)
            .count()
    }

    /// How far the renders have come since no photo was waiting; `None`
    /// while none waits.
    pub(in crate::ui::library) fn developed_render_progress(
        &self,
        paused: bool,
    ) -> Option<ThumbnailProgress> {
        let waiting = self.developed_renders_waiting();
        (waiting > 0).then_some(ThumbnailProgress {
            stage: ThumbnailStage::Edits,
            completed: self.developed.finished,
            total: self.developed.finished + waiting,
            paused,
        })
    }

    /// One message for the renders that failed, once no photo waits for its
    /// edits to render.
    pub(crate) fn take_developed_render_failures(&mut self) -> Option<String> {
        if self.developed.failures.is_empty()
            || !self.developed.running.is_empty()
            || self.developed_renders_waiting() > 0
        {
            return None;
        }
        let failures = std::mem::take(&mut self.developed.failures);
        let mut listed = failures
            .iter()
            .take(LISTED_FAILURES)
            .cloned()
            .collect::<Vec<_>>();
        if failures.len() > LISTED_FAILURES {
            listed.push(tr_fmt(
                "…and {count} more",
                &[("count", &(failures.len() - LISTED_FAILURES))],
            ));
        }
        Some(tr_fmt(
            "The saved edits of these photos could not be rendered:\n{failures}",
            &[("failures", &listed.join("\n"))],
        ))
    }
}
