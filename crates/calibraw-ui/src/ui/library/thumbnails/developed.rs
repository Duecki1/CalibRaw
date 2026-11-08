//! Rendering the saved edits of photos whose thumbnail does not show them:
//! one photo at a time, once the library has loaded every preview of the
//! folder, so the grid fills first. Until a render arrives, its tile keeps
//! its thumbnail with the loading wave over it.

use super::*;
use crate::app::{EditedPhoto, EditedRender, EditedRenderJob, SavedEditsSettings};
use crate::i18n::{tr, tr_fmt};

/// Failures a report names one by one; the rest are counted.
const LISTED_FAILURES: usize = 5;

/// The edited-thumbnail render that runs, if one does, and the failures not
/// reported yet.
#[derive(Default)]
pub(in crate::ui::library) struct DevelopedRenders {
    running: Option<RunningRender>,
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
    /// Stops the running render before its next step. Its photo keeps
    /// waiting and renders again later.
    pub(in crate::ui::library) fn cancel(&self) {
        if let Some(running) = &self.running {
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
    /// Whether thumbnails that do not show the saved edits render them.
    pub(in crate::ui::library) fn renders_saved_edits(&self) -> bool {
        self.render_edited_thumbnails_during_indexing && self.platform.caps().edited_thumbnails
    }

    /// The entry whose saved edits render next, when one may start: no render
    /// runs, every preview of the folder has loaded and the library is shown.
    /// The photo shown most recently goes first, then the grid's order.
    fn next_developed_render(&self) -> Option<usize> {
        if self.developed.running.is_some()
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
            .filter(|(_, entry)| entry.edits == ThumbnailEdits::Rendering)
            .max_by_key(|(index, entry)| (entry.last_used, std::cmp::Reverse(*index)))
            .map(|(index, _)| index)
    }

    pub(crate) fn developed_render_due(&self) -> bool {
        self.next_developed_render().is_some()
    }

    /// Starts rendering the saved edits of the next waiting photo, when one
    /// is due.
    pub(crate) fn start_developed_render(
        &mut self,
        settings: SavedEditsSettings,
        context: &egui::Context,
    ) {
        let Some(index) = self.next_developed_render() else {
            return;
        };
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
            Ok(_) => {
                self.developed.running = Some(RunningRender {
                    generation,
                    asset_id,
                    cancel,
                    result,
                });
            }
            Err(error) => self.fail_developed_render(
                index,
                tr_fmt(
                    "could not start the edited-thumbnail render: {error}",
                    &[("error", &error)],
                ),
            ),
        }
    }

    /// Shows a finished render in its tile.
    pub(in crate::ui::library) fn poll_developed_render(&mut self, context: &egui::Context) {
        let Some(running) = &self.developed.running else {
            return;
        };
        let result = match running.result.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err(tr("the edited-thumbnail render stopped unexpectedly").to_owned())
            }
        };
        let Some(running) = self.developed.running.take() else {
            return;
        };
        // The next render starts on the next frame.
        context.request_repaint();
        if running.generation != self.generation.load(Ordering::Acquire) {
            return;
        }
        let Some(index) = self.entry_indices.get(&running.asset_id).copied() else {
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
                self.developed.revision += 1;
                let revision = self.developed.revision;
                self.install_developed_thumbnail_at(index, thumbnail, context, revision);
            }
            // The sidecar changed while the edits rendered, or the library
            // gave way to Develop: the photo renders again.
            Ok(EditedRender::Rendered { current: false, .. } | EditedRender::Cancelled) => {}
            Ok(EditedRender::Unedited) => self.entries[index].edits = ThumbnailEdits::Unedited,
            Err(error) => self.fail_developed_render(index, error),
        }
    }

    fn fail_developed_render(&mut self, index: usize, error: String) {
        let entry = &mut self.entries[index];
        self.developed
            .failures
            .push(format!("{}: {error}", entry.asset.display_name));
        entry.edits = ThumbnailEdits::Failed(error);
    }

    /// One message for the renders that failed, once no photo waits for its
    /// edits to render.
    pub(crate) fn take_developed_render_failures(&mut self) -> Option<String> {
        if self.developed.failures.is_empty()
            || self.developed.running.is_some()
            || self
                .entries
                .iter()
                .any(|entry| entry.edits == ThumbnailEdits::Rendering)
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
