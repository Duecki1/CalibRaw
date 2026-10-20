//! The Android backend: the JNI bridge in `calibraw-ffi`.

use super::service::{
    CameraProfileFolderResult, ExportPublishResult, ImportProgress, ImportedLibraryDocument,
    LibraryDocument, LibraryFolder, PickedDocument, PickerResult, Platform, PublishedExport,
    ReplayVideoEncoder, TaskNotification,
};
use super::PlatformCaps;
use crate::pipeline::RawThumbnail;
use crate::sidecar::{EditState, LoadedSidecar, PhotoReview, SidecarError};
use calibraw_ffi::AndroidApp;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The Android activity's entry point, called by `android-activity`.
#[unsafe(no_mangle)]
#[allow(
    unreachable_pub,
    reason = "found by its symbol name, which `no_mangle` exports"
)]
pub fn android_main(android_app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("CalibRaw"),
    );

    match calibraw_ffi::gpu_stage_timing_requested(&android_app) {
        Ok(true) => calibraw_gpu::stage_timing::request(),
        Ok(false) => {}
        Err(error) => log::warn!("{error}"),
    }
    let mut options = crate::native_options(PlatformCaps::ANDROID);
    options.wgpu_options.surface = eframe::egui_wgpu::SurfaceConfig::LOW_LATENCY;
    options.android_app = Some(android_app.clone());
    options.viewport = eframe::egui::ViewportBuilder::default();

    let platform = AndroidPlatform::new(android_app);
    let result = eframe::run_native(
        "CalibRaw",
        options,
        Box::new(move |cc| Ok(crate::app::create_app(cc, platform))),
    );
    if let Err(error) = result {
        log::error!("CalibRaw terminated: {error:#}");
    }
}

/// The program's `main` does nothing on Android; the activity starts
/// [`android_main`].
pub(crate) fn main() -> eframe::Result {
    Ok(())
}

pub(crate) struct AndroidPlatform {
    app: AndroidApp,
}

impl AndroidPlatform {
    pub(crate) fn new(app: AndroidApp) -> Arc<Self> {
        Arc::new(Self { app })
    }
}

impl Platform for AndroidPlatform {
    fn caps(&self) -> PlatformCaps {
        PlatformCaps::ANDROID
    }

    fn attach_ui(&self, request_repaint: Box<dyn Fn() + Send + Sync>) {
        calibraw_ffi::attach_ui(request_repaint);
    }

    fn detach_ui(&self) {
        calibraw_ffi::detach_ui();
    }

    fn set_back_navigation_active(&self, active: bool) {
        calibraw_ffi::set_back_navigation_active(active);
    }

    fn take_back_request(&self) -> bool {
        calibraw_ffi::take_back_request()
    }

    fn system_bar_insets_points(&self, pixels_per_point: f32) -> [f32; 4] {
        calibraw_ffi::system_bar_insets_points(pixels_per_point)
    }

    fn set_light_system_bars(&self, light: bool) -> Result<(), String> {
        calibraw_ffi::set_light_system_bars(&self.app, light)
    }

    fn device_diagnostics(&self) -> Result<Option<String>, String> {
        calibraw_ffi::device_diagnostics(&self.app).map(Some)
    }

    fn copy_text_to_clipboard(&self, label: &str, text: &str) -> Option<Result<(), String>> {
        Some(calibraw_ffi::copy_text_to_clipboard(&self.app, label, text))
    }

    fn update_task_notification(&self, notification: TaskNotification<'_>) -> Result<(), String> {
        calibraw_ffi::update_background_task_notification(
            &self.app,
            notification.title,
            notification.phase,
            notification.detail,
            notification.progress_percent,
            notification.indeterminate,
            notification.queued_count,
        )
    }

    fn clear_task_notification(&self) -> Result<(), String> {
        calibraw_ffi::clear_background_task_notification(&self.app)
    }

    fn release_free_memory(&self) {
        calibraw_ffi::release_free_heap();
    }

    fn performance_settings_path(&self) -> Result<Option<PathBuf>, String> {
        calibraw_ffi::performance_settings_path(&self.app).map(Some)
    }

    fn internal_data_dir(&self) -> Option<PathBuf> {
        self.app.internal_data_path()
    }

