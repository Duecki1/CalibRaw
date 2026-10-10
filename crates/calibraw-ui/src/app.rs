use crate::appearance::{PreviewBackdrop, UiDesign};
use crate::pipeline::{
    affected_stage, apply_lensfun_correction, build_proxy, build_region_proxy,
    is_unsupported_raw_error, load_raw_file_with_profile_selection, spawn_tiled_export, BrushMode,
    CameraProfileMode, ExportEvent, ExportFormat, ExportMetadata, ExportSettings, ExposureParams,
    GeometryTransform, GpuParams, GpuProgramPrewarm, LensfunCatalog, LensfunCorrections,
    LensfunLens, LoadedRaw, MaskGeometry, MaskImage, MaskKind, MaskRgbImage, MaskStack,
    PipelineOptions, ProcessingQuality, ProcessingStage, ProxySpec, RawGpuPipeline,
    RawGpuProgramTemplate, RemoveBrushPoint, RemoveBrushStroke, RemoveEditState,
    RemoveSceneContext, RetouchAlignment, RetouchStroke, RetouchTool, SubjectRefinement, TileSpec,
    TiledExportJob, MAX_LOCAL_MASKS,
};
#[cfg(not(target_os = "android"))]
use crate::pipeline::{lensfun_catalog, RawThumbnail};
use crate::sidecar::{
    AdjustmentCopySettings, AdjustmentPasteMode, EditSelection, EditState as SidecarEditState,
    LensEditState as SidecarLensEditState,
};
#[cfg(not(target_os = "android"))]
use crate::ui::develop::Develop;
use crate::ui::library::{AdjustmentClipboard, Library, LibraryState};
use crate::ui::settings::Settings;
use crate::ui::sidebar::Sidebar;
use crate::ui::top_bar::TopBar;
use calibraw_ai::ai_masks::{
    spawn_ai_mask, spawn_object_mask, AiMaskEvent, AiMaskWorkerRequest, BiRefNetQuality,
    ObjectInferenceCache, ObjectMaskEvent, ObjectMaskRequest, ObjectMaskWorkerRequest,
    SAM21_MODEL_BYTES_ESTIMATE,
};
use calibraw_ai::remove::{
    spawn_remove, spawn_retouch, RemoveEvent, RemoveRequest, RetouchRequest,
};
use eframe::{egui, wgpu};
use moduwu_design::slider_scroll_locked;
use moduwu_design::ScreenLayout;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

mod actions;
pub(crate) use actions::AppAction;
mod ai_denoise;
#[cfg(not(target_os = "android"))]
mod discord_presence;
#[cfg(not(target_os = "android"))]
use discord_presence::DiscordPresence;
mod edit_history;
use edit_history::EditHistory;
mod version_update;
pub(crate) use version_update::{GITHUB_UPDATE_CHECKS_AVAILABLE, UPDATE_STORE_NAME};
mod worker;
use worker::drain_worker_events;
#[cfg(not(target_os = "android"))]
use worker::spawn_ui_worker;

mod decoded_raw_cache;
mod develop_state;
mod export_state;
mod job_state;
mod mask_properties;
mod mask_state;
mod mask_strip;
mod mask_tool;
mod persistence_state;
mod preview_state;
mod ui_state;
pub(crate) use decoded_raw_cache::{DecodeKey, DecodedRaw, DecodedRawCache};
pub(crate) use develop_state::*;
pub(crate) use export_state::*;
pub(crate) use job_state::*;
pub(crate) use mask_properties::{
    apply_mask_property_actions, MaskPropertiesControls, MaskPropertyAction,
};
pub(crate) use mask_state::*;
pub(crate) use mask_strip::{MaskStripActions, MaskStripCommand, MaskStripEdit};
pub(crate) use mask_tool::{BrushStrokeSamples, MaskPointerEdit, MaskToolAction};
pub(crate) use persistence_state::*;
pub(crate) use preview_state::*;
pub(crate) use ui_state::*;

