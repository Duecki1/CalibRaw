use super::*;

/// Settings to apply to one or more photos.
#[derive(Clone, Copy)]
pub(crate) enum EditTransfer<'a> {
    /// Adjustments copied from another photo with Copy adjustments.
    Paste {
        clipboard: &'a AdjustmentClipboard,
        mode: AdjustmentPasteMode,
    },
    Preset(&'a crate::presets::Preset),
}

impl EditTransfer<'_> {
    fn selection(self) -> EditSelection {
        match self {
            Self::Paste { clipboard, .. } => clipboard.selection,
            Self::Preset(preset) => preset.selection(),
        }
    }

    /// Whether every category is rewritten, selected or not.
    fn replaces_all_categories(self) -> bool {
        matches!(
            self,
            Self::Paste {
                mode: AdjustmentPasteMode::Replace,
                ..
            }
        )
    }

    fn apply(self, destination: &mut SidecarEditState) {
        match self {
            Self::Paste { clipboard, mode } => crate::sidecar::transfer_edits(
                destination,
                &clipboard.edits,
                clipboard.selection,
                mode,
            ),
            Self::Preset(preset) => preset.apply_to(destination),
        }
    }
}

/// Where an edit transfer started. Changing the camera profile reopens the
/// photo; a Library action keeps the Library on screen while that happens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditTransferOrigin {
    Library,
    Develop,
}

/// The result of applying an [`EditTransfer`] to several Library photos.
#[derive(Debug, Default)]
pub(crate) struct LibraryEditTransferOutcome {
    pub(crate) completed: usize,
    /// Photos whose new edit has AI masks that must be regenerated.
    pub(crate) ai_refresh: Vec<crate::ui::library::LibraryAsset>,
    pub(crate) failures: Vec<String>,
}

impl CalibRawApp {
    /// Applies `transfer` to the open photo as one undoable edit. Returns
    /// whether AI masks must be regenerated.
    pub(crate) fn apply_edit_transfer_to_current(
        &mut self,
        transfer: EditTransfer<'_>,
        origin: EditTransferOrigin,
        frame: &eframe::Frame,
    ) -> Result<bool, String> {
        if self.develop.loaded_raw.is_none() || self.persistence.sidecar_target.is_none() {
            return Err("The destination image is not loaded.".to_owned());
        }

        self.finish_mask_geometry_interaction();
        self.commit_edit_history_now();
        let mut merged = self.capture_sidecar_edit_state();
        transfer.apply(&mut merged);

        let previous_camera_profile = self.develop.selected_camera_profile.clone();
        let pasted_camera_profile = merged.camera_profile.as_ref().and_then(|relative| {
            self.preferences
                .camera_profile_folder
                .as_ref()
                .map(|root| root.join(relative))
        });

        let selection = transfer.selection();
        let replacing = transfer.replaces_all_categories();
        let adjustments_changed = selection.includes_exposure() || replacing;
        let geometry_changed = selection.geometry || replacing;
        let camera_profile_category_changed = selection.camera_profile || replacing;
        let pipeline_adjustments_changed =
            adjustments_changed || geometry_changed || camera_profile_category_changed;
        let masks_changed = selection.includes_masks() || replacing;
        let lens_category_changed = selection.lens_correction || replacing;
        let lens_changed = lens_category_changed
            && (self.develop.lens_correction.enabled != merged.lens.enabled
                || self.develop.lens_correction.selected_maker != merged.lens.maker
                || self.develop.lens_correction.selected_model != merged.lens.model
                || self.develop.lens_correction.corrections != merged.lens.corrections());

        if masks_changed {
            crate::sidecar::preflight_mask_change(&merged.masks).map_err(|error| {
                format!(
                    "The adjustments were not applied because the resulting edit could not be saved: {error}"
                )
            })?;
        }

        let ai_denoise_was_enabled = self.develop.exposure.ai_denoise_enabled;
        if adjustments_changed {
            self.develop.exposure = merged.exposure;
            self.develop.exposure.sanitize_tone_curves();
        }
        if !ai_denoise_was_enabled && self.develop.exposure.ai_denoise_enabled {
            // Ask about applying the pasted AI denoise once the image is shown.
            self.ai.denoise_resume_pending = true;
        }
        if geometry_changed {
            self.develop.geometry = merged.geometry.sanitized();
            self.develop_ui.crop_constraint_reference = None;
            self.note_geometry_changed();
        }
        if camera_profile_category_changed {
            self.develop.selected_camera_profile = pasted_camera_profile.clone();
        }
        if pipeline_adjustments_changed {
            self.note_edit_changed();
            self.ai.update_needed |= merged.ai_masks_need_update;
        }

        if masks_changed {
            self.masks.stack = Arc::unwrap_or_clone(merged.masks);
            self.ai.update_needed = merged.ai_masks_need_update;
            self.rehydrate_restored_mask_state();
            self.ai.update_needed |= merged.ai_masks_need_update;
            self.mark_all_mask_layers_dirty();
        }

        if lens_category_changed {
            self.develop.lens_correction.enabled = merged.lens.enabled;
            self.develop.lens_correction.corrections = merged.lens.corrections();
            self.develop.lens_correction.selected_maker = merged.lens.maker;
            self.develop.lens_correction.selected_model = merged.lens.model;
            if lens_changed {
                self.note_lens_correction_changed_for_masks();
                self.ai.update_needed |= merged.ai_masks_need_update;
                self.mark_lens_correction_dirty();
            }
        }

        if pipeline_adjustments_changed && !lens_changed {
            self.mark_pipeline_dirty();
        }

        self.commit_edit_history_now();
        self.queue_explicit_sidecar_save();
        let needs_ai_refresh = self.ai_update_needed();
        if camera_profile_category_changed && previous_camera_profile != pasted_camera_profile {
            let edit_override = self.capture_sidecar_edit_state();
            self.reload_current_after_edit_transfer(
                frame,
                pasted_camera_profile,
                edit_override,
                origin,
            );
        }
        Ok(needs_ai_refresh)
    }

