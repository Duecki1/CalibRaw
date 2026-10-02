use super::*;

impl CalibRawApp {
    #[cfg(target_os = "android")]
    pub fn open_android_library_document(&mut self, uri: &str, display_name: &str) {
        if self.android_foreground_task_active() {
            self.ui.notice = Some(format!(
                "{display_name} cannot be opened while an export or another foreground operation is running. Wait for it to finish or cancel it first."
            ));
            self.egui_ctx.request_repaint();
            return;
        }
        if self.android.picker_pending {
            return;
        }

        let already_loaded = self.develop.loaded_raw.is_some()
            && self.develop.preview_raw.is_some()
            && self.preview.gpu_pipeline.is_some()
            && matches!(
                self.persistence.sidecar_target.as_ref(),
                Some(crate::sidecar::SidecarTarget::Android {
                    raw_uri,
                    display_name: current_name,
                }) if raw_uri == uri && current_name == display_name
            );
        if already_loaded {
            self.activate_tab(AppTab::Develop);
            self.ui.notice = None;
            self.refresh_status();
            self.egui_ctx.request_repaint();
            return;
        }

        self.prepare_android_develop_loading_thumbnail(uri);

        self.export.android_batch_load_pending = false;
        match calibraw_ffi::open_library_document(&self.android.android_app, uri, display_name) {
            Ok(()) => {
                self.android.picker_pending = true;
                self.ui.notice = None;
                self.ui.status = format!("Opening {display_name}…");
            }
            Err(error) => {
                self.develop_ui.loading_thumbnail.clear();
                self.ui.notice = Some(error);
            }
        }
    }

    #[cfg(target_os = "android")]
    pub(crate) fn reload_android_library_document_after_reset(
        &mut self,
        uri: &str,
        display_name: &str,
    ) {
        if self.android.picker_pending {
            self.ui.notice = Some(format!(
                "Could not reload {display_name} after resetting adjustments because another Android document operation is still pending."
            ));
            return;
        }
        self.export.android_batch_load_pending = false;
        self.android.pending_android_library_reset_reload = true;
        match calibraw_ffi::open_library_document(&self.android.android_app, uri, display_name) {
            Ok(()) => {
                self.android.picker_pending = true;
                self.ui.notice = None;
                self.ui.status = format!("Reloading {display_name} after reset…");
            }
            Err(error) => {
                self.android.pending_android_library_reset_reload = false;
                self.ui.notice = Some(format!(
                    "Could not reload {display_name} after resetting adjustments: {error}"
                ));
            }
        }
    }

    pub fn open_path(&mut self, path: PathBuf, frame: &eframe::Frame) {
        if self.ai.library_mask_refresh.is_some() {
            // Background refresh temporarily opens each document in Develop.
            // Preserve that loading context without interactive AI cancellation.
            self.ui.active_tab = AppTab::Develop;
        } else {
            self.activate_tab(AppTab::Develop);
        }
        let label = path.display().to_string();
        self.open_document(DocumentSource::desktop(path, label), frame);
    }

    #[cfg(not(target_os = "android"))]
    pub(crate) fn reload_desktop_library_document_after_reset(
        &mut self,
        path: PathBuf,
        frame: &eframe::Frame,
    ) {
        let label = path.display().to_string();
        self.open_document(DocumentSource::desktop(path, label), frame);
    }

    pub(crate) fn open_document(&mut self, source: DocumentSource, frame: &eframe::Frame) {
        self.start_document_load(source, None, frame);
    }

    /// Reopens a document with another camera profile, keeping the edits in
    /// `reload` instead of reading its sidecar.
    pub(in crate::app) fn reopen_with_camera_profile(
        &mut self,
        source: DocumentSource,
        reload: ProfileReload,
        frame: &eframe::Frame,
    ) {
        self.start_document_load(source, Some(reload), frame);
    }

    /// Reopens the current desktop document with the label it is shown under.
    pub(in crate::app) fn reopen_desktop_with_camera_profile(
        &mut self,
        raw_path: PathBuf,
        reload: ProfileReload,
        frame: &eframe::Frame,
    ) {
        let label = self
            .develop
            .current_label
            .clone()
            .unwrap_or_else(|| raw_path.display().to_string());
        self.reopen_with_camera_profile(DocumentSource::desktop(raw_path, label), reload, frame);
    }

