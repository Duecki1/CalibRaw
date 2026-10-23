//! iOS startup, asynchronous document imports and sandbox-owned storage.

use super::ios_library::{IosImportStaging, IosLibrary};
use super::{
    ExportPublishResult, ImportProgress, ImportedLibraryDocument, LibraryDocument, LibraryFolder,
    PickerResult, Platform, PlatformCaps, PublishedExport,
};
use calibraw_core::pipeline::RawThumbnail;
use calibraw_core::sidecar::{EditState, LoadedSidecar, PhotoReview, SidecarError};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde::Deserialize;
use std::collections::VecDeque;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

extern "C" {
    fn calibraw_ios_attach_view(view: *mut c_void) -> bool;
    fn calibraw_ios_system_bar_insets(insets: *mut f64);
    fn calibraw_ios_open_documents(
        anchor_x: f64,
        anchor_y: f64,
        context: *mut c_void,
        callback: extern "C" fn(*mut c_void, *const c_char),
    );
    fn calibraw_ios_share_file(path: *const c_char) -> bool;
    fn calibraw_ios_publish_photo(
        path: *const c_char,
        name: *const c_char,
        video: bool,
        context: *mut c_void,
        callback: extern "C" fn(*mut c_void, *const c_char),
    );
}

type RepaintCallback = Arc<dyn Fn() + Send + Sync>;

struct IosState {
    library: IosLibrary,
    exports: IosLibrary,
    picker_results: Mutex<VecDeque<PickerResult>>,
    export_results: Mutex<VecDeque<ExportPublishResult>>,
    import_progress: Mutex<Option<ImportProgress>>,
    import_active: AtomicBool,
    repaint: Mutex<Option<RepaintCallback>>,
}

impl IosState {
    fn request_repaint(&self) {
        let callback = self
            .repaint
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(callback) = callback {
            callback();
        }
    }

    fn enqueue_picker(&self, result: PickerResult) {
        self.picker_results
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(result);
        self.request_repaint();
    }

    fn finish_import(&self, result: PickerResult) {
        self.import_active.store(false, Ordering::Release);
        *self
            .import_progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        self.enqueue_picker(result);
    }

    fn import_documents(&self, paths: Vec<PathBuf>, mut errors: Vec<String>) {
        let provider_failures = errors.len();
        let total = paths.len() + provider_failures;
        let mut imported = 0;
        let mut failed = provider_failures;
        errors.truncate(16);
        let mut single_document = None;
        for (index, path) in paths.iter().enumerate() {
            let name = path.file_name().and_then(|name| name.to_str());
            let result = name
                .ok_or_else(|| "The selected file has an invalid name".to_owned())
                .and_then(|name| self.library.import_local_library_document(path, name));
            match result {
                Ok(document) => {
                    imported += 1;
                    if total == 1 {
                        single_document = Some(document);
                    }
                }
                Err(error) => {
                    failed += 1;
                    if errors.len() < 16 {
                        errors.push(format!("{}: {error}", path.display()));
                    }
                }
            }
            *self
                .import_progress
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(ImportProgress {
                completed: index + 1 + provider_failures,
                total,
            });
            self.request_repaint();
        }
        let result = match single_document {
            Some(document) => match self
                .library
                .open_library_document_data(&document.uri, &document.display_name)
            {
                Ok(document) => PickerResult::Picked(document),
                Err(error) => PickerResult::Failed(error),
            },
            None => PickerResult::BatchImported {
                imported,
                failed,
                errors: errors.join("\n"),
            },
        };
        self.finish_import(result);
    }
}

#[derive(Deserialize)]
struct DocumentSelection {
    #[serde(default)]
    paths: Vec<PathBuf>,
    #[serde(default)]
    cancelled: bool,
    error: Option<String>,
    #[serde(default)]
    errors: Vec<String>,
    staging_directory: Option<PathBuf>,
}

