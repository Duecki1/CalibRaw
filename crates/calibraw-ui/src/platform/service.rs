use super::PlatformCaps;
use crate::pipeline::RawThumbnail;
use crate::sidecar::{EditState, LoadedSidecar, PhotoReview, SidecarError};
use std::fs::File;
use std::path::{Path, PathBuf};

/// A photo the platform's document picker or another app handed over.
#[derive(Debug)]
pub(crate) struct PickedDocument {
    pub(crate) path: PathBuf,
    pub(crate) display_name: String,
    pub(crate) library_uri: String,
    pub(crate) delete_after_decode: bool,
    /// Keeps the descriptor behind `path` open until the decode finishes.
    pub(crate) raw_fd_guard: Option<File>,
}

#[derive(Clone, Debug)]
pub(crate) struct LibraryDocument {
    pub(crate) uri: String,
    pub(crate) display_name: String,
    pub(crate) display_path: String,
    pub(crate) bytes: u64,
    pub(crate) modified_seconds: u64,
}

/// A media-library folder; `path` is relative to the library root, `/`-separated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LibraryFolder {
    pub(crate) path: String,
    pub(crate) name: String,
}

#[cfg_attr(
    not(target_os = "android"),
    allow(dead_code, reason = "constructed only by the Android backend")
)]
#[derive(Debug)]
pub(crate) enum PickerResult {
    Picked(PickedDocument),
    BatchImported {
        imported: usize,
        failed: usize,
        errors: String,
    },
    Cancelled,
    Failed(String),
}

/// How far a running multi-photo import has copied into the library.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ImportProgress {
    pub(crate) completed: usize,
    pub(crate) total: usize,
}

/// A finished export in the platform's shared media store.
#[derive(Debug)]
pub(crate) struct PublishedExport {
    /// Human-readable folder and name, for notices.
    pub(crate) location: String,
    /// Content URI other apps can be granted, or empty when none was reported.
    pub(crate) uri: String,
}

#[cfg_attr(
    not(target_os = "android"),
    allow(dead_code, reason = "constructed only by the Android backend")
)]
#[derive(Debug)]
pub(crate) enum ExportPublishResult {
    Published(PublishedExport),
    Failed(String),
}

#[cfg_attr(
    not(target_os = "android"),
    allow(dead_code, reason = "constructed only by the Android backend")
)]
#[derive(Debug)]
pub(crate) enum CameraProfileFolderResult {
    ImportStarted {
        label: String,
    },
    Picked {
        path: PathBuf,
        label: String,
        profiles: usize,
    },
    Cancelled,
    Failed(String),
}

/// A photo copied into the media library.
#[derive(Clone, Debug)]
pub(crate) struct ImportedLibraryDocument {
    pub(crate) uri: String,
    pub(crate) display_name: String,
}

/// The progress shown in the platform's background-task notification.
#[cfg_attr(
    not(target_os = "android"),
    allow(dead_code, reason = "read only by the Android backend")
)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct TaskNotification<'a> {
    pub(crate) title: &'a str,
    pub(crate) phase: &'a str,
    pub(crate) detail: Option<&'a str>,
    pub(crate) progress_percent: i32,
    pub(crate) indeterminate: bool,
    pub(crate) queued_count: usize,
}

/// Encodes replay frames into a video file through the platform's codec.
pub(crate) trait ReplayVideoEncoder: Send {
    /// One NV12 frame of the size the encoder was started with.
    fn write_frame(&mut self, yuv: &[u8]) -> Result<(), String>;
    fn finish(self: Box<Self>) -> Result<(), String>;
}

/// The operating-system services behind actions. Handlers call these; views
/// never do.
///
/// Each backend implements what its platform offers. The provided methods
/// answer for a platform without the service: they report an error, return
/// nothing or do nothing, and are only reached if a caller ignored
/// [`PlatformCaps`].
pub(crate) trait Platform: Send + Sync {
    fn caps(&self) -> PlatformCaps;

    // --- UI connection and system chrome ---