pub struct CalibRawApp {
    pub(crate) develop: DevelopState,
    pub(crate) preview: PreviewState,
    pub(crate) develop_ui: DevelopUiState,
    pub(crate) library: LibraryState,
    pub(crate) masks: MaskState,
    pub(crate) ai: AiState,
    pub(crate) inpaint: InpaintState,
    pub(crate) export: ExportState,
    pub(crate) persistence: PersistenceState,
    pub(crate) usage: UsageState,
    pub(crate) preferences: PreferencesState,
    pub(crate) presets: PresetState,
    pub(crate) ui: UiState,
    #[cfg(not(target_os = "android"))]
    discord_presence: DiscordPresence,
    #[cfg(not(target_os = "android"))]
    pub(crate) toolbar_brand_texture: egui::TextureHandle,
    egui_ctx: egui::Context,
    foreground_operation: Option<ForegroundOperation>,
    #[cfg(target_os = "android")]
    pub(crate) android: AndroidState,
}

impl CalibRawApp {
    pub(crate) fn app_usage_duration(&self) -> Duration {
        self.usage
            .app_persisted
            .saturating_add(self.usage.app_started_at.elapsed())
    }

    /// The format of the open photo when it is a rendered JPEG, PNG or HEIC
    /// rather than a RAW. Android documents only carry their name in the label.
    pub(crate) fn current_rendered_format(&self) -> Option<crate::pipeline::RenderedImageFormat> {
        self.develop
            .current_label
            .as_deref()
            .map(std::path::Path::new)
            .or(self.develop.current_path.as_deref())
            .and_then(crate::pipeline::RenderedImageFormat::from_path)
    }

    pub(crate) fn raw_edit_duration(&self) -> Duration {
        self.usage.raw_accumulated.saturating_add(
            self.usage
                .raw_active_since
                .map(|started| started.elapsed())
                .unwrap_or_default(),
        )
    }

    pub(crate) fn raw_editing_time_ms(&self) -> u64 {
        duration_millis_saturating(self.raw_edit_duration())
    }

    pub(in crate::app) fn install_raw_edit_timer(&mut self, editing_time_ms: u64) {
        self.usage.raw_accumulated = Duration::from_millis(editing_time_ms);
        self.usage.raw_active_since = self.raw_edit_timer_should_run().then(Instant::now);
    }

    pub(in crate::app) fn clear_raw_edit_timer(&mut self) {
        self.usage.raw_accumulated = Duration::ZERO;
        self.usage.raw_active_since = None;
    }

    fn pause_raw_edit_timer(&mut self) {
        let Some(started) = self.usage.raw_active_since.take() else {
            return;
        };
        self.usage.raw_accumulated = self.usage.raw_accumulated.saturating_add(started.elapsed());
    }

    fn resume_raw_edit_timer(&mut self) {
        if self.usage.raw_active_since.is_none() && self.raw_edit_timer_should_run() {
            self.usage.raw_active_since = Some(Instant::now());
        }
    }

    fn raw_edit_timer_should_run(&self) -> bool {
        self.develop.loaded_raw.is_some() && self.ui.active_tab == AppTab::Develop
    }

    pub(crate) fn activate_tab(&mut self, tab: AppTab) {
        if self.ui.active_tab == tab {
            return;
        }
        if self.ui.active_tab == AppTab::Develop && tab != AppTab::Develop {
            self.pause_raw_edit_timer();
            self.set_original_preview_requested(false);
            self.clear_android_original_hold();
        }
        if self.ui.active_tab == AppTab::Library && tab != AppTab::Library {
            self.library.prepare_for_develop();
            #[cfg(target_os = "android")]
            self.library.set_folder_sidebar_open(false);
        }
        if tab == AppTab::Settings {
            self.ui.thumbnail_cache_size = None;
            self.ui.thumbnail_cache_size_receiver = None;
        }
        self.ui.active_tab = tab;
        if tab == AppTab::Develop {
            self.resume_raw_edit_timer();
        }
        self.sync_ai_runtime();
    }

