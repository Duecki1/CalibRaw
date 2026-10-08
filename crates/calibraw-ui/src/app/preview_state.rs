//! State of the develop preview: quality, detail and navigation renders, crop drags.

use super::*;

#[cfg(target_os = "android")]
#[derive(Clone, Copy, Debug)]
pub(crate) struct AndroidOriginalHold {
    pub start: egui::Pos2,
    pub started_at: Instant,
    pub phase: AndroidOriginalHoldPhase,
}

#[cfg(target_os = "android")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AndroidOriginalHoldPhase {
    /// The finger is down and still; the original shows once the hold time passes.
    Waiting,
    ShowingOriginal,
    /// The original was shown and the finger then moved. The touch stays
    /// with the hold until it lifts, so the cancelled tool gesture cannot
    /// resume mid-press.
    Moved,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PreviewQuality {
    Low,
    #[default]
    Medium,
    High,
    Max,
}

impl PreviewQuality {
    pub(crate) fn bounded_source_edge(width: u32, height: u32, requested: u32) -> u32 {
        if cfg!(target_os = "android") {
            RawGpuPipeline::bounded_mobile_preview_edge(width, height, requested)
        } else {
            requested.min(width.max(height))
        }
    }

    pub(crate) const fn pixel_scale(self) -> f32 {
        match self {
            Self::Low => 0.75,
            Self::Medium => 1.00,
            Self::High => 1.25,
            Self::Max => 1.50,
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Max => "Max",
        }
    }

    pub(crate) const fn proxy_edge(self) -> u32 {
        match self {
            Self::Low => 640,
            Self::Medium => 800,
            Self::High => 1024,
            Self::Max => 1280,
        }
    }

    fn edge_for_scale(self, viewport_pixels: [u32; 2], support_scale: f32) -> u32 {
        const CFA_PHASE_GUARD: u32 = 6;
        let viewport_edge = viewport_pixels[0].max(viewport_pixels[1]).max(1) as f64;
        let requested = (viewport_edge * f64::from(self.pixel_scale() * support_scale)).ceil();
        self.proxy_edge()
            .max(requested.min(f64::from(u32::MAX - CFA_PHASE_GUARD)) as u32 + CFA_PHASE_GUARD)
    }

    pub(crate) fn proxy_edge_for_viewport(self, viewport_pixels: [u32; 2]) -> u32 {
        self.edge_for_scale(viewport_pixels, 1.0)
    }

    pub(crate) fn proxy_edge_for_fitted_source(
        self,
        viewport_pixels: [u32; 2],
        source_width: u32,
        source_height: u32,
        geometry: GeometryTransform,
    ) -> u32 {
        const CFA_PHASE_GUARD: u32 = 6;
        let source_width = source_width.max(1);
        let source_height = source_height.max(1);
        let source_edge = source_width.max(source_height);
        let (display_width, display_height) =
            geometry.crop_pixel_dimensions(source_width, source_height);
        let fit_scale = (f64::from(viewport_pixels[0].max(1)) / f64::from(display_width.max(1)))
            .min(f64::from(viewport_pixels[1].max(1)) / f64::from(display_height.max(1)));
        let requested = (f64::from(source_edge) * fit_scale * f64::from(self.pixel_scale()))
            .ceil()
            .min(f64::from(u32::MAX - CFA_PHASE_GUARD)) as u32
            + CFA_PHASE_GUARD;
        Self::bounded_source_edge(
            source_width,
            source_height,
            self.proxy_edge().max(requested),
        )
    }

    pub(crate) fn detail_edge_for_viewport(self, viewport_pixels: [u32; 2]) -> u32 {
        self.edge_for_scale(viewport_pixels, 1.35)
    }