    fn reload_current_after_edit_transfer(
        &mut self,
        frame: &eframe::Frame,
        profile_selection: Option<PathBuf>,
        edit_override: SidecarEditState,
        origin: EditTransferOrigin,
    ) {
        let Some(sidecar_target) = self.persistence.sidecar_target.clone() else {
            return;
        };
        let reload = ProfileReload {
            camera_profile: profile_selection,
            edits: edit_override,
        };

        #[cfg(not(target_os = "android"))]
        {
            let crate::sidecar::SidecarTarget::Desktop { raw_path } = sidecar_target;
            self.reopen_desktop_with_camera_profile(raw_path, reload, frame);
        }

        #[cfg(target_os = "android")]
        match sidecar_target {
            crate::sidecar::SidecarTarget::Desktop { raw_path } => {
                self.reopen_desktop_with_camera_profile(raw_path, reload, frame);
            }
            crate::sidecar::SidecarTarget::Android {
                raw_uri,
                display_name,
            } => match calibraw_ffi::open_library_document(
                &self.android.android_app,
                &raw_uri,
                &display_name,
            ) {
                Ok(()) => {
                    self.android.pending_android_profile_reload = Some(reload);
                    self.android.picker_pending = true;
                }
                Err(error) => {
                    self.report_error("Library adjustment failed", format!(
                        "The adjustments were applied, but the camera profile could not be reloaded: {error}"
                    ));
                    return;
                }
            },
        }

        if origin == EditTransferOrigin::Library {
            // Reload in the background without the interactive tab-exit side
            // effects of leaving Develop.
            self.ui.active_tab = AppTab::Library;
        }
    }

    fn library_asset_is_current(&self, asset: &crate::ui::library::LibraryAsset) -> bool {
        #[cfg(not(target_os = "android"))]
        {
            asset
                .desktop_path()
                .is_some_and(|path| self.develop.current_path.as_deref() == Some(path))
                && self.develop.loaded_raw.is_some()
        }
        #[cfg(target_os = "android")]
        {
            let Some(uri) = asset.android_uri() else {
                return false;
            };
            matches!(
                self.persistence.sidecar_target.as_ref(),
                Some(crate::sidecar::SidecarTarget::Android {
                    raw_uri: current_uri,
                    display_name: current_name,
                }) if current_uri == uri && current_name == &asset.display_name
            ) && self.develop.loaded_raw.is_some()
        }
    }

