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
    #[cfg(target_os = "android")]
    pub(super) review: crate::sidecar::PhotoReview,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SidecarSaveJob {
    pub(super) generation: u64,
    pub(super) revision: u64,
    pub(super) explicit: bool,
}

pub(crate) struct SidecarSaveEvent {
    pub(super) job: SidecarSaveJob,
    pub(super) result: Result<String, crate::sidecar::SidecarError>,
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

#[derive(Clone, Copy, Debug)]
pub(crate) struct SidecarAutosaveDeadline {
    pub(super) generation: u64,
    pub(super) due_at: Instant,
}

pub(crate) struct PersistenceState {
    pub(crate) history: EditHistory,
    pub(crate) lens_restore_masks: Option<MaskStack>,
    pub(crate) sidecar_target: Option<crate::sidecar::SidecarTarget>,
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
    pub(crate) sidecar_autosave_deadline: Option<SidecarAutosaveDeadline>,
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