    fn model_root(&self) -> PathBuf {
        self.app
            .internal_data_path()
            .unwrap_or_else(std::env::temp_dir)
            .join("models")
    }

    fn gpu_pipeline_cache_dir(&self) -> Result<Option<PathBuf>, String> {
        calibraw_ffi::gpu_pipeline_cache_dir(&self.app).map(Some)
    }

    fn lensfun_database_dir(&self) -> Result<Option<PathBuf>, String> {
        calibraw_ffi::lensfun_database_dir(&self.app).map(Some)
    }

    fn open_raw_document(&self) -> Result<(), String> {
        calibraw_ffi::open_raw_document(&self.app)
    }

    fn take_picker_result(&self) -> Option<PickerResult> {
        calibraw_ffi::take_picker_result().map(picker_result)
    }

    fn take_external_open_result(&self) -> Option<PickerResult> {
        calibraw_ffi::take_external_open_result().map(picker_result)
    }

    fn has_external_open_result(&self) -> bool {
        calibraw_ffi::has_external_open_result()
    }

    fn take_import_progress(&self) -> Option<ImportProgress> {
        calibraw_ffi::take_import_progress().map(|progress| ImportProgress {
            completed: progress.completed,
            total: progress.total,
        })
    }

    fn take_camera_profile_folder_result(&self) -> Option<CameraProfileFolderResult> {
        calibraw_ffi::take_camera_profile_folder_result().map(|result| match result {
            calibraw_ffi::CameraProfileFolderResult::ImportStarted { label } => {
                CameraProfileFolderResult::ImportStarted { label }
            }
            calibraw_ffi::CameraProfileFolderResult::Picked {
                path,
                label,
                profiles,
            } => CameraProfileFolderResult::Picked {
                path,
                label,
                profiles,
            },
            calibraw_ffi::CameraProfileFolderResult::Cancelled => {
                CameraProfileFolderResult::Cancelled
            }
            calibraw_ffi::CameraProfileFolderResult::Failed(error) => {
                CameraProfileFolderResult::Failed(error)
            }
        })
    }

    fn take_export_publish_result(&self) -> Option<ExportPublishResult> {
        calibraw_ffi::take_export_publish_result().map(|result| match result {
            calibraw_ffi::ExportPublishResult::Published(export) => {
                ExportPublishResult::Published(published_export(export))
            }
            calibraw_ffi::ExportPublishResult::Failed(error) => ExportPublishResult::Failed(error),
        })
    }

    fn open_camera_profile_folder(&self) -> Result<(), String> {
        calibraw_ffi::open_camera_profile_folder(&self.app)
    }

    fn remove_camera_profile_mirror(&self, path: &Path) -> Result<(), String> {
        calibraw_ffi::remove_camera_profile_mirror(&self.app, path)
    }

    fn scavenge_camera_profile_mirrors(&self, active_mirror: Option<&Path>) -> Result<(), String> {
        calibraw_ffi::scavenge_camera_profile_mirrors(&self.app, active_mirror)
    }

    fn clear_camera_profile_folder_picker_location(&self) -> Result<(), String> {
        calibraw_ffi::clear_camera_profile_folder_picker_location(&self.app)
    }

    fn library_location(&self) -> Result<String, String> {
        calibraw_ffi::library_location(&self.app)
    }

    fn list_library_documents(&self) -> Result<Vec<LibraryDocument>, String> {
        calibraw_ffi::list_library_documents(&self.app).map(|documents| {
            documents
                .into_iter()
                .map(|document| LibraryDocument {
                    uri: document.uri,
                    display_name: document.display_name,
                    display_path: document.display_path,
                    bytes: document.bytes,
                    modified_seconds: document.modified_seconds,
                })
                .collect()
        })
    }

    fn list_library_folders(&self) -> Result<Vec<LibraryFolder>, String> {
        calibraw_ffi::list_library_folders(&self.app).map(|folders| {
            folders
                .into_iter()
                .map(|folder| LibraryFolder {
                    path: folder.path,
                    name: folder.name,
                })
                .collect()
        })
    }

    fn select_library_folder(&self, relative_path: &str) -> Result<(), String> {
        calibraw_ffi::select_library_folder(&self.app, relative_path)
    }