    /// Wakes the UI after a platform callback queued a result.
    fn attach_ui(&self, _request_repaint: Box<dyn Fn() + Send + Sync>) {}
    /// Disconnects the UI before it goes away; later callbacks wake nothing.
    fn detach_ui(&self) {}
    /// Whether the app handles the system back gesture, or leaves it to the system.
    fn set_back_navigation_active(&self, _active: bool) {}
    fn take_back_request(&self) -> bool {
        false
    }
    /// Whether the system menu asked to open Settings (macOS Settings…,
    /// Cmd+,) since the last call.
    fn take_settings_request(&self) -> bool {
        false
    }
    /// Left, top, right and bottom system-bar insets in points.
    fn system_bar_insets_points(&self, _pixels_per_point: f32) -> [f32; 4] {
        [0.0; 4]
    }
    /// Height in points of an on-screen keyboard that overlays the bottom of
    /// the window, 0 while none is shown or when the window resizes for it.
    fn keyboard_inset_points(&self, _pixels_per_point: f32) -> f32 {
        0.0
    }
    /// Shows the on-screen keyboard's number layout instead of its text
    /// layout while `numeric`.
    fn set_numeric_keyboard(&self, _numeric: bool) -> Result<(), String> {
        Ok(())
    }
    fn set_light_system_bars(&self, _light: bool) -> Result<(), String> {
        Ok(())
    }
    fn device_diagnostics(&self) -> Result<Option<String>, String> {
        Ok(None)
    }
    /// Copies `text` with the platform's own clipboard, or returns `None`
    /// when egui's clipboard reaches the system one.
    fn copy_text_to_clipboard(&self, _label: &str, _text: &str) -> Option<Result<(), String>> {
        None
    }
    fn update_task_notification(&self, _notification: TaskNotification<'_>) -> Result<(), String> {
        Ok(())
    }
    fn clear_task_notification(&self) -> Result<(), String> {
        Ok(())
    }
    /// The command that starts this program again after a lost GPU device,
    /// or `None` where a restart cannot work.
    fn restart_after_gpu_loss_command(&self) -> Option<std::process::Command> {
        None
    }

    // --- Memory ---

    /// Returns the heap memory the app has freed to the system, without
    /// blocking the caller. Handlers call it after work that frees a lot.
    /// Where the allocator returns freed memory by itself, it does nothing
    /// (`calibraw_core::system_memory::release_free_heap`).
    fn release_free_memory(&self) {
        calibraw_core::system_memory::release_free_heap();
    }

    // --- App storage ---

    fn performance_settings_path(&self) -> Result<Option<PathBuf>, String>;
    /// The app's private data folder, where the platform assigns one.
    fn internal_data_dir(&self) -> Option<PathBuf> {
        None
    }
    /// Where downloaded AI models (and on desktop, automatic runtimes) live.
    fn model_root(&self) -> PathBuf;
    /// The persistent GPU pipeline cache, when the platform keeps one.
    fn gpu_pipeline_cache_dir(&self) -> Result<Option<PathBuf>, String> {
        Ok(None)
    }
    /// Where GPU AI jobs are recorded, so the next start can tell a run that
    /// died during one; `None` keeps no record.
    fn gpu_ai_job_record_path(&self) -> Option<PathBuf> {
        None
    }
    /// A Lensfun database the platform materializes, when it bundles one.
    fn lensfun_database_dir(&self) -> Result<Option<PathBuf>, String> {
        Ok(None)
    }

    // --- Document picker and results queued by the platform ---

