use super::*;

impl CalibRawApp {
    /// Asks for a photo to open: a native file dialog, or the platform's
    /// document picker where photos live in its media library.
    pub fn open_file_dialog(&mut self, _frame: &eframe::Frame) {
        if self.caps().has_folders() {
            self.open_desktop_file_dialog();
        } else {
            self.open_platform_document_picker();
        }
    }

    fn open_desktop_file_dialog(&mut self) {
        if crate::platform::is_flatpak() {
            // A single-file grant does not authorize persistent adjacent edit
            // sidecars. Authorize the folder, then choose the RAW in Library.
            self.open_library_folder_dialog();
            return;
        }
        if self.ui.desktop_picker_receiver.is_some() {
            return;
        }
        let extensions = crate::pipeline::supported_image_extensions()
            .flat_map(|extension| [extension.to_string(), extension.to_ascii_uppercase()])
            .collect::<Vec<_>>();
        let initial_directory = self
            .develop
            .current_path
            .as_deref()
            .and_then(selected_picker_directory)
            .or_else(|| self.library.folder().map(std::path::Path::to_path_buf));
        let mut dialog = crate::platform::FileDialog::new()
            .add_filter("RAW, TIFF, JPEG, PNG and HEIC images", &extensions);
        if let Some(directory) = initial_directory {
            dialog = dialog.set_directory(directory);
        }
        self.ui.desktop_picker_receiver = Some(spawn_ui_worker(&self.egui_ctx, move || {
            crate::app::DesktopPickerEvent::RawFile(dialog.pick_file_on_worker())
        }));
    }

    pub fn open_library_folder_dialog(&mut self) {
        if self.ui.desktop_picker_receiver.is_some() {
            return;
        }
        let mut dialog = crate::platform::FileDialog::new();
        if crate::platform::is_flatpak() {
            dialog =
                dialog.set_title("Choose a photo folder to save edits; select a RAW in Library");
        }
        if let Some(folder) = self.library.folder() {
            dialog = dialog.set_directory(folder);
        }
        self.ui.desktop_picker_receiver = Some(spawn_ui_worker(&self.egui_ctx, move || {
            crate::app::DesktopPickerEvent::LibraryFolder(dialog.pick_folder_on_worker())
        }));
    }

    pub(crate) fn choose_export_folder(&mut self) {
        if self.ui.desktop_picker_receiver.is_some() || !self.caps().has_folders() {
            return;
        }
        let initial = self.preferences.last_export_directory.as_deref();
        let mut dialog = crate::platform::FileDialog::new().set_title("Choose export folder");
        if let Some(directory) = initial.filter(|directory| directory.is_dir()) {
            dialog = dialog.set_directory(directory);
        }
        self.ui.desktop_picker_receiver = Some(spawn_ui_worker(&self.egui_ctx, move || {
            crate::app::DesktopPickerEvent::ExportFolder(dialog.pick_folder_on_worker())
        }));
    }

    fn open_platform_document_picker(&mut self) {
        if self.android_foreground_task_active() {
            self.ui.notice = Some(
                "Wait for the current foreground operation to finish before opening another photo."
                    .to_owned(),
            );
            self.egui_ctx.request_repaint();
            return;
        }
        if self.android.picker_pending {
            return;
        }
        self.develop_ui.loading_thumbnail.clear();
        let anchor = self.egui_ctx.input(|input| {
            let viewport = input.viewport_rect();
            input.pointer.interact_pos().map(|position| {
                [
                    (position.x - viewport.min.x) / viewport.width().max(1.0),
                    (position.y - viewport.min.y) / viewport.height().max(1.0),
                ]
            })
        });
        match self.platform.open_raw_document_at(anchor) {
            Ok(()) => {
                self.android.picker_pending = true;
                self.ui.notice = None;
                self.ui.status = "Choose one or more photos…".to_owned();
            }
            Err(error) => self.report_error(ErrorKind::OpenPhoto, error),
        }
    }