    fn create_library_folder(&self, parent_path: &str, name: &str) -> Result<String, String> {
        calibraw_ffi::create_library_folder(&self.app, parent_path, name)
    }

    fn materialize_library_document(
        &self,
        raw_uri: &str,
        display_name: &str,
    ) -> Result<PathBuf, String> {
        calibraw_ffi::materialize_library_document(&self.app, raw_uri, display_name)
    }

    fn open_library_document(&self, uri: &str, display_name: &str) -> Result<(), String> {
        calibraw_ffi::open_library_document(&self.app, uri, display_name)
    }

    fn import_local_library_document(
        &self,
        raw_path: &Path,
        display_name: &str,
    ) -> Result<ImportedLibraryDocument, String> {
        calibraw_ffi::import_local_library_document(&self.app, raw_path, display_name).map(
            |document| ImportedLibraryDocument {
                uri: document.uri,
                display_name: document.display_name,
            },
        )
    }

    fn delete_imported_library_document(
        &self,
        raw_uri: &str,
        display_name: &str,
    ) -> Result<(), String> {
        calibraw_ffi::delete_imported_library_document(&self.app, raw_uri, display_name)
    }

    fn rename_library_document(
        &self,
        raw_uri: &str,
        display_name: &str,
        requested_name: &str,
    ) -> Result<String, String> {
        calibraw_ffi::rename_library_document(&self.app, raw_uri, display_name, requested_name)
    }

    fn delete_library_document(&self, raw_uri: &str, display_name: &str) -> Result<(), String> {
        calibraw_ffi::delete_library_document(&self.app, raw_uri, display_name)
    }

    fn load_sidecar(
        &self,
        raw_uri: &str,
        display_name: &str,
    ) -> Result<Option<LoadedSidecar>, SidecarError> {
        calibraw_ffi::load_android(&self.app, raw_uri, display_name)
    }

    fn load_sidecar_review(
        &self,
        raw_uri: &str,
        display_name: &str,
    ) -> Result<Option<PhotoReview>, SidecarError> {
        calibraw_ffi::load_android_review(&self.app, raw_uri, display_name)
    }

    fn save_sidecar(
        &self,
        raw_uri: &str,
        display_name: &str,
        edits: EditState,
    ) -> Result<String, SidecarError> {
        calibraw_ffi::save_android(&self.app, raw_uri, display_name, edits)
    }

    fn save_sidecar_with_review_and_editing_time(
        &self,
        raw_uri: &str,
        display_name: &str,
        edits: EditState,
        review: PhotoReview,
        editing_time_ms: u64,
    ) -> Result<String, SidecarError> {
        calibraw_ffi::save_android_with_review_and_editing_time(
            &self.app,
            raw_uri,
            display_name,
            edits,
            review,
            editing_time_ms,
        )
    }

    fn reset_adjustments(
        &self,
        raw_uri: &str,
        display_name: &str,
        editing_time_ms: Option<u64>,
    ) -> Result<(), String> {
        match editing_time_ms {
            Some(editing_time_ms) => calibraw_ffi::reset_android_adjustments_with_editing_time(
                &self.app,
                raw_uri,
                display_name,
                editing_time_ms,
            ),
            None => calibraw_ffi::reset_android_adjustments(&self.app, raw_uri, display_name),
        }
    }

    fn materialize_raw_sidecar(
        &self,
        raw_uri: &str,
        display_name: &str,
    ) -> Result<Option<PathBuf>, String> {
        calibraw_ffi::materialize_raw_sidecar(&self.app, raw_uri, display_name)
    }

    fn ai_denoise_result_path(&self, raw_uri: &str, display_name: &str) -> Result<PathBuf, String> {
        calibraw_ffi::ai_denoise_result_path(&self.app, raw_uri, display_name)
    }

    fn load_library_thumbnail(
        &self,
        uri: &str,
        display_name: &str,
        bytes: u64,
        modified_seconds: u64,
        maximum_edge: u32,
    ) -> Result<RawThumbnail, String> {
        calibraw_ffi::load_library_thumbnail(
            &self.app,
            uri,
            display_name,
            bytes,
            modified_seconds,
            maximum_edge,
        )
    }