    fn start_document_load(
        &mut self,
        source: DocumentSource,
        profile_reload: Option<ProfileReload>,
        frame: &eframe::Frame,
    ) {
        if self.develop.load_receiver.is_some() {
            if let Some(copy) = source.disposable_copy() {
                remove_temporary_raw(copy);
            }
            self.ui.notice = Some("Wait for the current RAW to finish opening.".to_owned());
            return;
        }
        let Some(render_state) = frame.wgpu_render_state() else {
            if let Some(copy) = source.disposable_copy() {
                remove_temporary_raw(copy);
            }
            self.ui.notice = Some("eframe is not running with the wgpu backend.".to_owned());
            self.refresh_status();
            return;
        };
        self.prepare_loading_thumbnail_for(&source.sidecar_target);
        self.library.prepare_for_develop();

        let raw_cache_key = self.raw_cache_key(&source.sidecar_target, profile_reload.as_ref());
        // Without an explicit selection, the decoder may pick a DCP from the
        // profile folder, which the cache key cannot know in advance.
        let cache_selection_is_known = profile_reload.is_some()
            || self.preferences.camera_profile_mode == CameraProfileMode::MatrixOnly
            || self.preferences.camera_profile_folder.is_none();
        let cached_original_raw = cache_selection_is_known
            .then(|| self.cached_raw_decode(&raw_cache_key))
            .flatten();
        calibraw_core::diagnostics::record(format!(
            "RAW open requested: label=\"{}\" cached={} preview_quality={}",
            source.label,
            cached_original_raw.is_some(),
            self.preview.quality.label()
        ));
        self.cancel_document_bound_foreground_operation();
        self.abandon_ai_denoise_worker();
        self.preview.rebuild_receiver = None;
        self.preview.detail_rebuild_receiver = None;
        let editing_time_override_ms = (profile_reload.is_some()
            && self.persistence.sidecar_target.as_ref() == Some(&source.sidecar_target))
        .then(|| self.raw_editing_time_ms());
        let sidecar_generation = self.begin_sidecar_open();
        crate::app::preview_visibility::PreviewVisibility::clear(&self.egui_ctx);
        let programs = PreviewProgramSources {
            previous_pipeline: self.take_preview_pipeline_and_release_textures(),
            retained_template: self.preview.program_template.clone(),
            startup_prewarm: self.preview.gpu_prewarm_receiver.take(),
        };
        let initial_exposure = self.new_image_exposure();
        self.reset_document_state(&source.label, initial_exposure);

        let label = source.label.clone();
        let source_cleanup_on_spawn_failure = source.disposable_copy().map(Path::to_path_buf);
        let job = DocumentLoadJob {
            raw_cache_key,
            cached_original_raw,
            decode_gate: self.library.decode_gate(),
            sidecar_generation,
            initial_exposure,
            preview_quality: self.preview.quality,
            viewport_pixels: self.preview.viewport_pixels,
            camera_profiles: CameraProfileSettings {
                mode: self.preferences.camera_profile_mode,
                folder: self.preferences.camera_profile_folder.clone(),
                last_used: self.preferences.last_camera_profile.clone(),
            },
            ai_denoise_cache_path: self
                .rawnind_result_cache_path_for_target(&source.sidecar_target),
            device: render_state.device.clone(),
            queue: render_state.queue.clone(),
            programs,
            #[cfg(target_os = "android")]
            android_app: self.android.android_app.clone(),
            #[cfg(target_os = "android")]
            export_active: self.export.task.is_some(),
            source,
            profile_reload,
            editing_time_override_ms,
        };
        let repaint = self.egui_ctx.clone();
        let (sender, receiver) = mpsc::channel();
        self.develop.load_receiver = Some(receiver);
        self.develop.loading_label = Some(label);
        self.ui.notice = None;
        self.ui.unsupported_file_dialog = None;
        self.refresh_status();

        let spawn_result = std::thread::Builder::new()
            .name("calibraw-decode-preview".to_owned())
            .spawn(move || {
                let result = run_document_load(job);
                let _ = sender.send(LoadEvent::Finished(result));
                repaint.request_repaint();
            });
        if let Err(error) = spawn_result {
            if let Some(path) = source_cleanup_on_spawn_failure {
                remove_temporary_raw(&path);
            }
            self.develop.load_receiver = None;
            self.develop.loading_label = None;
            self.develop_ui.loading_thumbnail.clear();
            self.ui.notice = Some(format!("could not start RAW decode worker: {error}"));
            self.refresh_status();
        }
    }