extern "C" fn documents_selected(context: *mut c_void, json: *const c_char) {
    // SAFETY: The native picker invokes this exactly once with the Box passed
    // to open_documents and a NUL-terminated JSON string valid for this call.
    let (state, selection) = unsafe {
        (
            *Box::from_raw(context.cast::<Arc<IosState>>()),
            serde_json::from_slice::<DocumentSelection>(CStr::from_ptr(json).to_bytes()),
        )
    };
    let (paths, errors, staging) = match selection {
        Ok(selection) if selection.cancelled => {
            state.finish_import(PickerResult::Cancelled);
            return;
        }
        Ok(selection) => {
            let staging = match selection.staging_directory {
                Some(directory) => match IosImportStaging::new(directory, &std::env::temp_dir()) {
                    Ok(staging) => Some(staging),
                    Err(error) => {
                        state.finish_import(PickerResult::Failed(error));
                        return;
                    }
                },
                None => None,
            };
            if let Some(error) = selection.error {
                state.finish_import(PickerResult::Failed(error));
                return;
            }
            if selection.paths.is_empty() {
                state.finish_import(if selection.errors.is_empty() {
                    PickerResult::Cancelled
                } else {
                    PickerResult::Failed(
                        selection
                            .errors
                            .into_iter()
                            .take(16)
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                });
                return;
            }
            (selection.paths, selection.errors, staging)
        }
        Err(error) => {
            state.finish_import(PickerResult::Failed(format!(
                "Invalid document picker response: {error}"
            )));
            return;
        }
    };
    let worker_state = Arc::clone(&state);
    if let Err(error) = std::thread::Builder::new()
        .name("calibraw-ios-import".to_owned())
        .spawn(move || {
            let _staging = staging;
            worker_state.import_documents(paths, errors);
        })
    {
        state.finish_import(PickerResult::Failed(format!(
            "Could not start photo import: {error}"
        )));
    }
}

struct PhotoPublication {
    state: Arc<IosState>,
    document: ImportedLibraryDocument,
}

#[derive(Deserialize)]
struct PhotoPublicationResult {
    #[serde(default)]
    saved: bool,
    error: Option<String>,
}

extern "C" fn photo_published(context: *mut c_void, json: *const c_char) {
    // SAFETY: PhotoKit calls back exactly once with our boxed publication and
    // a NUL-terminated response that remains valid throughout this callback.
    let (publication, response) = unsafe {
        (
            Box::from_raw(context.cast::<PhotoPublication>()),
            serde_json::from_slice::<PhotoPublicationResult>(CStr::from_ptr(json).to_bytes()),
        )
    };
    let result = match response {
        Ok(response) if response.saved && response.error.is_none() => {
            ExportPublishResult::Published(PublishedExport {
                location: format!("Photos/{}", publication.document.display_name),
                uri: publication.document.uri.clone(),
            })
        }
        Ok(response) => ExportPublishResult::Failed(
            response
                .error
                .unwrap_or_else(|| "Photos did not save the export".to_owned()),
        ),
        Err(error) => ExportPublishResult::Failed(format!("Invalid Photos save response: {error}")),
    };
    if matches!(result, ExportPublishResult::Failed(_)) {
        let _ = publication.state.exports.delete_imported_library_document(
            &publication.document.uri,
            &publication.document.display_name,
        );
    }
    publication
        .state
        .export_results
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push_back(result);
    publication.state.request_repaint();
}

struct IosPlatform {
    data_dir: PathBuf,
    cache_dir: PathBuf,
    state: Arc<IosState>,
}

impl Platform for IosPlatform {
    fn caps(&self) -> PlatformCaps {
        PlatformCaps::IOS
    }

    fn attach_ui(&self, request_repaint: Box<dyn Fn() + Send + Sync>) {
        *self
            .state
            .repaint
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::from(request_repaint));
    }

    fn detach_ui(&self) {
        *self
            .state
            .repaint
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn system_bar_insets_points(&self, pixels_per_point: f32) -> [f32; 4] {
        let mut insets = [0.0_f64; 4];
        // SAFETY: App::ui calls this hook on UIKit's main thread; the native
        // function writes at most four values into this live array.
        unsafe { calibraw_ios_system_bar_insets(insets.as_mut_ptr()) };
        insets.map(|inset| (inset as f32 / pixels_per_point.max(f32::EPSILON)).max(0.0))
    }

    fn performance_settings_path(&self) -> Result<Option<PathBuf>, String> {
        Ok(Some(self.data_dir.join("performance.json")))
    }

    fn internal_data_dir(&self) -> Option<PathBuf> {
        Some(self.data_dir.clone())
    }

    fn model_root(&self) -> PathBuf {
        self.data_dir.join("models")
    }

    fn gpu_pipeline_cache_dir(&self) -> Result<Option<PathBuf>, String> {
        Ok(Some(self.cache_dir.clone()))
    }

    fn open_raw_document(&self) -> Result<(), String> {
        self.open_raw_document_at(None)
    }

    fn open_raw_document_at(&self, anchor: Option<[f32; 2]>) -> Result<(), String> {
        self.state
            .import_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "A photo import is already in progress".to_owned())?;
        let context = Box::into_raw(Box::new(Arc::clone(&self.state))).cast::<c_void>();
        // SAFETY: UIKit owns this callback context until its exactly-once
        // completion; documents_selected reclaims it. Presentation is dispatched
        // to the main queue by the native bridge.
        let [anchor_x, anchor_y] = anchor.unwrap_or([-1.0, -1.0]).map(f64::from);
        unsafe { calibraw_ios_open_documents(anchor_x, anchor_y, context, documents_selected) };
        Ok(())
    }

    fn take_picker_result(&self) -> Option<PickerResult> {
        self.state
            .picker_results
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
    }

    fn take_import_progress(&self) -> Option<ImportProgress> {
        self.state
            .import_progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    fn library_location(&self) -> Result<String, String> {
        self.state.library.library_location()
    }

    fn list_library_documents(&self) -> Result<Vec<LibraryDocument>, String> {
        self.state.library.list_library_documents()
    }

    fn list_library_folders(&self) -> Result<Vec<LibraryFolder>, String> {
        self.state.library.list_library_folders()
    }

    fn select_library_folder(&self, relative_path: &str) -> Result<(), String> {
        self.state.library.select_library_folder(relative_path)
    }

    fn create_library_folder(&self, parent_path: &str, name: &str) -> Result<String, String> {
        self.state.library.create_library_folder(parent_path, name)
    }

    fn materialize_library_document(
        &self,
        uri: &str,
        display_name: &str,
    ) -> Result<PathBuf, String> {
        self.state
            .library
            .materialize_library_document(uri, display_name)
    }

    fn open_library_document(&self, uri: &str, display_name: &str) -> Result<(), String> {
        let document = self
            .state
            .library
            .open_library_document_data(uri, display_name)?;
        self.state.enqueue_picker(PickerResult::Picked(document));
        Ok(())
    }

    fn import_local_library_document(
        &self,
        path: &Path,
        name: &str,
    ) -> Result<ImportedLibraryDocument, String> {
        self.state.library.import_local_library_document(path, name)
    }

    fn delete_imported_library_document(&self, uri: &str, name: &str) -> Result<(), String> {
        self.state
            .library
            .delete_imported_library_document(uri, name)
    }

    fn rename_library_document(
        &self,
        uri: &str,
        name: &str,
        requested_name: &str,
    ) -> Result<String, String> {
        self.state
            .library
            .rename_library_document(uri, name, requested_name)
    }

    fn delete_library_document(&self, uri: &str, name: &str) -> Result<(), String> {
        self.state.library.delete_library_document(uri, name)
    }

    fn load_sidecar(&self, uri: &str, name: &str) -> Result<Option<LoadedSidecar>, SidecarError> {
        self.state.library.load_sidecar(uri, name)
    }

    fn load_sidecar_review(
        &self,
        uri: &str,
        name: &str,
    ) -> Result<Option<PhotoReview>, SidecarError> {
        self.state.library.load_sidecar_review(uri, name)
    }

    fn save_sidecar(
        &self,
        uri: &str,
        name: &str,
        edits: EditState,
    ) -> Result<String, SidecarError> {
        self.state.library.save_sidecar(uri, name, edits)
    }

    fn save_sidecar_with_review_and_editing_time(
        &self,
        uri: &str,
        name: &str,
        edits: EditState,
        review: PhotoReview,
        editing_time_ms: u64,
    ) -> Result<String, SidecarError> {
        self.state
            .library
            .save_sidecar_with_review_and_editing_time(uri, name, edits, review, editing_time_ms)
    }

    fn reset_adjustments(
        &self,
        uri: &str,
        name: &str,
        editing_time_ms: Option<u64>,
    ) -> Result<(), String> {
        self.state
            .library
            .reset_adjustments(uri, name, editing_time_ms)
    }

    fn materialize_raw_sidecar(&self, uri: &str, name: &str) -> Result<Option<PathBuf>, String> {
        self.state.library.materialize_raw_sidecar(uri, name)
    }

    fn ai_denoise_result_path(&self, uri: &str, name: &str) -> Result<PathBuf, String> {
        self.state.library.ai_denoise_result_path(uri, name)
    }

    fn load_library_thumbnail(
        &self,
        uri: &str,
        name: &str,
        bytes: u64,
        modified_seconds: u64,
        maximum_edge: u32,
    ) -> Result<RawThumbnail, String> {
        self.state
            .library
            .load_library_thumbnail(uri, name, bytes, modified_seconds, maximum_edge)
    }

    fn load_library_display_dimensions(&self, uri: &str) -> Result<[u32; 2], String> {
        self.state.library.load_library_display_dimensions(uri)
    }

    fn load_developed_thumbnail_cache(
        &self,
        uri: &str,
        name: &str,
        maximum_edge: u32,
    ) -> Result<Option<RawThumbnail>, String> {
        self.state
            .library
            .load_developed_thumbnail_cache(uri, name, maximum_edge)
    }

    fn save_developed_thumbnail_cache(
        &self,
        uri: &str,
        name: &str,
        thumbnail: &RawThumbnail,
    ) -> Result<(), String> {
        self.state
            .library
            .save_developed_thumbnail_cache(uri, name, thumbnail)
    }

    fn copy_library_developed_thumbnail_cache(
        &self,
        source_uri: &str,
        destination_uri: &str,
    ) -> Result<(), String> {
        self.state
            .library
            .copy_library_developed_thumbnail_cache(source_uri, destination_uri)
    }

    fn clear_thumbnail_cache(&self) -> Result<(), String> {
        self.state.library.clear_thumbnail_cache()
    }

    fn thumbnail_cache_size_bytes(&self) -> Result<u64, String> {
        self.state.library.thumbnail_cache_size_bytes()
    }

    fn export_location_label(&self) -> &'static str {
        "Photos"
    }

    fn publish_image(
        &self,
        path: &Path,
        display_name: &str,
        mime_type: &str,
    ) -> Result<(), String> {
        let state = Arc::clone(&self.state);
        let path = path.to_owned();
        let display_name = display_name.to_owned();
        let video = mime_type.starts_with("video/");
        std::thread::Builder::new()
            .name("calibraw-ios-publish".to_owned())
            .spawn(move || {
                let prepared = (|| {
                    let name =
                        CString::new(display_name.as_str()).map_err(|error| error.to_string())?;
                    let document = state
                        .exports
                        .import_local_library_document(&path, &display_name)?;
                    let cached = state
                        .exports
                        .materialize_library_document(&document.uri, "")?;
                    let path = CString::new(
                        cached
                            .to_str()
                            .ok_or_else(|| "Export path is not UTF-8".to_owned())?,
                    )
                    .map_err(|error| error.to_string())?;
                    Ok::<_, String>((document, path, name))
                })();
                let result = match prepared {
                    Ok((document, path, name)) => {
                        let context = Box::into_raw(Box::new(PhotoPublication {
                            state: Arc::clone(&state),
                            document,
                        }))
                        .cast::<c_void>();
                        // SAFETY: The bridge copies both strings before returning
                        // and owns the context until its exactly-once callback.
                        unsafe {
                            calibraw_ios_publish_photo(
                                path.as_ptr(),
                                name.as_ptr(),
                                video,
                                context,
                                photo_published,
                            )
                        };
                        return;
                    }
                    Err(error) => ExportPublishResult::Failed(error),
                };
                state
                    .export_results
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push_back(result);
                state.request_repaint();
            })
            .map_err(|error| format!("Could not start export publishing: {error}"))?;
        Ok(())
    }

    fn take_export_publish_result(&self) -> Option<ExportPublishResult> {
        self.state
            .export_results
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
    }

    fn share_export(&self, uri: &str, _mime_type: &str) -> Result<(), String> {
        let path = self.state.exports.materialize_library_document(uri, "")?;
        let path = CString::new(
            path.to_str()
                .ok_or_else(|| "Export path is not UTF-8".to_owned())?,
        )
        .map_err(|error| error.to_string())?;
        // SAFETY: The share handler runs on UIKit's main thread; the native
        // function copies this live NUL-terminated path before returning.
        if unsafe { calibraw_ios_share_file(path.as_ptr()) } {
            Ok(())
        } else {
            Err("Could not present the iOS share sheet".to_owned())
        }
    }
}