    fn load_library_display_dimensions(&self, uri: &str) -> Result<[u32; 2], String> {
        calibraw_ffi::load_library_display_dimensions(&self.app, uri)
    }

    fn load_developed_thumbnail_cache(
        &self,
        raw_uri: &str,
        display_name: &str,
        maximum_edge: u32,
    ) -> Result<Option<RawThumbnail>, String> {
        calibraw_ffi::load_developed_thumbnail_cache(&self.app, raw_uri, display_name, maximum_edge)
    }

    fn save_developed_thumbnail_cache(
        &self,
        raw_uri: &str,
        display_name: &str,
        thumbnail: &RawThumbnail,
    ) -> Result<(), String> {
        calibraw_ffi::save_developed_thumbnail_cache(&self.app, raw_uri, display_name, thumbnail)
    }

    fn copy_library_developed_thumbnail_cache(
        &self,
        source_uri: &str,
        destination_uri: &str,
    ) -> Result<(), String> {
        calibraw_ffi::copy_library_developed_thumbnail_cache(&self.app, source_uri, destination_uri)
    }

    fn clear_thumbnail_cache(&self) -> Result<(), String> {
        calibraw_ffi::clear_thumbnail_cache(&self.app)
    }

    fn thumbnail_cache_size_bytes(&self) -> Result<u64, String> {
        calibraw_ffi::thumbnail_cache_size_bytes(&self.app)
    }

    fn prepare_direct_export(
        &self,
        display_name: &str,
        mime_type: &str,
    ) -> Result<Option<PathBuf>, String> {
        calibraw_ffi::prepare_direct_export(&self.app, display_name, mime_type)
    }

    fn finalize_direct_export(&self, path: &Path) -> Result<PublishedExport, String> {
        calibraw_ffi::finalize_direct_export(&self.app, path).map(published_export)
    }

    fn cancel_direct_export(&self, path: &Path) {
        calibraw_ffi::cancel_direct_export(&self.app, path);
    }

    fn cancel_all_direct_exports(&self) {
        calibraw_ffi::cancel_all_direct_exports(&self.app);
    }

    fn publish_image(
        &self,
        path: &Path,
        display_name: &str,
        mime_type: &str,
    ) -> Result<(), String> {
        calibraw_ffi::publish_image(&self.app, path, display_name, mime_type)
    }

    fn share_export(&self, uri: &str, mime_type: &str) -> Result<(), String> {
        calibraw_ffi::share_export(&self.app, uri, mime_type)
    }

    fn start_replay_encoder(
        &self,
        path: &Path,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<Box<dyn ReplayVideoEncoder>, String> {
        calibraw_ffi::ReplayVideoEncoder::start(&self.app, path, width, height, fps)
            .map(|encoder| Box::new(MediaCodecEncoder(encoder)) as Box<dyn ReplayVideoEncoder>)
    }
}

struct MediaCodecEncoder(calibraw_ffi::ReplayVideoEncoder);

impl ReplayVideoEncoder for MediaCodecEncoder {
    fn write_frame(&mut self, yuv: &[u8]) -> Result<(), String> {
        self.0.write_frame(yuv)
    }

    fn finish(self: Box<Self>) -> Result<(), String> {
        self.0.finish()
    }
}

fn picker_result(result: calibraw_ffi::PickerResult) -> PickerResult {
    match result {
        calibraw_ffi::PickerResult::Picked(document) => PickerResult::Picked(PickedDocument {
            path: document.path,
            display_name: document.display_name,
            library_uri: document.library_uri,
            delete_after_decode: document.delete_after_decode,
            raw_fd_guard: document.raw_fd_guard,
        }),
        calibraw_ffi::PickerResult::BatchImported {
            imported,
            failed,
            errors,
        } => PickerResult::BatchImported {
            imported,
            failed,
            errors,
        },
        calibraw_ffi::PickerResult::Cancelled => PickerResult::Cancelled,
        calibraw_ffi::PickerResult::Failed(error) => PickerResult::Failed(error),
    }
}

fn published_export(export: calibraw_ffi::PublishedExport) -> PublishedExport {
    PublishedExport {
        location: export.location,
        uri: export.uri,
    }
}
