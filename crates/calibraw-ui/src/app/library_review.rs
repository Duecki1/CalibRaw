//! Ratings and flags: applying a change to photos and saving it.

use super::error_dialogs::ErrorKind;
use super::CalibRawApp;
use crate::sidecar::PhotoReview;
use crate::ui::library::{LibraryAsset, LibraryLocator, ReviewChange, ReviewFeedback, ReviewSave};

impl CalibRawApp {
    /// Applies `change` to each photo. The open photo changes through its
    /// document, so its next save keeps the review; the others are saved in
    /// the background. Returns the result when there is one photo.
    pub(crate) fn review_photos(
        &mut self,
        assets: Vec<LibraryAsset>,
        change: ReviewChange,
    ) -> Option<PhotoReview> {
        let single = assets.len() == 1;
        let mut result = None;
        let mut saves = Vec::with_capacity(assets.len());
        for asset in assets {
            let current = self.library_asset_is_current(&asset);
            result = if current {
                self.develop.review = change.apply(self.develop.review);
                Some(self.develop.review)
            } else {
                self.library
                    .review_of(&asset.id)
                    .map(|review| change.apply(review))
            };
            // A media-library document is saved whole, review included.
            if current && matches!(asset.locator, LibraryLocator::Android { .. }) {
                self.library
                    .show_saved_review(&asset.id, self.develop.review);
                self.queue_explicit_sidecar_save();
            } else {
                saves.push(ReviewSave {
                    editing_time_ms: current.then(|| self.raw_editing_time_ms()),
                    asset,
                    change,
                });
            }
        }
        if let Err(error) = self.library.save_reviews(saves, &self.egui_ctx) {
            self.report_error(ErrorKind::PhotoReview, error);
        }
        result.filter(|_| single)
    }

    /// [`Self::review_photos`] for a rating or flag key, confirmed briefly
    /// on screen because the key gives no other sign of what it did.
    pub(crate) fn review_photos_from_keyboard(
        &mut self,
        assets: Vec<LibraryAsset>,
        change: ReviewChange,
    ) {
        let count = assets.len();
        if count == 0 {
            return;
        }
        let result = self.review_photos(assets, change);
        let now = self.egui_ctx.input(|input| input.time);
        self.ui.review_feedback = Some(ReviewFeedback::new(change, count, result, now));
    }

    /// Lets queued ratings reach their sidecars before the app closes; no
    /// dialog can show any more, so failures are logged.
    pub(in crate::app) fn finish_library_review_saves_on_exit(&mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        for failure in self.library.finish_review_saves(deadline) {
            log::error!("{}: {failure}", ErrorKind::PhotoReview.title());
        }
    }

    pub(in crate::app) fn poll_library_review_saves(&mut self) {
        let failures = self.library.poll_review_saves();
        if !failures.is_empty() {
            self.report_error(ErrorKind::PhotoReview, failures.join("\n"));
        }
    }
}