    fn library_asset_edit_state(
        &mut self,
        asset: &crate::ui::library::LibraryAsset,
    ) -> Result<SidecarEditState, String> {
        #[cfg(not(target_os = "android"))]
        let persisted = {
            let path = asset
                .desktop_path()
                .ok_or_else(|| "Library asset is not available from desktop storage".to_owned())?;
            desktop_library_sidecar_edits(path)?
        };
        #[cfg(target_os = "android")]
        let persisted = {
            let uri = asset
                .android_uri()
                .ok_or_else(|| "Library asset is not available from Android storage".to_owned())?;
            crate::sidecar::load_android(&self.android.android_app, uri, &asset.display_name)
                .map_err(|error| error.to_string())?
                .map(|loaded| loaded.edits)
        };

        if self.library_asset_is_current(asset) {
            self.finish_mask_geometry_interaction();
            self.commit_edit_history_now();
            if persisted.is_none() && !self.can_undo_edit() {
                return Ok(crate::sidecar::default_edit_state());
            }
            return Ok(self.capture_sidecar_edit_state());
        }
        Ok(persisted.unwrap_or_else(crate::sidecar::default_edit_state))
    }

    fn save_library_asset_edit_state(
        &mut self,
        asset: &crate::ui::library::LibraryAsset,
        edits: SidecarEditState,
    ) -> Result<(), String> {
        let is_current = self.library_asset_is_current(asset);
        #[cfg(not(target_os = "android"))]
        {
            let path = asset
                .desktop_path()
                .ok_or_else(|| "Library asset is not available from desktop storage".to_owned())?;
            let result = if is_current {
                crate::sidecar::save_desktop_with_editing_time(
                    path,
                    edits,
                    self.raw_editing_time_ms(),
                )
            } else {
                crate::sidecar::save_desktop(path, edits)
            };
            result.map_err(|error| error.to_string())?;
            crate::sidecar::invalidate_developed_thumbnail_cache(path)?;
            Ok(())
        }
        #[cfg(target_os = "android")]
        {
            let uri = asset
                .android_uri()
                .ok_or_else(|| "Library asset is not available from Android storage".to_owned())?;
            let result = if is_current {
                crate::sidecar::save_android_with_review_and_editing_time(
                    &self.android.android_app,
                    uri,
                    &asset.display_name,
                    edits,
                    self.develop.review,
                    self.raw_editing_time_ms(),
                )
            } else {
                crate::sidecar::save_android(
                    &self.android.android_app,
                    uri,
                    &asset.display_name,
                    edits,
                )
            };
            result.map(|_| ()).map_err(|error| error.to_string())
        }
    }

    pub(crate) fn library_adjustment_edit_count(
        &mut self,
        assets: &[crate::ui::library::LibraryAsset],
    ) -> (usize, Vec<String>) {
        let mut edited = 0usize;
        let mut failures = Vec::new();
        for asset in assets {
            match self.library_asset_edit_state(asset) {
                Ok(state) => {
                    if crate::sidecar::edit_state_has_adjustments(&state) {
                        edited += 1;
                    }
                }
                Err(error) => failures.push(format!("{}: {error}", asset.display_name)),
            }
        }
        (edited, failures)
    }

    pub(crate) fn copy_library_adjustments(
        &mut self,
        asset: &crate::ui::library::LibraryAsset,
    ) -> Result<(), String> {
        let edits = self.library_asset_edit_state(asset)?;
        self.library
            .install_adjustment_clipboard(edits, self.preferences.adjustment_copy_settings.into());
        Ok(())
    }