    /// Whether the app consumes Android back presses: outside the library, or
    /// while the library has a selection or an open folder sidebar to dismiss.
    #[cfg(any(target_os = "android", test))]
    pub(crate) fn handles_back_navigation(&self) -> bool {
        self.ui.active_tab != AppTab::Library
            || self.library.has_selection()
            || self.library.folder_sidebar_open()
    }

    #[cfg(target_os = "android")]
    fn clear_android_original_hold(&mut self) {
        self.preview.original_hold = None;
    }

    #[cfg(not(target_os = "android"))]
    fn clear_android_original_hold(&mut self) {}

    fn release_retired_egui_textures(&mut self, frame: &eframe::Frame) {
        if self.preview.retired_textures.is_empty() {
            return;
        }
        let Some(render_state) = frame.wgpu_render_state() else {
            return;
        };
        self.preview
            .retired_textures
            .release(&mut render_state.renderer.write());
    }

    /// Registers `pipeline`'s output for display; its texture is retired when
    /// the returned value is dropped.
    pub(crate) fn present_pipeline(
        &self,
        pipeline: RawGpuPipeline,
        render_state: &eframe::egui_wgpu::RenderState,
    ) -> PreviewPipeline {
        PreviewPipeline::register(
            pipeline,
            &render_state.device,
            &mut render_state.renderer.write(),
            &self.preview.retired_textures,
        )
    }

    fn take_preview_pipeline_and_release_textures(&mut self) -> Option<RawGpuPipeline> {
        let pipeline = self
            .preview
            .gpu_pipeline
            .take()
            .map(PreviewPipeline::into_gpu);
        if let Some(pipeline) = pipeline.as_ref() {
            self.preview.program_template = Some(pipeline.program_template());
        }
        self.discard_auxiliary_previews();
        pipeline
    }

    fn discard_auxiliary_previews(&mut self) {
        self.preview.detail = None;
        self.preview.navigation = None;
    }

    #[cfg(target_os = "android")]
    pub(crate) fn copy_text_to_clipboard(&self, label: &str, text: &str) -> Result<(), String> {
        calibraw_ffi::copy_text_to_clipboard(&self.android.android_app, label, text)
    }
}

