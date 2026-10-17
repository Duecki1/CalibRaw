use super::*;

impl CalibRawApp {
    /// Asks for a DCP profile folder: a native folder dialog, or the
    /// platform's picker, which copies the folder into app storage.
    pub(crate) fn choose_camera_profile_folder(&mut self) {
        if self.caps().has_folders() {
            self.choose_desktop_camera_profile_folder();
        } else {
            self.import_camera_profile_folder();
        }
    }

    fn choose_desktop_camera_profile_folder(&mut self) {
        if self.ui.desktop_picker_receiver.is_some() {
            return;
        }
        let mut dialog = crate::platform::FileDialog::new();
        if let Some(folder) = &self.preferences.camera_profile_folder {
            dialog = dialog.set_directory(folder);
        }
        self.ui.desktop_picker_receiver = Some(spawn_ui_worker(&self.egui_ctx, move || {
            crate::app::DesktopPickerEvent::CameraProfileFolder(dialog.pick_folder_on_worker())
        }));
    }

    pub(in crate::app) fn apply_camera_profile_folder(&mut self, folder: PathBuf) {
        crate::pipeline::invalidate_dcp_profile_index();
        self.preferences.camera_profile_folder_label = folder
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned);
        self.preferences.camera_profile_folder = Some(folder);
        self.preferences.camera_profile_auto_detect = false;
        self.preferences.last_camera_profile = None;
        self.clear_decoded_raws();
        self.persist_performance_settings();
        self.ui.notice = Some(
            "Camera profile folder updated. Reopen the RAW to apply the new profile selection."
                .to_owned(),
        );
    }

    fn import_camera_profile_folder(&mut self) {
        if self.android.picker_pending {
            self.ui.notice = Some("Finish the current Android file picker first.".to_owned());
            return;
        }
        match self.platform.open_camera_profile_folder() {
            Ok(()) => {
                self.android.picker_pending = true;
                self.android.camera_profile_folder_importing_label = None;
                self.ui.notice = None;
                self.ui.status = "Choose a CameraProfiles folder…".to_owned();
            }
            Err(error) => self.report_error(ErrorKind::CameraProfile, error),
        }
    }

    pub(crate) fn clear_camera_profile_folder(&mut self) {
        let previous_folder = self.preferences.camera_profile_folder.take();
        if previous_folder.is_some() || self.preferences.camera_profile_auto_detect {
            crate::pipeline::invalidate_dcp_profile_index();
            self.preferences.camera_profile_folder_label = None;
            self.preferences.camera_profile_auto_detect = false;
            self.preferences.last_camera_profile = None;
            self.clear_decoded_raws();
            // Imported profiles are app-storage copies; forget the picker
            // location and delete the copy.
            let imported = self.caps().has_media_library();
            if imported {
                if let Err(error) = self.platform.clear_camera_profile_folder_picker_location() {
                    log::warn!("{error}");
                }
            }
            if self.persist_performance_settings() && imported {
                if let Some(previous_folder) = previous_folder {
                    if let Err(error) = self.platform.remove_camera_profile_mirror(&previous_folder)
                    {
                        log::warn!("{error}");
                    }
                }
            }
            self.ui.notice = Some(
                "Camera profile folder cleared. Reopen the RAW to apply the new profile selection."
                    .to_owned(),
            );
        }
    }

    pub(crate) fn auto_detect_camera_profile_folder(&mut self) {
        crate::pipeline::invalidate_dcp_profile_index();
        self.preferences.camera_profile_auto_detect = true;
        match crate::platform::detected_camera_profile_folder() {
            Some(folder) => {
                self.preferences.camera_profile_folder = Some(folder.clone());
                self.preferences.camera_profile_folder_label =
                    Some("DCP profiles (auto-detected)".to_owned());
                self.preferences.last_camera_profile = None;
                self.clear_decoded_raws();
                self.persist_performance_settings();
                self.ui.notice = Some(format!(
                    "Using DCP profiles from {}. Reopen the RAW to apply them.",
                    folder.display()
                ));
            }
            None => {
                self.preferences.camera_profile_folder = None;
                self.preferences.camera_profile_folder_label = None;
                self.preferences.last_camera_profile = None;
                self.clear_decoded_raws();
                self.persist_performance_settings();
                self.ui.notice =
                    Some("No DCP profile folder was found in the standard locations.".to_owned());
            }
        }
    }

    pub(crate) fn set_camera_profile_mode(&mut self, mode: CameraProfileMode) {
        if self.preferences.camera_profile_mode == mode {
            return;
        }
        self.preferences.camera_profile_mode = mode;
        self.clear_decoded_raws();
        self.persist_performance_settings();
        self.ui.notice = Some(
            "RAW color profile mode changed. Reopen the RAW to apply the new profile selection."
                .to_owned(),
        );
    }

    pub(crate) fn select_camera_profile_for_current(
        &mut self,
        selection: Option<PathBuf>,
        frame: &eframe::Frame,
    ) {
        if self.android_foreground_task_active() {
            self.ui.notice = Some(
                "Wait for the current foreground operation to finish before changing camera profile."
                    .to_owned(),
            );
            self.egui_ctx.request_repaint();
            return;
        }
        if self.develop.load_receiver.is_some() {
            self.ui.notice = Some(
                "Wait for the current RAW load to finish before changing camera profile."
                    .to_owned(),
            );
            return;
        }
        if self.develop.selected_camera_profile == selection {
            return;
        }
        let Some(sidecar_target) = self.persistence.sidecar_target.clone() else {
            self.ui.notice = Some("Open a RAW before choosing a camera profile.".to_owned());
            return;
        };
        if let Some(selected) = selection.as_ref() {
            let embedded_matrix = self
                .preferences
                .camera_profile_folder
                .as_ref()
                .is_some_and(|root| selected == root);
            let is_available = self.develop.loaded_raw.as_ref().is_some_and(|raw| {
                raw.available_camera_profiles
                    .iter()
                    .any(|candidate| candidate.path == *selected)
            });
            if !embedded_matrix && !is_available {
                self.report_error(ErrorKind::CameraProfile, "That DCP is no longer available for the current camera. Refresh the profile folder and reopen the RAW."
                        .to_owned(),);
                return;
            }
        }

        self.develop.selected_camera_profile = selection.clone();
        self.preferences.last_camera_profile = selection
            .as_ref()
            .zip(self.preferences.camera_profile_folder.as_ref())
            .and_then(|(selected, root)| selected.strip_prefix(root).ok())
            .filter(|relative| !relative.as_os_str().is_empty())
            .map(std::path::Path::to_path_buf);
        self.persist_performance_settings();
        self.queue_explicit_sidecar_save();
        let edit_override = self.capture_sidecar_edit_state();

        let reload = ProfileReload {
            camera_profile: selection,
            edits: edit_override,
        };
        if self.caps().has_folders() {
            if let crate::sidecar::SidecarTarget::Desktop { raw_path } = sidecar_target {
                self.reopen_desktop_with_camera_profile(raw_path, reload, frame);
            }
        } else {
            let (raw_uri, display_name) = match sidecar_target {
                crate::sidecar::SidecarTarget::Android {
                    raw_uri,
                    display_name,
                } => (raw_uri, display_name),
                crate::sidecar::SidecarTarget::Desktop { .. } => {
                    self.report_error(
                        ErrorKind::CameraProfile,
                        "The current Android RAW does not have a reloadable library target."
                            .to_owned(),
                    );
                    return;
                }
            };
            match self.platform.open_library_document(&raw_uri, &display_name) {
                Ok(()) => {
                    self.android.pending_android_profile_reload = Some(reload);
                    self.android.picker_pending = true;
                    self.ui.notice = None;
                    self.ui.status = format!("Applying camera profile to {display_name}…");
                }
                Err(error) => {
                    self.report_error(
                        ErrorKind::CameraProfile,
                        format!("Could not reload RAW for camera profile: {error}"),
                    );
                }
            }
        }
    }
}