    fn prepare_loading_thumbnail_for(&mut self, target: &crate::sidecar::SidecarTarget) {
        #[cfg(not(target_os = "android"))]
        {
            if self.ui.active_tab == AppTab::Develop {
                let crate::sidecar::SidecarTarget::Desktop { raw_path } = target;
                self.prepare_develop_loading_thumbnail(raw_path);
            } else {
                self.develop_ui.loading_thumbnail.clear();
            }
        }
        #[cfg(target_os = "android")]
        {
            let loading_thumbnail_matches = self.ui.active_tab == AppTab::Develop
                && matches!(
                    target,
                    crate::sidecar::SidecarTarget::Android { raw_uri, .. }
                        if self.develop_ui.loading_thumbnail.source_uri.as_deref()
                            == Some(raw_uri.as_str())
                );
            if !loading_thumbnail_matches {
                self.develop_ui.loading_thumbnail.clear();
            }
        }
    }

    /// Identifies a decoded RAW together with the camera-profile settings that
    /// shaped its decode.
    fn raw_cache_key(
        &self,
        target: &crate::sidecar::SidecarTarget,
        profile_reload: Option<&ProfileReload>,
    ) -> String {
        let profile_selection = match profile_reload.map(|reload| &reload.camera_profile) {
            Some(Some(path)) => path.to_string_lossy().into_owned(),
            Some(None) => "automatic".to_owned(),
            None => "sidecar".to_owned(),
        };
        format!(
            "{}|profile:{}|folder:{}|selection:{}",
            raw_cache_key_for_target(target),
            self.preferences.camera_profile_mode.cache_key(),
            self.preferences
                .camera_profile_folder
                .as_deref()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default(),
            profile_selection,
        )
    }

    /// Clears everything tied to the previous document before a new one loads.
    fn reset_document_state(&mut self, label: &str, initial_exposure: ExposureParams) {
        self.develop.original_raw = None;
        self.develop.loaded_raw = None;
        self.develop.preview_raw = None;
        self.develop_ui.point_color = Default::default();
        self.develop_ui.mask_point_color = Default::default();
        self.develop_ui.mask_point_color_mask = None;
        self.develop_ui.cancel_white_balance_picker();
        self.develop.current_path = None;
        self.develop.current_label = None;
        self.develop.selected_camera_profile = None;
        self.develop.image_status = format!("Loading {label}…");
        self.preview.original_exposure = initial_exposure;
        self.preview.original_requested = false;
        self.preview.original_rendered_state = None;
        self.clear_android_original_hold();
        self.develop.exposure = initial_exposure;
        self.develop.target_exposure = initial_exposure;
        self.masks.stack.clear();
        self.reset_inpainting_state();
        self.masks.reset_transient_state();
        self.ai.masks_need_update = false;
        self.ai.mask_update_active = false;
        self.ai.mask_update_subject_pending = false;
        self.ai.mask_update_object_queue.clear();
        self.ai.mask_update_failed = false;
        if self.ai.consent.is_mask_consent() {
            self.ai.consent = AiConsentState::None;
        }
        self.ai.object_pending_target = None;
        self.ai.object_cache = None;
        self.preview.pending_stage = None;
        self.preview.detail_pending_stage = None;
        self.preview.navigation_pending_stage = None;
        self.preview.detail_urgent = false;
        self.preview.zoom = 1.0;
        self.preview.center = [0.5, 0.5];
        self.preview.visible_uv = PreviewUvRect {
            min: [0.0, 0.0],
            max: [1.0, 1.0],
        };
        self.preview.motion_at = None;
        self.preview.touch_navigation_active = false;
        self.preview.revision = self.preview.revision.wrapping_add(1);
        self.develop.lens_correction = LensCorrectionState::default();
        self.develop.lens_correction_dirty = false;
        #[cfg(target_os = "android")]
        {
            self.preview.lens_original_cache = None;
            self.preview.lens_corrected_cache = None;
        }
        self.reset_edit_history();
    }