    /// Opens the platform's document picker; the result arrives through
    /// [`Platform::take_picker_result`].
    fn open_raw_document(&self) -> Result<(), String> {
        Err(unavailable("the document picker"))
    }
    /// Picker anchor in normalized window coordinates; other platforms ignore it.
    fn open_raw_document_at(&self, _anchor: Option<[f32; 2]>) -> Result<(), String> {
        self.open_raw_document()
    }
    fn take_picker_result(&self) -> Option<PickerResult> {
        None
    }
    /// Next photo another app sent ("Open with", Share). Never `Cancelled`.
    fn take_external_open_result(&self) -> Option<PickerResult> {
        None
    }
    fn has_external_open_result(&self) -> bool {
        false
    }
    /// The newest progress of a running multi-photo import.
    fn take_import_progress(&self) -> Option<ImportProgress> {
        None
    }
    fn take_camera_profile_folder_result(&self) -> Option<CameraProfileFolderResult> {
        None
    }
    fn take_export_publish_result(&self) -> Option<ExportPublishResult> {
        None
    }

    // --- Camera profiles copied into app storage ---

    fn open_camera_profile_folder(&self) -> Result<(), String> {
        Err(unavailable("the camera profile folder picker"))
    }
    /// Reveals a local file in the system file manager.
    fn reveal_file(&self, _path: &Path) -> Result<(), String> {
        Err(unavailable("the system file manager"))
    }
    fn remove_camera_profile_mirror(&self, _path: &Path) -> Result<(), String> {
        Ok(())
    }
    fn scavenge_camera_profile_mirrors(&self, _active_mirror: Option<&Path>) -> Result<(), String> {
        Ok(())
    }
    fn clear_camera_profile_folder_picker_location(&self) -> Result<(), String> {
        Ok(())
    }

    // --- Media library ---

    fn library_location(&self) -> Result<String, String> {
        Err(unavailable("the media library"))
    }
    fn list_library_documents(&self) -> Result<Vec<LibraryDocument>, String> {
        Err(unavailable("the media library"))
    }
    fn list_library_folders(&self) -> Result<Vec<LibraryFolder>, String> {
        Err(unavailable("the media library"))
    }
    fn select_library_folder(&self, _relative_path: &str) -> Result<(), String> {
        Err(unavailable("the media library"))
    }
    /// Creates `name` under `parent_path` and returns the new folder's path.
    fn create_library_folder(&self, _parent_path: &str, _name: &str) -> Result<String, String> {
        Err(unavailable("the media library"))
    }
    /// Copies a library photo to a readable local file.
    fn materialize_library_document(
        &self,
        _raw_uri: &str,
        _display_name: &str,
    ) -> Result<PathBuf, String> {
        Err(unavailable("the media library"))
    }
    /// Opens a library photo; the result arrives as a picker result.
    fn open_library_document(&self, _uri: &str, _display_name: &str) -> Result<(), String> {
        Err(unavailable("the media library"))
    }
    fn import_local_library_document(
        &self,
        _raw_path: &Path,
        _display_name: &str,
    ) -> Result<ImportedLibraryDocument, String> {
        Err(unavailable("the media library"))
    }
    fn delete_imported_library_document(
        &self,
        _raw_uri: &str,
        _display_name: &str,
    ) -> Result<(), String> {
        Err(unavailable("the media library"))
    }
    /// Renames a library photo and returns its new display name.
    fn rename_library_document(
        &self,
        _raw_uri: &str,
        _display_name: &str,
        _requested_name: &str,
    ) -> Result<String, String> {
        Err(unavailable("the media library"))
    }
    fn delete_library_document(&self, _raw_uri: &str, _display_name: &str) -> Result<(), String> {
        Err(unavailable("the media library"))
    }

    // --- Media-library sidecars and caches ---