pub(crate) fn duration_millis_saturating(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

pub(crate) fn format_usage_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let days = total_seconds / 86_400;
    let hours = (total_seconds / 3_600) % 24;
    let minutes = (total_seconds / 60) % 60;
    let seconds = total_seconds % 60;
    if days > 0 {
        format!("{days}d {hours:02}h {minutes:02}m {seconds:02}s")
    } else if hours > 0 {
        format!("{hours}h {minutes:02}m {seconds:02}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

mod ai;
mod eframe_impl;
mod error_dialogs;
pub(crate) use error_dialogs::{ErrorDialogQueue, ErrorKind};
mod foreground;
mod inpainting;
mod library_adjustments;
mod presets;
pub(crate) use library_adjustments::{
    EditTransfer, EditTransferOrigin, LibraryEditTransferOutcome,
};
pub(crate) use presets::{GroupDialogMode, PresetEditor, PresetEditorMode, PresetState};
mod lifecycle;
mod preview_clipping;
mod preview_histogram;
#[cfg(all(test, not(target_os = "android")))]
mod preview_tests;
mod preview_texture;
mod preview_viewport;
pub(crate) use preview_texture::{PreviewPipeline, TextureRetirement};
pub(crate) use preview_viewport::PreviewViewportAction;
mod processing_export;
mod straighten;
use straighten::AutoStraightenResult;
mod sidecar_persistence;
#[cfg(all(test, not(target_os = "android")))]
mod ui_review_tests;

use ai::AiUpdate;
use lifecycle::needs_canonical_mask_source;
pub(crate) use lifecycle::ProfileReload;
#[cfg(not(target_os = "android"))]
pub(crate) use lifecycle::{
    install_missing_range_sources, masks_have_missing_range_sources, DocumentSource,
};
use sidecar_persistence::sidecar_interaction_active;

#[cfg(test)]
mod tests {
    use super::{AiMaskTarget, MaskGeometry, MaskKind, MaskStack, MaskState, PreviewQuality};
    use crate::pipeline::GeometryTransform;

    #[test]
    fn back_presses_are_handled_outside_the_plain_library() {
        let context = eframe::egui::Context::default();
        let mut app = super::CalibRawApp::empty(&context);
        app.ui.active_tab = super::AppTab::Develop;
        assert!(app.handles_back_navigation());

        app.ui.active_tab = super::AppTab::Library;
        app.library.set_folder_sidebar_open(false);
        assert!(!app.handles_back_navigation());
        app.library.set_folder_sidebar_open(true);
        assert!(app.handles_back_navigation());
        app.library.set_folder_sidebar_open(false);
        assert!(!app.handles_back_navigation());
    }

    #[test]
    #[cfg(not(target_os = "android"))]
    fn preview_quality_levels_track_physical_viewport_density() {
        assert_eq!(
            PreviewQuality::Max.proxy_edge_for_viewport([3_000, 2_000]),
            4_506
        );
        assert!(PreviewQuality::Max.detail_edge_for_viewport([3_200, 1_800]) >= 3_200 * 2);
        for quality in [
            PreviewQuality::Low,
            PreviewQuality::Medium,
            PreviewQuality::High,
            PreviewQuality::Max,
        ] {
            assert!(quality.proxy_edge_for_viewport([3_840, 2_160]) > quality.proxy_edge());
        }
    }

    #[test]
    fn preview_quality_density_is_ordered_and_medium_matches_physical_pixels() {
        let viewport = [2_400, 1_600];
        let edges = [
            PreviewQuality::Low.proxy_edge_for_viewport(viewport),
            PreviewQuality::Medium.proxy_edge_for_viewport(viewport),
            PreviewQuality::High.proxy_edge_for_viewport(viewport),
            PreviewQuality::Max.proxy_edge_for_viewport(viewport),
        ];
        assert!(edges.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(edges[1], 2_406);
    }

    #[test]
    fn fitted_preview_density_excludes_phone_letterbox_space() {
        let geometry = GeometryTransform::default();
        let edge =
            PreviewQuality::Max.proxy_edge_for_fitted_source([720, 1_500], 7_028, 4_688, geometry);
        assert_eq!(edge, 1_280);

        let mut cropped = geometry;
        cropped.crop = [0.375, 0.0, 0.625, 1.0];
        assert!(
            PreviewQuality::Max.proxy_edge_for_fitted_source([720, 1_500], 7_028, 4_688, cropped,)
                > edge
        );
    }

    #[test]
    fn ai_mask_result_target_survives_reordering_but_not_replacement() {
        let mut stack = MaskStack::default();
        stack.add_mask(MaskKind::Object);
        let target = AiMaskTarget {
            mask_index: 0,
            component_index: 0,
            kind: MaskKind::Object,
            geometry: stack.masks[0].components[0].geometry.clone(),
        };
        stack.add_component(MaskKind::Brush, crate::pipeline::MaskCombineMode::Add);
        assert_eq!(stack.move_submask_component(0, 0, 0, 2), Some((0, 1)));
        assert_eq!(
            MaskState::resolve_ai_target_in_stack(&stack, &target),
            Ok((0, 1))
        );

        stack.masks[0].components[1].kind = MaskKind::Brush;
        stack.masks[0].components[1].geometry = MaskGeometry::for_kind(MaskKind::Brush);
        let error = MaskState::resolve_ai_target_in_stack(&stack, &target).unwrap_err();
        assert!(error.contains("changed type"));
    }
}

pub(crate) mod preview_visibility;