    pub(in crate::app) fn poll_load_worker(&mut self, frame: &eframe::Frame) {
        let received = self
            .develop
            .load_receiver
            .as_ref()
            .map(|receiver| receiver.try_recv());
        let event = match received {
            Some(Ok(event)) => Some(event),
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.develop.load_receiver = None;
                self.develop.loading_label = None;
                self.develop_ui.loading_thumbnail.clear();
                self.ui.notice = Some("RAW decode worker stopped unexpectedly.".to_owned());
                self.on_library_ai_mask_refresh_load_finished(false, frame);
                #[cfg(target_os = "android")]
                if std::mem::take(&mut self.export.android_batch_load_pending) {
                    self.on_library_batch_load_finished(false, frame);
                }
                #[cfg(not(target_os = "android"))]
                self.on_library_batch_load_finished(false, frame);
                None
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => None,
        };
        let Some(LoadEvent::Finished(result)) = event else {
            return;
        };

        self.develop.load_receiver = None;
        self.develop.loading_label = None;
        self.develop_ui.loading_thumbnail.clear();
        #[cfg(target_os = "android")]
        let batch_owned_load = std::mem::take(&mut self.export.android_batch_load_pending);

        match result {
            Ok(mut loaded) => {
                let Some(render_state) = frame.wgpu_render_state() else {
                    self.ui.notice =
                        Some("eframe is not running with the wgpu backend.".to_owned());
                    self.on_library_ai_mask_refresh_load_finished(false, frame);
                    #[cfg(target_os = "android")]
                    if batch_owned_load {
                        self.on_library_batch_load_finished(false, frame);
                    }
                    #[cfg(not(target_os = "android"))]
                    self.on_library_batch_load_finished(false, frame);
                    return;
                };
                let previous_pipeline = {
                    let mut renderer = render_state.renderer.write();
                    let previous = self.take_preview_pipeline_and_release_textures();
                    loaded
                        .pipeline
                        .register_egui_texture(&render_state.device, &mut renderer);
                    previous
                };
                drop(previous_pipeline);

                let full_width = loaded.full_raw.width;
                let full_height = loaded.full_raw.height;
                let preview_width = loaded.preview_raw.width;
                let preview_height = loaded.preview_raw.height;
                let profile_label = loaded
                    .full_raw
                    .camera_profile
                    .name
                    .as_deref()
                    .map(|name| format!(", profile {name}"))
                    .unwrap_or_default();
                self.develop.image_status = format!(
                    "{} {} — full {}×{}, preview {}×{} ({}{})",
                    loaded.full_raw.camera_make,
                    loaded.full_raw.camera_model,
                    full_width,
                    full_height,
                    preview_width,
                    preview_height,
                    self.preview.quality.label(),
                    profile_label,
                );
                self.develop.current_path = loaded.source_path;
                self.develop.current_label = Some(loaded.label.clone());
                self.develop.selected_camera_profile = loaded.selected_camera_profile.clone();
                self.cache_raw_decode(loaded.raw_cache_key, Arc::clone(&loaded.original_raw));
                self.develop.original_raw = Some(loaded.original_raw);
                self.develop.loaded_raw = Some(loaded.full_raw);
                self.develop.preview_raw = Some(loaded.preview_raw);
                self.install_raw_edit_timer(loaded.editing_time_ms);
                self.preview.program_template = Some(loaded.pipeline.program_template());
                self.preview.gpu_pipeline = Some(loaded.pipeline);
                self.develop.review = loaded.review;
                self.develop.exposure = loaded.rendered_exposure;
                self.develop.geometry = loaded.geometry.sanitized();
                self.develop_ui.crop_constraint_reference = None;
                self.develop_ui.crop_drag = None;
                self.develop_ui.straighten_tool_active = false;
                self.develop_ui.straighten_drag = None;
                self.develop.geometry_revision = 0;
                self.masks.stack = loaded.rendered_masks;
                self.install_remove_edits(Arc::new(loaded.remove));
                self.ai.masks_need_update = loaded.ai_masks_need_update;
                self.rehydrate_restored_mask_state();
                self.ai.masks_need_update |= loaded.ai_masks_need_update;
                if loaded.mask_source.is_some() {
                    self.masks.source_cache = loaded.mask_source;
                }
                self.preview.zoom = 1.0;
                self.preview.center = [0.5, 0.5];
                self.preview.visible_uv = PreviewUvRect {
                    min: [0.0, 0.0],
                    max: [1.0, 1.0],
                };
                self.preview.motion_at = None;
                self.preview.touch_navigation_active = false;
                self.preview.revision = self.preview.revision.wrapping_add(1);
                self.preview.original_rendered_state = None;
                self.preview.detail = None;
                self.preview.navigation = None;
                self.preview.detail_pending_stage = None;
                self.preview.navigation_pending_stage = None;
                self.preview.detail_urgent = false;
                self.masks.detail_dirty_layers.fill(false);
                self.masks.navigation_dirty_layers.fill(false);
                self.masks.dirty_layers.fill(false);
                self.develop.lens_correction = loaded.lens_correction;
                self.develop.lens_correction_dirty = false;
                #[cfg(target_os = "android")]
                {
                    if self.develop.lens_correction.applied {
                        self.preview.lens_corrected_cache = match (
                            self.develop.lens_correction.selected_lens(),
                            self.develop.loaded_raw.as_ref(),
                            self.develop.preview_raw.as_ref(),
                        ) {
                            (Some(selection), Some(full_raw), Some(preview_raw)) => Some((
                                selection,
                                self.preview.quality,
                                Arc::clone(full_raw),
                                Arc::clone(preview_raw),
                            )),
                            _ => None,
                        };
                        self.preview.lens_original_cache = None;
                    } else {
                        self.preview.lens_original_cache = self
                            .develop
                            .preview_raw
                            .as_ref()
                            .map(|raw| (self.preview.quality, Arc::clone(raw)));
                        self.preview.lens_corrected_cache = None;
                    }
                }
                self.develop.target_exposure = loaded.rendered_exposure;
                self.preview.pending_stage = None;
                self.ui.notice = loaded.sidecar_warning;
                crate::app::preview_visibility::PreviewVisibility::invalidate_masks(
                    &self.egui_ctx,
                    &self.masks.stack,
                );
                self.reset_edit_history();
                self.install_sidecar_target(
                    loaded.sidecar_target,
                    loaded.sidecar_generation,
                    loaded.sidecar_needs_rewrite,
                );
                self.cancel_document_bound_foreground_operation();
                self.resume_persisted_ai_denoise(frame);
                log::info!("loaded RAW preview for {}", loaded.label);
                self.on_library_ai_mask_refresh_load_finished(true, frame);
                #[cfg(target_os = "android")]
                if batch_owned_load {
                    self.on_library_batch_load_finished(true, frame);
                }
                #[cfg(not(target_os = "android"))]
                self.on_library_batch_load_finished(true, frame);
            }
            Err(error) => {
                self.ui.notice = Some(format!("Failed to decode or render RAW: {}", error.message));
                let interactive_open =
                    self.ai.library_mask_refresh.is_none() && self.export.batch.is_none();
                if error.unsupported && interactive_open {
                    self.ui.unsupported_file_dialog = Some(UnsupportedFileDialog {
                        label: error.label.clone(),
                        detail: error.message.clone(),
                    });
                }
                log::error!("RAW load failed: {}", error.message);
                self.on_library_ai_mask_refresh_load_finished(false, frame);
                #[cfg(target_os = "android")]
                if batch_owned_load {
                    self.on_library_batch_load_finished(false, frame);
                }
                #[cfg(not(target_os = "android"))]
                self.on_library_batch_load_finished(false, frame);
            }
        }
    }
}

