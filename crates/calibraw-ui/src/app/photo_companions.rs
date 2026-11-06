//! AI results kept beside a photo. Native code writes them to a local file, so
//! a photo in a linked folder needs them copied there afterwards.

use super::*;
use crate::i18n::tr_fmt;
use crate::sidecar::{PhotoCompanion, SidecarTarget};

pub(super) fn ai_denoise_not_saved_message(error: &str) -> String {
    tr_fmt(
        "AI denoise applied, but its result could not be saved next to the photo ({error}). It will need to run again the next time the photo is opened.",
        &[("error", &error)],
    )
}

pub(super) fn people_not_saved_message(reason: &str) -> String {
    tr_fmt(
        "The people in this photo could not be saved next to it ({reason}). They \
         will be found again the next time the photo is opened.",
        &[("reason", &reason)],
    )
}

impl CalibRawApp {
    /// Copies the open photo's new AI result to where the photo is, without
    /// holding the UI: the file can be large and the folder slow.
    pub(in crate::app) fn publish_open_photo_companion(&mut self, companion: PhotoCompanion) {
        let Some(SidecarTarget::Android {
            raw_uri,
            display_name,
        }) = self.persistence.sidecar_target.clone()
        else {
            return;
        };
        let platform = Arc::clone(&self.platform);
        let receiver = spawn_ui_worker(&self.egui_ctx, move || {
            platform.publish_photo_companion(&raw_uri, &display_name, companion)
        });
        self.android.companion_publishes.push((companion, receiver));
    }

    pub(in crate::app) fn poll_photo_companion_publishes(&mut self) {
        let mut failed = Vec::new();
        self.android
            .companion_publishes
            .retain(|(companion, receiver)| match receiver.try_recv() {
                Ok(Ok(())) => false,
                Ok(Err(error)) => {
                    failed.push((*companion, error));
                    false
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => true,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => false,
            });
        for (companion, error) in failed {
            match companion {
                PhotoCompanion::AiDenoise => self.report_error(
                    ErrorKind::AiDenoiseNotSaved,
                    ai_denoise_not_saved_message(&error),
                ),
                PhotoCompanion::PeopleAnalysis => {
                    self.report_error(ErrorKind::PeopleNotSaved, people_not_saved_message(&error))
                }
            }
        }
    }
}