    pub(crate) fn poll_desktop_picker(&mut self, frame: &eframe::Frame) {
        let result = self
            .ui
            .desktop_picker_receiver
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok());
        let Some(result) = result else {
            return;
        };
        self.ui.desktop_picker_receiver = None;
        match result {
            crate::app::DesktopPickerEvent::RawFile(Some(path)) => self.open_path(path, frame),
            crate::app::DesktopPickerEvent::LibraryFolder(Some(folder)) => {
                self.library.open_folder(folder, &self.egui_ctx);
                self.persist_performance_settings();
                self.activate_tab(AppTab::Library);
            }
            crate::app::DesktopPickerEvent::CameraProfileFolder(Some(folder)) => {
                self.apply_camera_profile_folder(folder);
            }
            crate::app::DesktopPickerEvent::ExportFolder(Some(folder)) => {
                self.remember_export_directory(&folder);
            }
            crate::app::DesktopPickerEvent::OnnxRuntime(Ok(Some((path, sha256)))) => {
                self.ai.manual_runtime = Some((path, sha256));
                self.sync_onnx_runtime_source();
                self.ui.notice = Some(
                    "ONNX Runtime selection and SHA-256 pin saved. Restart CalibRaw before generating another subject mask."
                        .to_owned(),
                );
                self.prompt_restart_for_runtime_change(
                    "ONNX Runtime selection and SHA-256 pin saved.",
                );
            }
            crate::app::DesktopPickerEvent::OnnxRuntime(Err(error)) => {
                self.report_error(ErrorKind::AiRuntime, error);
            }
            crate::app::DesktopPickerEvent::PresetFiles(Some(paths)) => {
                self.import_preset_files(&paths);
            }
            crate::app::DesktopPickerEvent::LutFiles(Some(paths)) => {
                self.import_lut_files(&paths);
            }
            crate::app::DesktopPickerEvent::LutFiles(None) => {
                self.luts.apply_next_import = false;
            }
            _ => {}
        }
    }

    pub(in crate::app) fn poll_android_picker(&mut self, frame: &eframe::Frame) {
        // Before the results: a batch's progress always precedes its `BatchImported`.
        if let Some(progress) = self.platform.take_import_progress() {
            self.android.photo_import = Some(PhotoImportStatus::Copying(progress));
        }
        while let Some(result) = self.platform.take_camera_profile_folder_result() {
            match result {
                crate::platform::CameraProfileFolderResult::ImportStarted { label } => {
                    self.android.camera_profile_folder_importing_label = Some(label.clone());
                    self.ui.status = format!("Importing DCP profiles from {label}…");
                    self.ui.notice = None;
                }
                crate::platform::CameraProfileFolderResult::Picked {
                    path,
                    label,
                    profiles,
                } => {
                    self.android.picker_pending = false;
                    self.android.camera_profile_folder_importing_label = None;
                    crate::pipeline::invalidate_dcp_profile_index();
                    let previous_folder = self.preferences.camera_profile_folder.replace(path);
                    self.preferences.camera_profile_folder_label = Some(label.clone());
                    self.preferences.camera_profile_auto_detect = false;
                    self.preferences.last_camera_profile = None;
                    self.clear_decoded_raws();
                    if self.persist_performance_settings() {
                        if let Some(previous_folder) = previous_folder {
                            if self.preferences.camera_profile_folder.as_deref()
                                != Some(previous_folder.as_path())
                            {
                                if let Err(error) =
                                    self.platform.remove_camera_profile_mirror(&previous_folder)
                                {
                                    log::warn!("{error}");
                                }
                            }
                        }
                    }
                    self.ui.notice = Some(format!(
                        "Camera profile folder '{label}' imported with {profiles} DCP {}. Reopen the RAW to apply the new profile selection.",
                        if profiles == 1 { "profile" } else { "profiles" }
                    ));
                }
                crate::platform::CameraProfileFolderResult::Cancelled => {
                    self.android.picker_pending = false;
                    self.android.camera_profile_folder_importing_label = None;
                    self.ui.notice = Some("No camera profile folder selected.".to_owned());
                }
                crate::platform::CameraProfileFolderResult::Failed(error) => {
                    self.android.picker_pending = false;
                    self.android.camera_profile_folder_importing_label = None;
                    self.report_error(
                        ErrorKind::CameraProfile,
                        format!("Could not import camera profiles: {error}"),
                    );
                }
            }
        }

        while let Some(result) = self.platform.take_lut_files_result() {
            match result {
                crate::platform::LutFilesResult::Staged(folder) => {
                    self.import_staged_lut_files(&folder);
                }
                crate::platform::LutFilesResult::Cancelled => self.luts.apply_next_import = false,
                crate::platform::LutFilesResult::Failed(error) => {
                    self.luts.apply_next_import = false;
                    self.report_error(ErrorKind::Luts, format!("Could not import LUTs: {error}"));
                }
            }
        }

        while let Some(result) = self.platform.take_picker_result() {
            self.android.picker_pending = false;
            self.android.photo_import = None;
            self.handle_android_picker_result(result, frame);
        }
        self.poll_android_external_open(frame);
        if self.android.photo_import == Some(PhotoImportStatus::Loading)
            && !self.library.is_scanning()
        {
            self.android.photo_import = None;
        }
    }

    /// Opens photos other apps sent ("Open with", Share) once no open that
    /// CalibRaw requested is in flight. The picker-result handling below
    /// attributes a result to whichever workflow is pending, so an external
    /// photo must never arrive while one is.
    fn poll_android_external_open(&mut self, frame: &eframe::Frame) {
        if !self.platform.has_external_open_result() {
            return;
        }
        let requested_open_in_flight = self.android.picker_pending
            || self.export.android_batch_load_pending
            || self.android.pending_android_profile_reload.is_some()
            || self.android.pending_android_library_reset_reload
            || self.ai.library_mask_refresh.is_some();
        if requested_open_in_flight {
            // Those workflows finish without user input; check again shortly.
            self.egui_ctx
                .request_repaint_after(std::time::Duration::from_millis(250));
            return;
        }
        if let Some(result) = self.platform.take_external_open_result() {
            self.handle_android_picker_result(result, frame);
        }
        if self.platform.has_external_open_result() {
            self.egui_ctx.request_repaint();
        }
    }

    fn handle_android_picker_result(
        &mut self,
        result: crate::platform::PickerResult,
        frame: &eframe::Frame,
    ) {
        match result {
            crate::platform::PickerResult::Picked(document) => {
                self.library.refresh(&self.egui_ctx);
                let batch_owned_open = self.export.android_batch_load_pending;
                let profile_reload_owned_open =
                    self.android.pending_android_profile_reload.is_some();
                let reset_reload_owned_open =
                    std::mem::take(&mut self.android.pending_android_library_reset_reload);
                let library_refresh_owned_open = !batch_owned_open
                    && !profile_reload_owned_open
                    && !reset_reload_owned_open
                    && self.ai.library_mask_refresh.is_some();
                let keep_library_for_profile_reload =
                    profile_reload_owned_open && self.ui.active_tab == AppTab::Library;
                let keep_library_for_reset =
                    reset_reload_owned_open && self.ui.active_tab == AppTab::Library;
                // Picker completion is owned by the background reload/batch
                // workflow, so avoid interactive tab-exit cancellation hooks.
                self.ui.active_tab = if batch_owned_open
                    || library_refresh_owned_open
                    || keep_library_for_profile_reload
                    || keep_library_for_reset
                {
                    AppTab::Library
                } else {
                    AppTab::Develop
                };
                let sidecar_target = crate::sidecar::SidecarTarget::Android {
                    raw_uri: document.library_uri,
                    display_name: document.display_name.clone(),
                };
                let source = DocumentSource {
                    path: document.path,
                    label: document.display_name,
                    sidecar_target,
                    delete_after_decode: document.delete_after_decode,
                    raw_fd_guard: document.raw_fd_guard,
                };
                match self.android.pending_android_profile_reload.take() {
                    Some(reload) => self.reopen_with_camera_profile(source, reload, frame),
                    None => self.open_document(source, frame),
                }

                if self.develop.load_receiver.is_none() {
                    let error = self.ui.notice.clone().unwrap_or_else(|| {
                        "The photo decode worker could not be started.".to_owned()
                    });
                    if batch_owned_open {
                        self.export.android_batch_load_pending = false;
                        self.complete_android_library_batch_export_item(Err(error));
                    } else if library_refresh_owned_open {
                        self.complete_android_library_ai_mask_open_failure(error, frame);
                    }
                }
            }
            crate::platform::PickerResult::BatchImported {
                imported,
                failed,
                errors,
            } => {
                self.develop_ui.loading_thumbnail.clear();
                self.android.pending_android_library_reset_reload = false;
                // Batch import completion updates the visible tab directly;
                // no interactive operation should be cancelled here.
                self.ui.active_tab = AppTab::Library;
                self.library.refresh(&self.egui_ctx);
                // Keep the import popup until the rescan shows the new photos.
                self.android.photo_import = (imported > 0).then_some(PhotoImportStatus::Loading);
                self.ui.status = match (imported, failed) {
                    (0, 0) => "No photos were imported.".to_owned(),
                    (_, 0) => format!(
                        "Imported {imported} {}.",
                        if imported == 1 { "photo" } else { "photos" }
                    ),
                    _ => format!(
                        "Imported {imported} {}; {failed} failed.",
                        if imported == 1 { "photo" } else { "photos" }
                    ),
                };
                if failed > 0 {
                    self.report_error(
                        ErrorKind::Import,
                        if errors.is_empty() {
                            format!("{failed} selected photo imports failed.")
                        } else {
                            format!("Some photos could not be imported:\n{errors}")
                        },
                    );
                } else {
                    self.ui.notice = None;
                }
            }
            crate::platform::PickerResult::Cancelled => {
                self.develop_ui.loading_thumbnail.clear();
                self.android.pending_android_profile_reload = None;
                let was_reset_reload =
                    std::mem::take(&mut self.android.pending_android_library_reset_reload);
                if self.export.android_batch_load_pending {
                    self.export.android_batch_load_pending = false;
                    self.complete_android_library_batch_export_item(Err(
                        "RAW open was canceled".to_owned()
                    ));
                } else if self.ai.library_mask_refresh.is_some() {
                    self.complete_android_library_ai_mask_open_failure(
                        "RAW open was canceled".to_owned(),
                        frame,
                    );
                } else if was_reset_reload {
                    self.report_error(ErrorKind::OpenPhoto, "The photo could not be reloaded after resetting adjustments. Reopen it from the Library before continuing in Develop."
                            .to_owned(),);
                } else {
                    self.ui.notice = Some("No photos selected.".to_owned());
                }
            }
            crate::platform::PickerResult::Failed(error) => {
                self.develop_ui.loading_thumbnail.clear();
                let was_profile_reload =
                    self.android.pending_android_profile_reload.take().is_some();
                let was_reset_reload =
                    std::mem::take(&mut self.android.pending_android_library_reset_reload);
                if self.export.android_batch_load_pending
                    && !was_profile_reload
                    && !was_reset_reload
                {
                    self.export.android_batch_load_pending = false;
                    self.complete_android_library_batch_export_item(Err(error));
                } else if self.ai.library_mask_refresh.is_some()
                    && !was_profile_reload
                    && !was_reset_reload
                {
                    self.complete_android_library_ai_mask_open_failure(error, frame);
                } else {
                    self.report_error(
                        ErrorKind::OpenPhoto,
                        if was_profile_reload {
                            format!("Could not reload RAW for camera profile: {error}")
                        } else if was_reset_reload {
                            format!(
                                "Could not reload the photo after resetting adjustments: {error}"
                            )
                        } else {
                            format!("Could not import the selected file: {error}")
                        },
                    );
                }
            }
        }
    }
}
