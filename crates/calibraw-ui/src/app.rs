use crate::appearance::{PreviewBackdrop, UiDesign};
use crate::pipeline::RawThumbnail;
use crate::pipeline::{
    affected_stage, apply_lens_correction, build_proxy, build_region_proxy_until,
    is_unsupported_raw_error, load_raw_file_with_profile_selection, preview_mask_layer_capacity,
    spawn_tiled_export, BrushMode, CameraProfileMode, ExportEvent, ExportFormat, ExportMetadata,
    ExportSettings, ExposureParams, GeometryTransform, GpuParams, GpuProgramPrewarm,
    LensCorrection, LensfunCatalog, LensfunCorrections, LensfunLens, LoadedRaw,
    ManualLensCorrection, MaskGeometry, MaskImage, MaskKind, MaskRgbImage, MaskStack,
    MobilePreviewRole, PipelineOptions, PreviewResourceShape, ProcessingQuality, ProcessingStage,
    ProxySpec, RawGpuPipeline, RawGpuProgramTemplate, RemoveBrushPoint, RemoveBrushStroke,
    RemoveEditState, RemoveSceneContext, RetouchAlignment, RetouchStroke, RetouchTool,
    SubjectRefinement, TileSpec, TiledExportJob, MAX_LOCAL_MASKS,
};
use crate::platform::{Platform, PlatformCaps};
use crate::sidecar::{
    AdjustmentCopySettings, AdjustmentPasteMode, EditSelection, EditState as SidecarEditState,
    LensEditState as SidecarLensEditState,
};
use crate::ui::develop::Develop;
use crate::ui::library::{AdjustmentClipboard, Library, LibraryState};
use crate::ui::settings::Settings;
use crate::ui::sidebar::Sidebar;
use crate::ui::top_bar::TopBar;
use calibraw_ai::models::masks::{spawn_ai_mask, AiMaskEvent, AiMaskWorkerRequest};
use calibraw_ai::models::object::{
    spawn_object_mask, ObjectInferenceCache, ObjectMaskEvent, ObjectMaskRequest,
    ObjectMaskWorkerRequest, SAM21_MODEL_BYTES_ESTIMATE,
};
use calibraw_ai::models::people::{PeopleAnalysis, PeopleMaskEvent};
use calibraw_ai::models::remove::{
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
mod discord_presence;
use discord_presence::DiscordPresence;
mod edit_history;
use edit_history::EditHistory;
mod version_update;
pub(crate) use version_update::{GITHUB_UPDATE_CHECKS_AVAILABLE, UPDATE_STORE_NAME};
mod worker;
use worker::drain_worker_events;
use worker::spawn_ui_worker;
use worker::{model_download_progress, poll_ai_job, AiJobUpdate};

mod decoded_raw_cache;
mod develop_cards;
pub(crate) use develop_cards::{DevelopCard, DevelopCardOrder};
mod develop_state;
mod effect_handles;
mod export_state;
mod job_state;
mod mask_properties;
mod mask_state;
mod mask_strip;
mod mask_tool;
mod persistence_state;
mod preview_state;
mod ui_state;
pub(crate) use decoded_raw_cache::{DecodeKey, DecodeRequest, DecodedRaw, DecodedRawCache};
pub(crate) use develop_state::*;
pub(crate) use effect_handles::{
    EffectComponentRef, EffectHandle, EffectHandleAction, EffectHandleEdit,
};
pub(crate) use export_state::*;
pub(crate) use job_state::*;
pub(crate) use mask_properties::{
    apply_mask_property_actions, MaskPropertiesControls, MaskPropertyAction, PeopleChoices,
    PeopleEdit, PersonThumbnail,
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
    pub(crate) luts: LutState,
    pub(crate) ui: UiState,
    discord_presence: DiscordPresence,
    /// The desktop toolbar's brand mark; `None` where there is no toolbar brand.
    pub(crate) toolbar_brand_texture: Option<egui::TextureHandle>,
    egui_ctx: egui::Context,
    foreground_operation: Option<ForegroundOperation>,
    pub(crate) android: AndroidState,
    pub(crate) platform: Arc<dyn Platform>,
}

impl CalibRawApp {
    /// What this device offers; views and handlers branch on it.
    pub(crate) fn caps(&self) -> PlatformCaps {
        self.platform.caps()
    }

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
            self.platform.release_free_memory();
        }
        if self.ui.active_tab == AppTab::Library && tab != AppTab::Library {
            self.library.prepare_for_develop();
            if self.caps().is_mobile() {
                self.library.set_folder_sidebar_open(false);
            }
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

    /// Whether the app consumes system back presses: outside the library, or
    /// while the library has a selection or an open folder sidebar to dismiss.
    pub(crate) fn handles_back_navigation(&self) -> bool {
        self.ui.active_tab != AppTab::Library
            || self.library.has_selection()
            || self.library.folder_sidebar_open()
    }

    fn clear_android_original_hold(&mut self) {
        self.preview.original_hold = None;
    }

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
        self.preview.interactive.reset();
    }

    /// `None` when the platform has no clipboard of its own and egui's is used.
    pub(crate) fn copy_text_to_clipboard(
        &self,
        label: &str,
        text: &str,
    ) -> Option<Result<(), String>> {
        self.platform.copy_text_to_clipboard(label, text)
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
mod background_tasks;
mod eframe_impl;
mod error_dialogs;
pub(crate) use error_dialogs::{ErrorDialogQueue, ErrorKind};
mod foreground;
mod gpu_recovery;
mod hover_preview;
mod inpainting;
mod library_adjustments;
mod luts;
mod manual_lens;
mod presets;
pub(crate) use library_adjustments::{
    EditTransfer, EditTransferOrigin, LibraryEditTransferOutcome,
};
pub(crate) use luts::{missing_look_lut_notice, LutState, LOOK_LUT_CARD_TITLE};
pub(crate) use presets::{GroupDialogMode, PresetEditor, PresetEditorMode, PresetState};
mod full_resolution;
mod interaction;
mod interactive_preview;
mod lifecycle;
mod preview_clipping;
mod preview_dispatch_gate;
mod preview_histogram;
mod preview_texture;
mod preview_viewport;
pub(crate) use full_resolution::FullResolutionState;
pub(crate) use interactive_preview::{shown_pipeline, InteractiveState};
pub(crate) use preview_dispatch_gate::{Batch, PreviewDispatchGate};
pub(crate) use preview_texture::{PreviewPipeline, TextureRetirement};
pub(crate) use preview_viewport::PreviewViewportAction;
mod processing_export;
mod straighten;
use straighten::AutoStraightenResult;
mod tips;
pub(crate) use tips::{ShownTip, TipPreferences};
mod unsupported_gpu;
pub(crate) use unsupported_gpu::create_app;
mod sidecar_persistence;
#[cfg(test)]
mod ui_review_tests;

use ai::{AiUpdate, PeopleState};
use lifecycle::needs_canonical_mask_source;
pub(crate) use lifecycle::{
    install_missing_range_sources, masks_have_missing_range_sources, DocumentSource,
};
pub(crate) use lifecycle::{CameraProfileChoice, ProfileReload};
use sidecar_persistence::sidecar_interaction_active;

pub(crate) mod preview_visibility;
