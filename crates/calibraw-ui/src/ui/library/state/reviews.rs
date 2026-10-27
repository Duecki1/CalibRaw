//! Saving ratings and flags in the background.
//!
//! One worker saves the requests in the order they were made, so a change to
//! many photos never blocks the interface and a later key press on the same
//! photo always wins. The grid shows a change at once; a saved review
//! replaces it once its photo has no later save waiting.

use super::*;
use crate::sidecar::PhotoReview;
use crate::ui::library::review::ReviewChange;

/// A review change to save for one photo.
pub(crate) struct ReviewSave {
    pub(crate) asset: LibraryAsset,
    pub(crate) change: ReviewChange,
    /// The open photo's editing time, saved with its review.
    pub(crate) editing_time_ms: Option<u64>,
}

struct ReviewSaved {
    asset_id: LibraryAssetId,
    display_name: String,
    /// The review the sidecar holds afterwards, when it could be read.
    review: Option<PhotoReview>,
    error: Option<String>,
}

#[derive(Default)]
pub(crate) struct ReviewWriter {
    /// Started with the first save; it ends when this writer is dropped.
    worker: Option<(mpsc::Sender<ReviewSave>, mpsc::Receiver<ReviewSaved>)>,
    /// Saves sent and not yet reported, per photo.
    pending: HashMap<LibraryAssetId, usize>,
}

impl ReviewWriter {
    fn sender(
        &mut self,
        platform: &Arc<dyn Platform>,
        context: &egui::Context,
    ) -> Result<&mpsc::Sender<ReviewSave>, String> {
        if self.worker.is_none() {
            let (request_sender, request_receiver) = mpsc::channel::<ReviewSave>();
            let (result_sender, result_receiver) = mpsc::channel();
            let platform = Arc::clone(platform);
            let repaint = context.clone();
            std::thread::Builder::new()
                .name("calibraw-review-writer".to_owned())
                .spawn(move || {
                    for request in request_receiver {
                        let saved = save_review(platform.as_ref(), &request);
                        if result_sender.send(saved).is_err() {
                            break;
                        }
                        repaint.request_repaint();
                    }
                })
                .map_err(|error| format!("Could not start saving ratings: {error}"))?;
            self.worker = Some((request_sender, result_receiver));
        }
        self.worker
            .as_ref()
            .map(|(sender, _)| sender)
            .ok_or_else(|| "Could not start saving ratings.".to_owned())
    }
}

fn save_review(platform: &dyn Platform, request: &ReviewSave) -> ReviewSaved {
    let asset = &request.asset;
    let (review, error) = match write_review(platform, request) {
        Ok(review) => (Some(review), None),
        Err(error) => (read_review(platform, asset).ok(), Some(error)),
    };
    ReviewSaved {
        asset_id: asset.id.clone(),
        display_name: asset.display_name.clone(),
        review,
        error,
    }
}

/// Applies the change to the review the sidecar holds, so the result is
/// right even where the library had not read it yet.
fn write_review(platform: &dyn Platform, request: &ReviewSave) -> Result<PhotoReview, String> {
    let asset = &request.asset;
    match &asset.locator {
        LibraryLocator::Desktop(path) => {
            let review = request.change.apply(read_review(platform, asset)?);
            match request.editing_time_ms {
                Some(editing_time_ms) => crate::sidecar::save_photo_review_with_editing_time(
                    path,
                    review,
                    editing_time_ms,
                ),
                None => crate::sidecar::save_photo_review(path, review),
            }
            .map_err(|error| error.to_string())?;
            Ok(review)
        }
        // A media-library sidecar is written whole, so its edits are kept.
        LibraryLocator::Android { uri } => {
            let sidecar = platform
                .load_sidecar(uri, &asset.display_name)
                .map_err(|error| error.to_string())?;
            let (edits, review, editing_time_ms) = sidecar.map_or_else(
                || {
                    (
                        crate::sidecar::default_edit_state(),
                        PhotoReview::default(),
                        0,
                    )
                },
                |sidecar| (sidecar.edits, sidecar.review, sidecar.editing_time_ms),
            );
            let review = request.change.apply(review);
            platform
                .save_sidecar_with_review_and_editing_time(
                    uri,
                    &asset.display_name,
                    edits,
                    review,
                    request.editing_time_ms.unwrap_or(editing_time_ms),
                )
                .map_err(|error| error.to_string())?;
            Ok(review)
        }
    }
}

