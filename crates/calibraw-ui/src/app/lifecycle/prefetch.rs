//! Preparing the open photo's library neighbours in the background, so that
//! stepping to one finds its decode, lens correction, highlight analysis and
//! GPU program ready. The work itself is [`prepare_ahead_of_open`]; this
//! module chooses the neighbours and runs it off the UI thread.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// Neighbour prefetch state for the open photo.
#[derive(Default)]
pub(crate) struct NeighbourPrefetch {
    /// Set to stop the running prefetch.
    running: Option<Arc<AtomicBool>>,
    /// The photo the last prefetch started from, to tell the browsing direction.
    origin: Option<PathBuf>,
}

impl CalibRawApp {
    /// Starts preparing the open photo's neighbours, as many as the
    /// decoded-RAW cache holds besides the open photo. The neighbour in the
    /// direction of browsing comes first, so a cache with room for one
    /// neighbour still covers browsing backwards.
    pub(in crate::app) fn prefetch_neighbour_raws(&mut self) {
        self.cancel_neighbour_prefetch();
        let Some(current) = self.develop.current_path.clone() else {
            return;
        };
        let previous_origin = self
            .develop
            .neighbour_prefetch
            .origin
            .replace(current.clone());
        let browsing_backwards = previous_origin
            .and_then(|origin| self.library.filmstrip_index_for_path(&origin))
            .zip(self.library.filmstrip_index_for_path(&current))
            .is_some_and(|(origin, current)| current < origin);
        let current = current.as_path();
        let neighbours = [!browsing_backwards, browsing_backwards]
            .into_iter()
            .filter_map(|forward| {
                self.library
                    .adjacent_library_item_for_path(current, forward)
            })
            .map(|item| item.path)
            .take(self.develop.decoded_raws.limit().saturating_sub(1))
            .collect::<Vec<_>>();
        if neighbours.is_empty() {
            return;
        }

        let context = AheadOfOpen {
            decoded_raws: self.develop.decoded_raws.clone(),
            decode_gate: self.library.decode_gate(),
            camera_profiles: self.camera_profile_settings(),
            automatic_lens: self.preferences.automatic_lens_correction,
            initial_exposure: self.new_image_exposure(),
            program_template: self.preview.program_template.clone(),
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let spawned = std::thread::Builder::new()
            .name("calibraw-raw-prefetch".to_owned())
            .spawn(move || {
                let is_cancelled = || worker_cancelled.load(Ordering::Acquire);
                for path in neighbours {
                    if is_cancelled() {
                        return;
                    }
                    let started = Instant::now();
                    match prepare_ahead_of_open(&context, &path, is_cancelled) {
                        Ok(()) => calibraw_core::diagnostics::record(format!(
                            "Prefetched {} in {:.3}s",
                            path.display(),
                            started.elapsed().as_secs_f64()
                        )),
                        // Opening the photo reports the failure, if it is opened.
                        Err(error) => {
                            log::debug!("could not prefetch {}: {error:#}", path.display())
                        }
                    }
                }
            });
        match spawned {
            Ok(_) => self.develop.neighbour_prefetch.running = Some(cancelled),
            Err(error) => log::warn!("could not start the neighbour prefetch: {error}"),
        }
    }

    /// Stops the running prefetch after its current step. A decode already in
    /// flight still completes into the cache, where an open of the same photo
    /// waits for it.
    pub(in crate::app) fn cancel_neighbour_prefetch(&mut self) {
        if let Some(cancelled) = self.develop.neighbour_prefetch.running.take() {
            cancelled.store(true, Ordering::Release);
        }
    }
}
