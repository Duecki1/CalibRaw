//! State of the develop preview: quality, detail and navigation renders, crop drags.

use super::*;

#[derive(Clone, Copy, Debug)]
pub(crate) struct AndroidOriginalHold {
    pub start: egui::Pos2,
    pub started_at: Instant,
    pub phase: AndroidOriginalHoldPhase,
}

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
    pub(crate) fn bounded_source_edge(
        width: u32,
        height: u32,
        requested: u32,
        cfa_kind: crate::pipeline::CfaKind,
    ) -> u32 {
        if PlatformCaps::CURRENT.constrained_memory() {
            RawGpuPipeline::bounded_mobile_preview_edge(width, height, requested, cfa_kind)
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
        cfa_kind: crate::pipeline::CfaKind,
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
            cfa_kind,
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

impl PreviewUvRect {
    /// The whole image.
    pub(crate) const FULL: Self = Self {
        min: [0.0, 0.0],
        max: [1.0, 1.0],
    };
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
    /// Automatic lens correction was due, but no profile matched the RAW.
    pub(super) lens_profile_missing: bool,
    pub(super) sidecar_target: crate::sidecar::SidecarTarget,
    pub(super) document_generation: u64,
    pub(super) sidecar_warning: Option<String>,
    pub(super) sidecar_needs_rewrite: bool,
    pub(super) editing_time_ms: u64,
    pub(super) selected_camera_profile: Option<PathBuf>,
    pub(super) geometry: GeometryTransform,
    pub(super) starts_unedited: bool,
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
    /// The `zoom` at which one displayed photo pixel covers one physical
    /// screen pixel (100%, 1:1). `zoom` itself is relative to the fitted view.
    /// `None` until the preview has been laid out.
    pub(crate) native_zoom: Option<f32>,
    /// A view kept from the previous document, shown by the first layout of
    /// the new one, once its 100% zoom is known. See `reset_view_keeping`.
    pub(crate) pending_view: Option<KeptView>,
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
    pub(crate) original_hold: Option<AndroidOriginalHold>,
    pub(crate) pending_stage: Option<ProcessingStage>,
    pub(crate) dispatch_gate: PreviewDispatchGate,
    pub(crate) lens_original_cache: Option<(PreviewQuality, Arc<LoadedRaw>)>,
    pub(crate) lens_corrected_cache: Option<(
        LensCorrection,
        PreviewQuality,
        Arc<LoadedRaw>,
        Arc<LoadedRaw>,
    )>,
}

/// A zoomed-in view carried from one document to the next, so a photo switched
/// to from the filmstrip or reopened with another camera profile shows the
/// same magnification and position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct KeptView {
    /// Photo pixels per physical screen pixel (`zoom / native_zoom`), the
    /// scale the zoom readout shows: 1.0 is 100%. Unlike the fit-relative
    /// `zoom`, it means the same for photos of another size or crop.
    magnification: f32,
    /// The view centre, as `PreviewState::center`.
    center: [f32; 2],
}

impl KeptView {
    /// The fit-relative zoom and centre for a document whose 100% view is
    /// at `native_zoom`. The viewport clamps both, as for any navigation.
    pub(crate) fn navigation(self, native_zoom: f32) -> (f32, [f32; 2]) {
        (self.magnification * native_zoom, self.center)
    }
}

impl PreviewState {
    /// Fits the whole image in the view again and stops view motion. Bumping
    /// the revision rejects detail renders made for the previous view.
    pub(crate) fn reset_view(&mut self) {
        self.zoom = 1.0;
        self.center = [0.5, 0.5];
        self.visible_uv = PreviewUvRect::FULL;
        self.motion_at = None;
        self.touch_navigation_active = false;
        self.pending_view = None;
        self.revision = self.revision.wrapping_add(1);
    }

    /// Like `reset_view`, but the next layout shows `kept` instead of the
    /// fitted view when it is set.
    pub(crate) fn reset_view_keeping(&mut self, kept: Option<KeptView>) {
        self.reset_view();
        self.pending_view = kept;
    }

    /// The current view to carry to the next document, or `None` while the
    /// photo is fitted, so the next one is fitted too.
    pub(crate) fn kept_view(&self) -> Option<KeptView> {
        if self.pending_view.is_some() {
            // Not laid out yet: pass on the view the last switch kept.
            return self.pending_view;
        }
        if crate::ui::preview::zoom_is_fit(self.zoom) {
            return None;
        }
        let native_zoom = self
            .native_zoom
            .filter(|native| native.is_finite() && *native > 0.0)?;
        Some(KeptView {
            magnification: self.zoom / native_zoom,
            center: self.center,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zoomed_app(zoom: f32, native_zoom: f32) -> CalibRawApp {
        let mut app = CalibRawApp::empty(&egui::Context::default());
        app.preview.zoom = zoom;
        app.preview.center = [0.3, 0.6];
        app.preview.native_zoom = Some(native_zoom);
        app
    }

    #[test]
    fn a_fitted_view_is_not_kept() {
        let app = zoomed_app(1.0, 4.0);
        assert_eq!(app.preview.kept_view(), None);
    }

    #[test]
    fn a_kept_view_carries_the_magnification_until_it_is_laid_out() {
        let mut app = zoomed_app(2.0, 4.0);
        let kept = app.preview.kept_view().expect("a zoomed view is kept");
        assert_eq!(kept.navigation(8.0), (4.0, [0.3, 0.6]));

        let revision = app.preview.revision;
        app.preview.reset_view_keeping(Some(kept));
        assert_eq!(app.preview.zoom, 1.0);
        assert_ne!(app.preview.revision, revision);
        // Switching again before the layout passes the same view on.
        assert_eq!(app.preview.kept_view(), Some(kept));

        app.preview.reset_view();
        assert_eq!(app.preview.pending_view, None);
    }
}