pub(crate) fn main() -> eframe::Result {
    env_logger::init();
    let home = std::env::var_os("HOME").ok_or_else(|| {
        eframe::Error::AppCreation("iOS did not provide a sandbox HOME directory".into())
    })?;
    let home = PathBuf::from(home);
    let data_dir = home.join("Library/Application Support/CalibRaw");
    let cache_dir = home.join("Library/Caches/CalibRaw");
    let library = IosLibrary::new(home.join("Documents/CalibRaw"), cache_dir.join("library"))
        .map_err(|error| eframe::Error::AppCreation(error.into()))?;
    let exports = IosLibrary::new(cache_dir.join("shared-exports"), cache_dir.join("exports"))
        .map_err(|error| eframe::Error::AppCreation(error.into()))?;
    let platform = Arc::new(IosPlatform {
        data_dir,
        cache_dir,
        state: Arc::new(IosState {
            library,
            exports,
            picker_results: Mutex::new(VecDeque::new()),
            export_results: Mutex::new(VecDeque::new()),
            import_progress: Mutex::new(None),
            import_active: AtomicBool::new(false),
            repaint: Mutex::new(None),
        }),
    });
    for directory in [
        &platform.data_dir,
        &platform.cache_dir,
        &home.join("Documents"),
    ] {
        std::fs::create_dir_all(directory)
            .map_err(|error| eframe::Error::AppCreation(Box::new(error)))?;
    }
    let mut options = crate::native_options(PlatformCaps::IOS);
    options.run_and_return = false;
    options.viewport = eframe::egui::ViewportBuilder::default().with_maximized(true);
    eframe::run_native(
        "CalibRaw",
        options,
        Box::new(move |cc| {
            let window_handle = cc.window_handle()?;
            let RawWindowHandle::UiKit(handle) = window_handle.as_raw() else {
                return Err("iOS did not provide a UIKit window handle".into());
            };
            // SAFETY: eframe owns the UIKit view for this borrowed handle's lifetime,
            // and invokes the creation callback on the UIKit main thread.
            if !unsafe { calibraw_ios_attach_view(handle.ui_view.as_ptr()) } {
                return Err("iOS window has no connected application scene".into());
            }
            Ok(crate::app::create_app(cc, platform))
        }),
    )
}