impl CalibRawApp {
    pub(in crate::app) fn show_unsupported_file_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.ui.unsupported_file_dialog.clone() else {
            return;
        };

        let mut close = false;
        crate::ui::theme::dialog_window(
            "Unsupported RAW file",
            ctx,
            crate::ui::theme::DIALOG_WIDTH_WIDE,
        )
        .resizable(true)
        .show(ctx, |ui| {
            ui.add(egui::Label::new(egui::RichText::new(&dialog.label).strong()).wrap());
            ui.add_space(6.0);
            ui.label(
                "CalibRaw could not decode this file with the available RAW decoders. \
                 The camera or RAW variant may not be supported yet.",
            );
            ui.add_space(8.0);
            egui::CollapsingHeader::new("Technical details")
                .default_open(false)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(&dialog.detail).monospace())
                            .wrap()
                            .selectable(true),
                    );
                });
            crate::ui::theme::dialog_button_row(ui, |ui| {
                close |= crate::ui::theme::secondary_button(ui, "Close").clicked();
            });
            if !close
                && crate::ui::theme::dialog_keyboard_action(
                    ui,
                    crate::ui::theme::DialogKeyboard::CLOSE_ONLY,
                    false,
                ) == crate::ui::theme::DialogAction::Cancel
            {
                close = true;
            }
        });

        if close {
            self.ui.unsupported_file_dialog = None;
        }
    }
}
