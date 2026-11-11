use super::PlatformCaps;
use crate::pipeline::RawThumbnail;
use crate::sidecar::{
    EditState, LoadedSidecar, PhotoCompanion, PhotoReview, SavedSidecar, SidecarError,
    SidecarMetadata, SidecarStart, SidecarSummary,
};
use calibraw_core::display_metadata_cache::FileStamp;
use calibraw_core::pipeline::RawDisplayMetadata;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a touch is held to become a long press where the platform has
/// no setting of its own: Android's default Touch & hold delay.
pub(super) const DEFAULT_LONG_PRESS: Duration = Duration::from_millis(400);

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
    pub(crate) sidecar: ListedSidecar,
    /// The folder below the listed one that holds the photo, `/`-separated
    /// and relative to it; empty for the listed folder itself.
    pub(crate) subfolder: String,
}

/// What a media-library listing says about a photo's sidecar.
#[cfg_attr(
    not(target_os = "android"),
    allow(dead_code, reason = "listed only by the Android backend")
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListedSidecar {
    /// The platform does not list sidecars; the photo's is read to find out.
    Unlisted,
    Absent,
    /// The sidecar's size and modification time, which tell its versions apart.
    Present(FileStamp),
}

/// A media-library folder; `path` is relative to the library root, `/`-separated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LibraryFolder {
    pub(crate) path: String,
    pub(crate) name: String,
}

/// A folder of the person's own, opened where it is; `key` names it in the
/// platform and in settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LinkedFolder {
    pub(crate) key: String,
    pub(crate) label: String,
}