    pub(crate) const fn detail_pixel_scale(self) -> f32 {
        self.pixel_scale()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CropHandle {
    Move,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CropDragState {
    pub handle: CropHandle,
    pub start: [f32; 2],
    pub crop: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct StraightenDragState {
    pub start: egui::Pos2,
    pub current: egui::Pos2,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PreviewUvRect {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OverlayRasterKey {
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub texture_width: u32,
    pub texture_height: u32,
}

pub(crate) struct PreviewNavigation {
    pub pipeline: PreviewPipeline,
    pub(super) raw: Arc<LoadedRaw>,
}

pub(crate) struct PreviewDetail {
    pub pipeline: PreviewPipeline,
    pub uv_rect: PreviewUvRect,
    pub texture_uv_rect: PreviewUvRect,
    pub revision: u64,
    pub(super) raw: Arc<LoadedRaw>,
    pub(super) source_origin: [u32; 2],
    pub(super) source_size: [u32; 2],
    pub(super) mask_source_region: [u32; 4],
    pub(super) mask_texture_extent: [u32; 2],
    pub(super) virtual_origin: [i32; 2],
    pub(super) virtual_full_size: [u32; 2],
    pub(super) full_source_size: [u32; 2],
    pub(super) processing_halo: u32,
}

pub(crate) struct LoadedPreview {
    pub(super) source_path: Option<PathBuf>,
    pub(super) raw_cache_key: String,
    pub(super) label: String,
    pub(super) original_raw: Arc<LoadedRaw>,
    pub(super) full_raw: Arc<LoadedRaw>,
    pub(super) preview_raw: Arc<LoadedRaw>,
    pub(super) pipeline: RawGpuPipeline,
    pub(super) review: crate::sidecar::PhotoReview,
    pub(super) rendered_exposure: ExposureParams,
    pub(super) rendered_masks: MaskStack,
    pub(super) remove: RemoveEditState,
    pub(super) ai_masks_need_update: bool,
    pub(super) mask_source: Option<MaskRgbImage>,
    pub(super) lens_correction: LensCorrectionState,
    pub(super) sidecar_target: crate::sidecar::SidecarTarget,
    pub(super) document_generation: u64,
    pub(super) sidecar_warning: Option<String>,
    pub(super) sidecar_needs_rewrite: bool,
    pub(super) editing_time_ms: u64,
    pub(super) selected_camera_profile: Option<PathBuf>,
    pub(super) geometry: GeometryTransform,
}

pub(crate) struct PreparedPreviewRebuild {
    pub(super) source_raw: Arc<LoadedRaw>,
    pub(super) preview_raw: Arc<LoadedRaw>,
    pub(super) quality: PreviewQuality,
    pub(super) requested_edge: u32,
    pub(super) ai_enabled: bool,
}

pub(crate) enum PreviewRebuildEvent {
    Finished(Result<PreparedPreviewRebuild, String>),
}

pub(crate) struct PreparedPreviewDetail {
    pub(super) source_raw: Arc<LoadedRaw>,
    pub(super) revision: u64,
    pub(super) quality: PreviewQuality,
    pub(super) visible: PreviewUvRect,
    pub(super) texture_uv_rect: PreviewUvRect,
    pub(super) source_origin: [u32; 2],
    pub(super) source_size: [u32; 2],
    pub(super) raw: Arc<LoadedRaw>,
    pub(super) processing_halo: u32,
}

pub(crate) enum PreviewDetailRebuildEvent {
    Finished(Result<PreparedPreviewDetail, String>),
}

pub(crate) struct PreviewState {
    pub(crate) clipping: preview_clipping::ClippingState,
    pub(crate) histogram: preview_histogram::HistogramState,
    pub(crate) gpu_pipeline: Option<PreviewPipeline>,
    pub(crate) program_template: Option<RawGpuProgramTemplate>,
    pub(crate) retired_textures: TextureRetirement,
    pub(crate) gpu_prewarm_receiver: Option<mpsc::Receiver<Result<RawGpuPipeline, String>>>,
    pub(crate) quality: PreviewQuality,
    pub(crate) zoom: f32,
    pub(crate) center: [f32; 2],
    pub(crate) visible_uv: PreviewUvRect,
    pub(crate) viewport_pixels: [u32; 2],
    pub(crate) source_axes_swapped: bool,
    pub(crate) motion_at: Option<Instant>,
    pub(crate) touch_navigation_active: bool,
    /// Space was held over the preview, so the primary button pans instead
    /// of driving the canvas tool. Ends when that button is released.
    pub(crate) space_pan_active: bool,
    pub(crate) revision: u64,
    pub(crate) detail: Option<PreviewDetail>,
    pub(crate) navigation: Option<PreviewNavigation>,
    pub(crate) detail_pending_stage: Option<ProcessingStage>,
    pub(crate) navigation_pending_stage: Option<ProcessingStage>,
    pub(crate) detail_urgent: bool,
    pub(crate) quality_dirty: bool,
    pub(crate) rebuild_receiver: Option<mpsc::Receiver<PreviewRebuildEvent>>,
    pub(crate) detail_rebuild_receiver: Option<mpsc::Receiver<PreviewDetailRebuildEvent>>,
    pub(crate) original_exposure: ExposureParams,
    pub(crate) original_requested: bool,
    pub(crate) original_rendered_state: Option<(bool, u64)>,
    #[cfg(target_os = "android")]
    pub(crate) original_hold: Option<AndroidOriginalHold>,
    pub(crate) pending_stage: Option<ProcessingStage>,
    // Keep white-balance updates exact and coalesce zoomed mask scrubs while
    // the preceding GPU render is still running.
    pub(crate) white_balance_refresh_pending: bool,
    pub(crate) interactive_render_ready: Arc<AtomicBool>,
    #[cfg(target_os = "android")]
    pub(crate) lens_original_cache: Option<(PreviewQuality, Arc<LoadedRaw>)>,
    #[cfg(target_os = "android")]
    pub(crate) lens_corrected_cache:
        Option<(LensfunLens, PreviewQuality, Arc<LoadedRaw>, Arc<LoadedRaw>)>,
}
