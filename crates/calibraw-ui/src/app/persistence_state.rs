//! Sidecar saving, developed-thumbnail jobs and usage time.

use super::*;

#[derive(Clone)]
pub(crate) struct SidecarSaveRequest {
    pub(super) target: crate::sidecar::SidecarTarget,
    pub(super) generation: u64,
    pub(super) revision: u64,
    pub(super) explicit: bool,
    pub(super) edits: SidecarEditState,
    pub(super) editing_time_ms: u64,
    /// Media-library sidecars carry the review; folder sidecars keep theirs on disk.
    pub(super) review: crate::sidecar::PhotoReview,
    /// The review changed, so a media-library photo saves it to its XMP
    /// first, as the rating saver does for other photos.
    pub(super) review_changed: bool,
    pub(super) start: crate::sidecar::SidecarStart,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SidecarSaveJob {
    pub(super) generation: u64,
    pub(super) revision: u64,
    pub(super) explicit: bool,
    /// The save carries a changed review to the photo's XMP
    /// ([`SidecarSaveRequest::review_changed`]).
    pub(super) review_changed: bool,
}

pub(crate) struct SidecarSaveEvent {
    pub(super) job: SidecarSaveJob,
    pub(super) target: crate::sidecar::SidecarTarget,
    pub(super) result: Result<crate::sidecar::SavedSidecar, crate::sidecar::SidecarError>,
    pub(super) recovery: Option<SidecarSaveRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DevelopedThumbnailJob {
    pub(super) target: crate::sidecar::SidecarTarget,
    pub(super) generation: u64,
    pub(super) revision: u64,
}

pub(crate) struct DevelopedThumbnailEvent {
    pub(super) job: DevelopedThumbnailJob,
    pub(super) result: Result<crate::pipeline::RawThumbnail, String>,
}

/// Whether the photo's cached developed thumbnail still matches its saved
/// sidecar, asked off the UI thread: on Android the sidecar read can wait on
/// a storage provider, and the event thread must keep answering lifecycle
/// callbacks. `None` when there is no current cached thumbnail.
pub(crate) struct DevelopedThumbnailCheck {
    pub(super) job: DevelopedThumbnailJob,
    pub(super) receiver: mpsc::Receiver<Result<Option<crate::pipeline::RawThumbnail>, String>>,
}

/// The start the open photo's saves record
/// ([`crate::sidecar::EditFingerprint`]): how it looked once opened, before
/// anyone changed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DocumentStart {
    /// An unedited photo is still opening, so what is saved now, such as the
    /// new-photo preset, is its start.
    Opening,
    Settled(crate::sidecar::SidecarStart),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SidecarAutosaveDeadline {
    pub(super) generation: u64,
    pub(super) due_at: Instant,
}

pub(crate) struct PersistenceState {
    pub(crate) history: EditHistory,
    pub(crate) lens_restore_masks: Option<MaskStack>,
    pub(crate) sidecar_target: Option<crate::sidecar::SidecarTarget>,
    pub(crate) document_start: DocumentStart,
    /// Identifies the open document. It increases whenever another document
    /// is installed, and document-bound jobs record the value they started with.
    pub(crate) document_generation: u64,
    pub(crate) sidecar_saved_revision: Option<u64>,
    pub(crate) sidecar_failed_revision: Option<u64>,
    pub(crate) sidecar_pending: VecDeque<SidecarSaveRequest>,
    pub(crate) sidecar_in_flight: Option<SidecarSaveJob>,
    pub(crate) sidecar_receiver: Option<mpsc::Receiver<SidecarSaveEvent>>,
    pub(crate) sidecar_save_feedback_until: Option<Instant>,
    pub(crate) sidecar_save_error_dialog: Option<String>,
    pub(crate) sidecar_recovery: Option<SidecarSaveRequest>,
    /// The open document's generation when a failed save of its changed
    /// review may not have brought it to its XMP. Its next saves carry the
    /// review to XMP again until one succeeds; otherwise they would put the
    /// review in CalibRaw's sidecar alone, and the older one in XMP, which
    /// wins, would return when it is opened again.
    pub(crate) review_xmp_unsaved: Option<u64>,
    pub(crate) sidecar_autosave_deadline: Option<SidecarAutosaveDeadline>,
    pub(crate) developed_thumbnail_check: Option<DevelopedThumbnailCheck>,
    pub(crate) developed_thumbnail_pending: Option<DevelopedThumbnailJob>,
    pub(crate) developed_thumbnail_in_flight: Option<DevelopedThumbnailJob>,
    pub(crate) developed_thumbnail_receiver: Option<mpsc::Receiver<DevelopedThumbnailEvent>>,
}

pub(crate) struct UsageState {
    pub(crate) app_persisted: Duration,
    pub(crate) app_started_at: Instant,
    pub(crate) raw_accumulated: Duration,
    pub(crate) raw_active_since: Option<Instant>,
}