#[cfg_attr(
    not(target_os = "android"),
    allow(dead_code, reason = "constructed only by the Android backend")
)]
#[derive(Debug)]
pub(crate) enum LinkFolderResult {
    Linked { key: String },
    Cancelled,
    Failed(String),
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

/// Files a platform picker copied into app storage: `.cube` files for the
/// LUT library or the watermark image.
#[cfg_attr(
    not(target_os = "android"),
    allow(dead_code, reason = "constructed only by the Android backend")
)]
#[derive(Debug)]
pub(crate) enum StagedFilesResult {
    /// The picked files, copied into this folder, which the receiver deletes
    /// after importing them.
    Staged(PathBuf),
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

/// Starts CalibRaw again after a lost GPU device, once the edits are saved
/// and the notice was shown. On the desktop it starts a new process and this
/// one then closes; on Android it ends this process and returns only with
/// the reason it could not.
pub(crate) type GpuLossRestart = Box<dyn FnOnce() -> Result<(), String>>;

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
    /// How long a touch is held to become a long press, as the user set it.
    fn long_press_duration(&self) -> Duration {
        DEFAULT_LONG_PRESS
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
    /// What starts CalibRaw again after a lost GPU device, or `None` where a
    /// restart cannot work.
    fn restart_after_gpu_loss(&self) -> Option<GpuLossRestart> {
        None
    }
    /// A native notice for a failure the UI can no longer draw, such as a
    /// lost GPU device. Blocks until dismissed.
    fn show_fatal_message(&self, title: &str, description: &str) {
        super::files::show_fatal_message(title, description);
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
    /// died during one; `None` keeps no record. Where GPU AI is unavailable
    /// there is nothing to record.
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
    fn take_lut_files_result(&self) -> Option<StagedFilesResult> {
        None
    }
    fn take_watermark_image_result(&self) -> Option<StagedFilesResult> {
        None
    }

    // --- Files copied into app storage ---

    /// Asks for `.cube` files; the result arrives through
    /// [`Self::take_lut_files_result`]. Folder storage uses `FileDialog`.
    fn open_lut_files(&self) -> Result<(), String> {
        Err(unavailable("the LUT file picker"))
    }

    /// Asks for one image; the result arrives through
    /// [`Self::take_watermark_image_result`]. Folder storage uses `FileDialog`.
    fn open_watermark_image(&self) -> Result<(), String> {
        Err(unavailable("the watermark image picker"))
    }

    fn open_camera_profile_folder(&self) -> Result<(), String> {
        Err(unavailable("the camera profile folder picker"))
    }
    /// Shows a local file in the system file manager, selected where the
    /// platform allows. The error is the system's own text, for an error dialog.
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
    /// The photos of the selected folder and, with `include_subfolders`, of
    /// every folder below it.
    fn list_library_documents(
        &self,
        _include_subfolders: bool,
    ) -> Result<Vec<LibraryDocument>, String> {
        Err(unavailable("the media library"))
    }
    fn list_library_folders(&self) -> Result<Vec<LibraryFolder>, String> {
        Err(unavailable("the media library"))
    }
    fn select_library_folder(&self, _relative_path: &str) -> Result<(), String> {
        Err(unavailable("the media library"))
    }
    /// Asks for a folder to link; the result arrives through
    /// [`Self::take_link_folder_result`].
    fn open_link_folder_picker(&self) -> Result<(), String> {
        Err(unavailable("linked folders"))
    }
    fn take_link_folder_result(&self) -> Option<LinkFolderResult> {
        None
    }
    fn list_linked_folders(&self) -> Result<Vec<LinkedFolder>, String> {
        Ok(Vec::new())
    }
    /// Stops showing a linked folder; its files stay where they are.
    fn unlink_folder(&self, _key: &str) -> Result<(), String> {
        Err(unavailable("linked folders"))
    }
    /// The folders of the linked folder `key`, or of the media library for an
    /// empty key, as [`Self::list_library_folders`] lists them, whichever
    /// folder is selected.
    fn list_folder_tree(&self, _key: &str) -> Result<Vec<LibraryFolder>, String> {
        Err(unavailable("linked folders"))
    }
    /// Lists from the top of a linked folder, or of the media library for an
    /// empty key.
    fn select_library_root(&self, key: &str) -> Result<(), String> {
        if key.is_empty() {
            Ok(())
        } else {
            Err(unavailable("linked folders"))
        }
    }
    /// Whether sync changed the selected linked folder since the last call.
    fn take_library_changed(&self) -> bool {
        false
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
    /// The sidecar's review, editing time and start, without decoding its
    /// edits; `None` when the photo has no sidecar.
    fn load_sidecar_metadata(
        &self,
        _raw_uri: &str,
        _display_name: &str,
    ) -> Result<Option<SidecarMetadata>, SidecarError> {
        Ok(None)
    }
    /// Saves `edits`, keeping the sidecar's review, editing time and start.
    fn save_sidecar(
        &self,
        _raw_uri: &str,
        _display_name: &str,
        _edits: EditState,
    ) -> Result<SavedSidecar, SidecarError> {
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
        _start: SidecarStart,
    ) -> Result<SavedSidecar, SidecarError> {
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
    fn people_analysis_path(&self, _raw_uri: &str, _display_name: &str) -> Result<PathBuf, String> {
        Err(unavailable("media-library people analyses"))
    }
    /// Makes the companion's local path hold what the photo's folder has,
    /// where that path is not the folder itself. May copy a large file.
    fn fetch_photo_companion(
        &self,
        _raw_uri: &str,
        _display_name: &str,
        _companion: PhotoCompanion,
    ) -> Result<(), String> {
        Ok(())
    }
    /// Stores what was written at the companion's local path in the photo's
    /// folder. May copy a large file.
    fn publish_photo_companion(
        &self,
        _raw_uri: &str,
        _display_name: &str,
        _companion: PhotoCompanion,
    ) -> Result<(), String> {
        Ok(())
    }
    /// Whether the photo's folder holds this companion.
    fn photo_companion_exists(
        &self,
        raw_uri: &str,
        display_name: &str,
        companion: PhotoCompanion,
    ) -> Result<bool, String> {
        let path = match companion {
            PhotoCompanion::AiDenoise => self.ai_denoise_result_path(raw_uri, display_name),
            PhotoCompanion::PeopleAnalysis => self.people_analysis_path(raw_uri, display_name),
        }?;
        Ok(path.is_file())
    }
    /// Where the folder cache of a media-library folder (dimensions and
    /// sidecar summaries) is kept: `folder` within the linked folder
    /// `linked_folder`, or within the library for an empty one. `None` keeps
    /// it for one scan only.
    fn library_folder_cache_path(&self, _linked_folder: &str, _folder: &str) -> Option<PathBuf> {
        None
    }
    /// The stored preview of a media-library photo, or `None` when it has to
    /// be extracted with [`Self::extract_library_preview`].
    fn load_cached_library_preview(
        &self,
        _uri: &str,
        _bytes: u64,
        _modified_seconds: u64,
        _maximum_edge: u32,
    ) -> Result<Option<RawThumbnail>, String> {
        Ok(None)
    }
    /// Reads a media-library photo's preview, and with
    /// `read_display_metadata` its display metadata from the same read, and
    /// stores the preview. A failed metadata read is `None`; the preview is
    /// still returned.
    fn extract_library_preview(
        &self,
        uri: &str,
        display_name: &str,
        bytes: u64,
        modified_seconds: u64,
        maximum_edge: u32,
        read_display_metadata: bool,
    ) -> Result<(RawThumbnail, Option<RawDisplayMetadata>), String> {
        let thumbnail =
            self.load_library_thumbnail(uri, display_name, bytes, modified_seconds, maximum_edge)?;
        let display_metadata = if read_display_metadata {
            self.load_library_display_metadata(uri, display_name)
                .map_err(|error| log::warn!("{error}"))
                .ok()
        } else {
            None
        };
        Ok((thumbnail, display_metadata))
    }
    /// A media-library photo's display metadata, read on its own.
    fn load_library_display_metadata(
        &self,
        uri: &str,
        _display_name: &str,
    ) -> Result<RawDisplayMetadata, String> {
        self.load_library_display_dimensions(uri)
            .map(|dimensions| RawDisplayMetadata {
                dimensions,
                ..RawDisplayMetadata::default()
            })
    }
    /// The preview half of the default [`Self::extract_library_preview`], for
    /// platforms that cache previews themselves.
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
    /// The dimensions half of the default
    /// [`Self::load_library_display_metadata`].
    fn load_library_display_dimensions(&self, _uri: &str) -> Result<[u32; 2], String> {
        Err(unavailable("media-library thumbnails"))
    }
    /// How many sidecar writes the app has made, taken before listing a
    /// folder for [`Self::sidecar_written_since`].
    fn sidecar_write_count(&self) -> u64 {
        0
    }
    /// Whether the app wrote the photo's sidecar after `count` was taken, so a
    /// listing taken before does not show it.
    fn sidecar_written_since(&self, _raw_uri: &str, _count: u64) -> bool {
        false
    }
    /// What the library grid needs from a media-library photo's sidecar;
    /// `None` when it has none. Asked only of platforms whose listing reports
    /// sidecars ([`ListedSidecar::Present`]).
    fn load_sidecar_summary(
        &self,
        _raw_uri: &str,
        _display_name: &str,
    ) -> Result<Option<SidecarSummary>, SidecarError> {
        Err(SidecarError::Platform(unavailable("sidecar summaries")))
    }
    /// Runs `read` with a readable path to a media-library photo, valid only
    /// while `read` runs: the photo's descriptor, or a private copy when the
    /// decoders cannot reopen that, so `read` may run twice.
    fn read_library_photo(
        &self,
        _uri: &str,
        _display_name: &str,
        _read: &mut dyn FnMut(&Path) -> Result<(), String>,
    ) -> Result<(), String> {
        Err(unavailable("reading media-library photos"))
    }
    /// The photo's sidecar and its render fingerprint from one read, for
    /// rendering its edits; `None` when it has none.
    fn load_sidecar_for_render(
        &self,
        _raw_uri: &str,
        _display_name: &str,
    ) -> Result<Option<(LoadedSidecar, u64)>, SidecarError> {
        Err(SidecarError::Platform(unavailable("rendering saved edits")))
    }
    /// Saves a developed thumbnail rendered from the photo's sidecar while it
    /// had `render_fingerprint`. False, and nothing kept, when the sidecar no
    /// longer has it.
    fn save_rendered_developed_thumbnail(
        &self,
        _raw_uri: &str,
        _display_name: &str,
        _thumbnail: &RawThumbnail,
        _render_fingerprint: u64,
    ) -> Result<bool, String> {
        Err(unavailable("media-library thumbnails"))
    }
    /// The developed thumbnail saved for the photo while its sidecar had
    /// `render_fingerprint`; `None` when there is none or it is out of date.
    /// Asked only of platforms whose listing reports sidecars.
    fn load_current_developed_thumbnail(
        &self,
        _raw_uri: &str,
        _render_fingerprint: u64,
        _maximum_edge: u32,
    ) -> Result<Option<RawThumbnail>, String> {
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
    /// Opens the share sheet for several published exports, in this order.
    fn share_exports(&self, _uris: &[String], _mime_type: &str) -> Result<(), String> {
        Err(unavailable("sharing several exports"))
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