    /// Applies `transfer` to every photo in `assets`, saving each sidecar.
    pub(crate) fn apply_edit_transfer_to_library_assets(
        &mut self,
        assets: &[crate::ui::library::LibraryAsset],
        transfer: EditTransfer<'_>,
        frame: &eframe::Frame,
    ) -> LibraryEditTransferOutcome {
        let mut outcome = LibraryEditTransferOutcome::default();

        // The open photo reloads when its camera profile changes, so it goes last.
        let mut ordered_assets = assets.to_vec();
        ordered_assets.sort_by_key(|asset| self.library_asset_is_current(asset));

        for asset in &ordered_assets {
            let result = if self.library_asset_is_current(asset) {
                self.apply_edit_transfer_to_current(transfer, EditTransferOrigin::Library, frame)
            } else {
                (|| {
                    let mut destination = self.library_asset_edit_state(asset)?;
                    transfer.apply(&mut destination);
                    let needs_ai_refresh = destination.ai_masks_need_update;
                    self.save_library_asset_edit_state(asset, destination)?;
                    Ok(needs_ai_refresh)
                })()
            };

            match result {
                Ok(needs_ai_refresh) => {
                    outcome.completed += 1;
                    if needs_ai_refresh {
                        outcome.ai_refresh.push(asset.clone());
                    }
                }
                Err(error) => outcome
                    .failures
                    .push(format!("{}: {error}", asset.display_name)),
            }
        }
        outcome
    }

    pub(crate) fn paste_library_adjustments(
        &mut self,
        assets: &[crate::ui::library::LibraryAsset],
        mode: AdjustmentPasteMode,
        frame: &eframe::Frame,
    ) -> LibraryEditTransferOutcome {
        let Some(clipboard) = self.library.adjustment_clipboard.clone() else {
            return LibraryEditTransferOutcome {
                failures: vec!["Copy adjustments from an image first.".to_owned()],
                ..LibraryEditTransferOutcome::default()
            };
        };
        self.apply_edit_transfer_to_library_assets(
            assets,
            EditTransfer::Paste {
                clipboard: &clipboard,
                mode,
            },
            frame,
        )
    }
}

#[cfg(not(target_os = "android"))]
pub(super) fn desktop_library_sidecar_edits(
    raw_path: &std::path::Path,
) -> Result<Option<SidecarEditState>, String> {
    match crate::sidecar::load_desktop(raw_path) {
        Ok(loaded) => Ok(loaded.map(|loaded| loaded.edits)),
        Err(crate::sidecar::SidecarError::Invalid(error)) => {
            log::warn!(
                "ignoring invalid sidecar while handling library adjustments for {}: {error}",
                raw_path.display()
            );
            Ok(None)
        }
        Err(error) => Err(error.to_string()),
    }
}

pub(super) fn complete_library_ai_mask_refresh_item(state: &mut LibraryAiMaskRefreshState) {
    if let Some(job) = state.current.take() {
        state.completed += 1;
        state.mask_completed += job.mask_targets;
    }
}

impl CalibRawApp {
    pub(crate) fn can_start_library_ai_mask_refresh(&self) -> bool {
        let ready = self.ai.library_mask_refresh.is_none()
            && !self.foreground_operation_active()
            && !self.sidecar_save_in_progress();
        #[cfg(not(target_os = "android"))]
        {
            ready
        }
        #[cfg(target_os = "android")]
        {
            ready && !self.android.picker_pending
        }
    }

