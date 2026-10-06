//! Foreground operations, AI runtime state and remove/retouch jobs.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ForegroundOperationKind {
    /// A local-AI job; Remove runs in its own worker and never appears here.
    Ai(calibraw_ai::AiFeature),
    LensCorrection,
    AutoStraighten,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ForegroundProgressValue {
    Indeterminate,
    Units {
        completed: u64,
        total: u64,
        unit: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ForegroundProgress {
    pub(crate) value: ForegroundProgressValue,
    pub(crate) phase: String,
    pub(crate) detail: Option<String>,
}

impl ForegroundProgress {
    pub(crate) fn indeterminate(phase: impl Into<String>) -> Self {
        Self {
            value: ForegroundProgressValue::Indeterminate,
            phase: phase.into(),
            detail: None,
        }
    }

    pub(crate) fn units(
        completed: u64,
        total: u64,
        unit: impl Into<Option<String>>,
        phase: impl Into<String>,
    ) -> Self {
        Self {
            value: ForegroundProgressValue::Units {
                completed,
                total,
                unit: unit.into(),
            },
            phase: phase.into(),
            detail: None,
        }
    }

    pub(crate) fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

pub(super) enum ForegroundOperationReceiver {
    AiMask(mpsc::Receiver<AiMaskEvent>),
    Object(mpsc::Receiver<ObjectMaskEvent>),
    AiDenoise(mpsc::Receiver<calibraw_ai::ai_denoise::AiDenoiseEvent>),
    LensCorrection(mpsc::Receiver<LensCorrectionEvent>),
    AutoStraighten(mpsc::Receiver<AutoStraightenResult>),
}

pub(super) enum ForegroundOperationContext {
    AiMask,
    Object {
        target: AiMaskTarget,
        inference_started: bool,
    },
    AiDenoise,
    LensCorrection,
    /// The geometry the estimate was made for; flips and perspective change its meaning.
    AutoStraighten {
        geometry: GeometryTransform,
    },
}

pub(crate) struct ForegroundOperation {
    pub(super) kind: ForegroundOperationKind,
    pub(super) document_id: u64,
    pub(super) cancellation: Arc<std::sync::atomic::AtomicBool>,
    pub(super) progress: ForegroundProgress,
    pub(super) cancelling: bool,
    pub(super) receiver: ForegroundOperationReceiver,
    pub(super) context: ForegroundOperationContext,
}

#[cfg(not(target_os = "android"))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OnnxRuntimeMode {
    #[default]
    Automatic,
    Manual,
}

/// Why a local-AI job is about to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AiJobOrigin {
    /// The user asked for the result just now.
    Requested,
    /// Re-creates a result an edit already uses but that is not saved, e.g.
    /// AI denoise on a reopened image. The model has to run again, so the user
    /// confirms first.
    Restore,
}

/// A local-AI job waiting for the user to confirm it, either to accept a model
/// or runtime download or to re-run a model for a restored edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AiConsent {
    pub(crate) feature: calibraw_ai::AiFeature,
    pub(crate) runtime_download_needed: bool,
    pub(crate) origin: AiJobOrigin,
}

pub(crate) struct AiState {
    pub(crate) birefnet_quality: BiRefNetQuality,
    #[cfg(not(target_os = "android"))]
    pub(crate) subject_crop_refinement: bool,
    #[cfg(not(target_os = "android"))]
    pub(crate) gpu_acceleration: bool,
    /// Content results (AI masks, scene depth, range sources) were made from an
    /// image that has since changed. Persisted as `ai_masks_need_update`.
    pub(crate) update_needed: bool,
    pub(crate) update: Option<AiUpdate>,
    #[cfg(not(target_os = "android"))]
    pub(crate) runtime_mode: OnnxRuntimeMode,
    #[cfg(not(target_os = "android"))]
    pub(crate) runtime_path: Option<PathBuf>,
    #[cfg(not(target_os = "android"))]
    pub(crate) runtime_sha256: Option<String>,
    pub(crate) library_mask_refresh: Option<LibraryAiMaskRefreshState>,
    pub(crate) consent: Option<AiConsent>,
    pub(crate) object_pending_target: Option<(usize, usize)>,
    pub(crate) object_error_dialog: Option<String>,
    pub(crate) object_cache: Option<((usize, usize), ObjectInferenceCache)>,
    pub(crate) denoise_resume_pending: bool,
}

pub(crate) struct InpaintState {
    pub(crate) tool: InpaintTool,
    pub(crate) brush_size: f32,
    pub(crate) brush_hardness: f32,
    pub(crate) brush_opacity: f32,
    pub(crate) alignment: RetouchAlignment,
    pub(crate) source_point: Option<[f32; 2]>,
    pub(crate) source_pick_active: bool,
    /// The primary press that placed the source is still held; it must not
    /// continue as a stroke.
    pub(crate) source_placement_press: bool,
    pub(crate) aligned_offset: Option<[f32; 2]>,
    pub(crate) edits: Arc<RemoveEditState>,
    pub(crate) active_points: Vec<RemoveBrushPoint>,
    pub(crate) last_brush_uv: Option<[f32; 2]>,
    pub(crate) pending_brush: Option<RemoveBrushStroke>,
    pub(crate) pending_retouch: Option<RetouchStroke>,
    pub(crate) receiver: Option<mpsc::Receiver<RemoveEvent>>,
    pub(crate) cancellation: Option<Arc<AtomicBool>>,
    pub(crate) processing_progress: Option<ForegroundProgress>,
    pub(crate) hovered_stroke: Option<usize>,
    pub(crate) selected_stroke: Option<usize>,
    pub(crate) stroke_opacity_edit_pending: bool,
}