fn read_review(platform: &dyn Platform, asset: &LibraryAsset) -> Result<PhotoReview, String> {
    match &asset.locator {
        LibraryLocator::Desktop(path) => crate::sidecar::load_photo_review(path),
        LibraryLocator::Android { uri } => platform
            .load_sidecar_review(uri, &asset.display_name)
            .map(Option::unwrap_or_default),
    }
    .map_err(|error| error.to_string())
}

impl LibraryState {
    /// Shows each change at once and saves it in the background.
    pub(crate) fn save_reviews(
        &mut self,
        saves: Vec<ReviewSave>,
        context: &egui::Context,
    ) -> Result<(), String> {
        if saves.is_empty() {
            return Ok(());
        }
        let mut resort = false;
        for save in &saves {
            if let Some(review) = self.review_of(&save.asset.id) {
                resort |= self.show_review(&save.asset.id, save.change.apply(review));
            }
        }
        if resort {
            self.sort_entries();
        }
        let sender = self.reviews.sender(&self.platform, context)?.clone();
        for save in saves {
            let asset_id = save.asset.id.clone();
            sender
                .send(save)
                .map_err(|_| "The rating saver stopped unexpectedly.".to_owned())?;
            *self.reviews.pending.entry(asset_id).or_default() += 1;
        }
        Ok(())
    }

    /// Shows saved reviews; returns the photos that could not be saved.
    pub(crate) fn poll_review_saves(&mut self) -> Vec<String> {
        let mut failures = Vec::new();
        let mut resort = false;
        while let Some((_, receiver)) = &self.reviews.worker {
            let saved = match receiver.try_recv() {
                Ok(saved) => saved,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.reviews.worker = None;
                    self.reviews.pending.clear();
                    failures.push("The rating saver stopped unexpectedly.".to_owned());
                    break;
                }
            };
            let settled = match self.reviews.pending.get_mut(&saved.asset_id) {
                Some(pending) if *pending > 1 => {
                    *pending -= 1;
                    false
                }
                _ => {
                    self.reviews.pending.remove(&saved.asset_id);
                    true
                }
            };
            if let Some(review) = saved.review.filter(|_| settled) {
                resort |= self.show_review(&saved.asset_id, review);
            }
            if let Some(error) = saved.error {
                failures.push(format!("{}: {error}", saved.display_name));
            }
        }
        if resort {
            self.sort_entries();
        }
        failures
    }

    /// Waits until every review save has finished or `deadline` passes, so
    /// closing the app right after a key press keeps the rating. Returns
    /// the photos that could not be saved.
    pub(crate) fn finish_review_saves(&mut self, deadline: Instant) -> Vec<String> {
        let mut failures = Vec::new();
        while !self.reviews.pending.is_empty() {
            let Some((_, receiver)) = &self.reviews.worker else {
                break;
            };
            let wait = deadline.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(wait) {
                Ok(saved) => {
                    self.reviews.pending.remove(&saved.asset_id);
                    if let Some(error) = saved.error {
                        failures.push(format!("{}: {error}", saved.display_name));
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    failures.push(format!(
                        "{} ratings were still being saved when the app closed.",
                        self.reviews.pending.len()
                    ));
                    break;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    failures.push("The rating saver stopped unexpectedly.".to_owned());
                    break;
                }
            }
        }
        failures
    }

    /// Whether a review save for the photo is still running, so a review
    /// read before it must not replace the one the grid shows.
    pub(super) fn review_save_pending(&self, asset_id: &LibraryAssetId) -> bool {
        self.reviews.pending.contains_key(asset_id)
    }
}