    fn load_sidecar(
        &self,
        _raw_uri: &str,
        _display_name: &str,
    ) -> Result<Option<LoadedSidecar>, SidecarError> {
        Ok(None)
    }
    fn load_sidecar_review(
        &self,
        _raw_uri: &str,
        _display_name: &str,
    ) -> Result<Option<PhotoReview>, SidecarError> {
        Ok(None)
    }
    fn save_sidecar(
        &self,
        _raw_uri: &str,
        _display_name: &str,
        _edits: EditState,
    ) -> Result<String, SidecarError> {
        Err(SidecarError::Io(std::io::Error::other(unavailable(
            "media-library sidecars",
        ))))
    }
    fn save_sidecar_with_review_and_editing_time(
        &self,
        _raw_uri: &str,
        _display_name: &str,
        _edits: EditState,
        _review: PhotoReview,
        _editing_time_ms: u64,
    ) -> Result<String, SidecarError> {
        Err(SidecarError::Io(std::io::Error::other(unavailable(
            "media-library sidecars",
        ))))
    }
    fn reset_adjustments(
        &self,
        _raw_uri: &str,
        _display_name: &str,
        _editing_time_ms: Option<u64>,
    ) -> Result<(), String> {
        Err(unavailable("media-library sidecars"))
    }
    fn materialize_raw_sidecar(
        &self,
        _raw_uri: &str,
        _display_name: &str,
    ) -> Result<Option<PathBuf>, String> {
        Err(unavailable("media-library sidecars"))
    }
    fn ai_denoise_result_path(
        &self,
        _raw_uri: &str,
        _display_name: &str,
    ) -> Result<PathBuf, String> {
        Err(unavailable("media-library AI denoise results"))
    }
    fn load_library_thumbnail(
        &self,
        _uri: &str,
        _display_name: &str,
        _bytes: u64,
        _modified_seconds: u64,
        _maximum_edge: u32,
    ) -> Result<RawThumbnail, String> {
        Err(unavailable("media-library thumbnails"))
    }
    fn load_library_display_dimensions(&self, _uri: &str) -> Result<[u32; 2], String> {
        Err(unavailable("media-library thumbnails"))
    }
    fn load_developed_thumbnail_cache(
        &self,
        _raw_uri: &str,
        _display_name: &str,
        _maximum_edge: u32,
    ) -> Result<Option<RawThumbnail>, String> {
        Ok(None)
    }
    fn save_developed_thumbnail_cache(
        &self,
        _raw_uri: &str,
        _display_name: &str,
        _thumbnail: &RawThumbnail,
    ) -> Result<(), String> {
        Err(unavailable("media-library thumbnails"))
    }
    fn copy_library_developed_thumbnail_cache(
        &self,
        _source_uri: &str,
        _destination_uri: &str,
    ) -> Result<(), String> {
        Err(unavailable("media-library thumbnails"))
    }
    /// Clears the app's library thumbnail cache.
    fn clear_thumbnail_cache(&self) -> Result<(), String> {
        Err(unavailable("the thumbnail cache"))
    }
    fn thumbnail_cache_size_bytes(&self) -> Result<u64, String> {
        Err(unavailable("the thumbnail cache"))
    }

    // --- Publishing exports to the shared media store ---

    fn export_location_label(&self) -> &'static str {
        "Pictures/CalibRaw"
    }

    /// A file to write the export into, or `None` to write it elsewhere and
    /// publish it with [`Platform::publish_image`].
    fn prepare_direct_export(
        &self,
        _display_name: &str,
        _mime_type: &str,
    ) -> Result<Option<PathBuf>, String> {
        Ok(None)
    }
    fn finalize_direct_export(&self, _path: &Path) -> Result<PublishedExport, String> {
        Err(unavailable("publishing exports"))
    }
    fn cancel_direct_export(&self, _path: &Path) {}
    fn cancel_all_direct_exports(&self) {}
    /// Copies a finished file into the media store; the result arrives
    /// through [`Platform::take_export_publish_result`].
    fn publish_image(
        &self,
        _path: &Path,
        _display_name: &str,
        _mime_type: &str,
    ) -> Result<(), String> {
        Err(unavailable("publishing exports"))
    }
    fn share_export(&self, _uri: &str, _mime_type: &str) -> Result<(), String> {
        Err(unavailable("sharing exports"))
    }
    fn start_replay_encoder(
        &self,
        _path: &Path,
        _width: u32,
        _height: u32,
        _fps: u32,
    ) -> Result<Box<dyn ReplayVideoEncoder>, String> {
        Err(unavailable("the replay video encoder"))
    }
}

fn unavailable(service: &str) -> String {
    format!("{service} is not available on this platform")
}