    pub(crate) fn library_ai_mask_refresh_status(
        &self,
    ) -> Option<(usize, usize, usize, Option<String>)> {
        self.ai.library_mask_refresh.as_ref().map(|state| {
            let current_mask_progress = state.current.as_ref().map_or(0, |job| {
                if state.phase == LibraryAiMaskRefreshPhase::Loading {
                    return 0;
                }
                if state.phase == LibraryAiMaskRefreshPhase::Updating
                    && self.ai_update_needed()
                    && !self.ai_update_busy()
                {
                    return 0;
                }
                job.mask_targets
                    .saturating_sub(self.ai_update_remaining_runs())
            });
            let current = state.current.as_ref().map(|job| {
                #[cfg(not(target_os = "android"))]
                {
                    job.source
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| job.source.display().to_string())
                }
                #[cfg(target_os = "android")]
                {
                    job.display_name.clone()
                }
            });
            (
                state.mask_completed + current_mask_progress,
                state.mask_total,
                state.failures.len(),
                current,
            )
        })
    }

    #[cfg(not(target_os = "android"))]
    pub(crate) fn start_library_ai_mask_refresh_paths(
        &mut self,
        paths: Vec<PathBuf>,
        frame: &eframe::Frame,
    ) {
        if paths.is_empty() {
            return;
        }
        if !self.can_start_library_ai_mask_refresh() {
            self.ui.notice = Some(
                "Finish the current editing operation or sidecar save before refreshing AI masks."
                    .to_owned(),
            );
            return;
        }
        let pending = paths
            .into_iter()
            .map(|source| LibraryAiMaskRefreshJob {
                source,
                mask_targets: 0,
            })
            .collect::<VecDeque<_>>();
        let total = pending.len();
        self.ai.library_mask_refresh = Some(LibraryAiMaskRefreshState {
            pending,
            current: None,
            phase: LibraryAiMaskRefreshPhase::Loading,
            total,
            completed: 0,
            mask_total: 0,
            mask_completed: 0,
            failures: Vec::new(),
            cancel_requested: false,
        });
        self.start_next_library_ai_mask_refresh(frame);
        self.egui_ctx.request_repaint();
    }

    #[cfg(target_os = "android")]
    pub(crate) fn start_library_ai_mask_refresh_android(
        &mut self,
        targets: Vec<(String, String)>,
        frame: &eframe::Frame,
    ) {
        if targets.is_empty() {
            return;
        }
        if !self.can_start_library_ai_mask_refresh() {
            self.ui.notice = Some(
                "Finish the current editing operation or sidecar save before refreshing AI masks."
                    .to_owned(),
            );
            return;
        }
        let pending = targets
            .into_iter()
            .map(|(uri, display_name)| LibraryAiMaskRefreshJob {
                uri,
                display_name,
                mask_targets: 0,
            })
            .collect::<VecDeque<_>>();
        let total = pending.len();
        self.ai.library_mask_refresh = Some(LibraryAiMaskRefreshState {
            pending,
            current: None,
            phase: LibraryAiMaskRefreshPhase::Loading,
            total,
            completed: 0,
            mask_total: 0,
            mask_completed: 0,
            failures: Vec::new(),
            cancel_requested: false,
        });
        self.start_next_library_ai_mask_refresh(frame);
        self.egui_ctx.request_repaint();
    }

    #[cfg(not(target_os = "android"))]
    pub(super) fn start_next_library_ai_mask_refresh(&mut self, frame: &eframe::Frame) {
        loop {
            let next = {
                let Some(state) = self.ai.library_mask_refresh.as_mut() else {
                    return;
                };
                if state.current.is_some() {
                    return;
                }
                (if state.cancel_requested {
                    state.pending.clear();
                    None
                } else {
                    state.pending.pop_front()
                })
                .inspect(|job| {
                    state.current = Some(job.clone());
                    state.phase = LibraryAiMaskRefreshPhase::Loading;
                })
            };

            let Some(job) = next else {
                self.finish_library_ai_mask_refresh();
                return;
            };

            self.open_path(job.source.clone(), frame);
            // AI mask refresh owns this transition; activating interactively
            // would cancel the refresh's document-bound operation.
            self.ui.active_tab = AppTab::Library;
            if self.develop.load_receiver.is_some() {
                return;
            }

            if let Some(state) = self.ai.library_mask_refresh.as_mut() {
                state.failures.push(format!(
                    "{}: could not start photo load",
                    job.source.display()
                ));
                complete_library_ai_mask_refresh_item(state);
            }
        }
    }

    #[cfg(target_os = "android")]
    pub(super) fn start_next_library_ai_mask_refresh(&mut self, _frame: &eframe::Frame) {
        loop {
            let next = {
                let Some(state) = self.ai.library_mask_refresh.as_mut() else {
                    return;
                };
                if state.current.is_some() {
                    return;
                }
                (if state.cancel_requested {
                    state.pending.clear();
                    None
                } else {
                    state.pending.pop_front()
                })
                .inspect(|job| {
                    state.current = Some(job.clone());
                    state.phase = LibraryAiMaskRefreshPhase::Loading;
                })
            };

            let Some(job) = next else {
                self.finish_library_ai_mask_refresh();
                return;
            };

            match calibraw_ffi::open_library_document(
                &self.android.android_app,
                &job.uri,
                &job.display_name,
            ) {
                Ok(()) => {
                    self.android.picker_pending = true;
                    self.ui.notice = None;
                    self.ui.status = format!("Opening {}…", job.display_name);
                    // AI mask refresh owns this transition; activating interactively
                    // would cancel the refresh's document-bound operation.
                    self.ui.active_tab = AppTab::Library;
                    return;
                }
                Err(error) => {
                    if let Some(state) = self.ai.library_mask_refresh.as_mut() {
                        state
                            .failures
                            .push(format!("{}: {error}", job.display_name));
                        complete_library_ai_mask_refresh_item(state);
                    }
                }
            }
        }
    }

    pub(super) fn on_library_ai_mask_refresh_load_finished(
        &mut self,
        success: bool,
        frame: &eframe::Frame,
    ) {
        let Some(state) = self.ai.library_mask_refresh.as_ref() else {
            return;
        };
        let Some(current) = state.current.as_ref() else {
            return;
        };

        let cancel_requested = self
            .ai
            .library_mask_refresh
            .as_ref()
            .is_some_and(|state| state.cancel_requested);
        if cancel_requested {
            if let Some(state) = self.ai.library_mask_refresh.as_mut() {
                state.current = None;
            }
            self.start_next_library_ai_mask_refresh(frame);
            return;
        }

        if !success {
            #[cfg(not(target_os = "android"))]
            let label = current.source.display().to_string();
            #[cfg(target_os = "android")]
            let label = current.display_name.clone();
            if let Some(state) = self.ai.library_mask_refresh.as_mut() {
                state.failures.push(format!("{label}: photo load failed"));
                complete_library_ai_mask_refresh_item(state);
            }
            self.start_next_library_ai_mask_refresh(frame);
            return;
        }

        let mask_targets = self.masks.stack.content_dependencies().ai_run_count();
        if let Some(state) = self.ai.library_mask_refresh.as_mut() {
            let previous_targets = state.current.as_ref().map_or(0, |job| job.mask_targets);
            state.mask_total = state.mask_total.saturating_sub(previous_targets);
            if let Some(current) = state.current.as_mut() {
                current.mask_targets = mask_targets;
            }
            state.mask_total = state.mask_total.saturating_add(mask_targets);
            state.phase = LibraryAiMaskRefreshPhase::Updating;
        }
        self.request_ai_update(frame);
        // Completion is a background transition; skip the interactive
        // tab-exit side effects of `activate_tab`.
        self.ui.active_tab = AppTab::Library;
    }

    #[cfg(target_os = "android")]
    pub(super) fn complete_android_library_ai_mask_open_failure(
        &mut self,
        error: String,
        frame: &eframe::Frame,
    ) {
        let label = self
            .ai
            .library_mask_refresh
            .as_ref()
            .and_then(|state| state.current.as_ref())
            .map(|job| job.display_name.clone())
            .unwrap_or_else(|| "image".to_owned());
        if let Some(state) = self.ai.library_mask_refresh.as_mut() {
            if state.cancel_requested {
                state.current = None;
            } else {
                state.failures.push(format!("{label}: {error}"));
                complete_library_ai_mask_refresh_item(state);
            }
        }
        self.start_next_library_ai_mask_refresh(frame);
    }

    pub(crate) fn cancel_library_ai_mask_refresh(&mut self) {
        if let Some(state) = self.ai.library_mask_refresh.as_mut() {
            state.cancel_requested = true;
            state.pending.clear();
        }
        if self.content_job_active() {
            self.cancel_foreground_operation();
        }
        self.egui_ctx.request_repaint();
    }

    pub(crate) fn poll_library_ai_mask_refresh(&mut self, frame: &eframe::Frame) {
        let cancel_now = self
            .ai
            .library_mask_refresh
            .as_ref()
            .is_some_and(|state| state.cancel_requested && state.current.is_none());
        if cancel_now {
            self.finish_library_ai_mask_refresh();
            return;
        }
        let phase = self
            .ai
            .library_mask_refresh
            .as_ref()
            .and_then(|state| state.current.as_ref().map(|_| state.phase));
        let Some(phase) = phase else {
            return;
        };
        let cancel_after_phase = self
            .ai
            .library_mask_refresh
            .as_ref()
            .is_some_and(|state| state.cancel_requested);
        if cancel_after_phase
            && phase == LibraryAiMaskRefreshPhase::Updating
            && !self.ai_update_busy()
        {
            if let Some(state) = self.ai.library_mask_refresh.as_mut() {
                state.current = None;
            }
            self.start_next_library_ai_mask_refresh(frame);
            return;
        }

        let label = self
            .ai
            .library_mask_refresh
            .as_ref()
            .and_then(|state| state.current.as_ref())
            .map(|job| {
                #[cfg(not(target_os = "android"))]
                {
                    job.source.display().to_string()
                }
                #[cfg(target_os = "android")]
                {
                    job.display_name.clone()
                }
            })
            .unwrap_or_else(|| "image".to_owned());

        match phase {
            LibraryAiMaskRefreshPhase::Loading => (),
            LibraryAiMaskRefreshPhase::Updating => {
                if self.ai_update_busy() {
                    return;
                }

                if self.ai_update_needed() {
                    let reason = self
                        .ui
                        .notice
                        .clone()
                        .unwrap_or_else(|| "AI results still need an update".to_owned());
                    if let Some(state) = self.ai.library_mask_refresh.as_mut() {
                        state.failures.push(format!("{label}: {reason}"));
                        complete_library_ai_mask_refresh_item(state);
                    }
                    self.start_next_library_ai_mask_refresh(frame);
                    return;
                }

                self.commit_edit_history_now();
                self.queue_explicit_sidecar_save();
                if let Some(state) = self.ai.library_mask_refresh.as_mut() {
                    state.phase = LibraryAiMaskRefreshPhase::Saving;
                }
            }
            LibraryAiMaskRefreshPhase::Saving => {
                if self.sidecar_save_in_progress() {
                    return;
                }

                let revision = self.edit_commit_revision();
                if self.persistence.sidecar_failed_revision == Some(revision) {
                    let reason = self
                        .ui
                        .notice
                        .clone()
                        .unwrap_or_else(|| "could not save regenerated AI masks".to_owned());
                    if let Some(state) = self.ai.library_mask_refresh.as_mut() {
                        state.failures.push(format!("{label}: {reason}"));
                    }
                }

                if let Some(state) = self.ai.library_mask_refresh.as_mut() {
                    complete_library_ai_mask_refresh_item(state);
                }
                self.start_next_library_ai_mask_refresh(frame);
            }
        }
    }

    pub(super) fn finish_library_ai_mask_refresh(&mut self) {
        let Some(state) = self.ai.library_mask_refresh.take() else {
            return;
        };
        let cancelled = state.cancel_requested;
        let succeeded = state.completed.saturating_sub(state.failures.len());
        self.ui.active_tab = AppTab::Library;
        self.library.refresh(&self.egui_ctx);
        let summary = if cancelled {
            format!(
                "AI-mask refresh cancelled after {}/{} images.",
                state.completed, state.total
            )
        } else if state.failures.is_empty() {
            format!(
                "Regenerated AI masks for {succeeded} {}.",
                if succeeded == 1 { "image" } else { "images" }
            )
        } else {
            format!(
                "Regenerated AI masks for {succeeded} of {} images. {}",
                state.total,
                state.failures.join(" · ")
            )
        };
        self.library.set_status(summary.clone());
        if state.failures.is_empty() {
            self.ui.notice = Some(summary);
        } else {
            self.report_error("AI mask refresh failed", summary);
        }
        self.egui_ctx.request_repaint();
    }
}
