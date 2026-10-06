use super::*;

mod developed_thumbnail;
mod failures;
mod library_actions;

const SIDECAR_AUTOSAVE_INTERVAL: Duration = Duration::from_millis(900);
const SIDECAR_AUTOSAVE_ACTIVE_POLL: Duration = Duration::from_millis(100);

pub(super) fn autosave_deadline(
    existing: Option<SidecarAutosaveDeadline>,
    generation: u64,
    now: Instant,
) -> SidecarAutosaveDeadline {
    existing
        .filter(|deadline| deadline.generation == generation)
        .unwrap_or(SidecarAutosaveDeadline {
            generation,
            due_at: now + SIDECAR_AUTOSAVE_INTERVAL,
        })
}

pub(super) fn sidecar_interaction_active(ctx: &egui::Context) -> bool {
    ctx.input(|input| input.pointer.any_down()) || ctx.egui_wants_keyboard_input()
}

fn edited_preview_is_current_for_thumbnail(
    original_requested: bool,
    original_rendered_state: Option<(bool, u64)>,
    preview_revision: u64,
) -> bool {
    !original_requested && original_rendered_state == Some((false, preview_revision))
}

impl CalibRawApp {
    pub(super) fn capture_sidecar_edit_state(&self) -> SidecarEditState {
        let masks = self.committed_mask_state_for_persistence();
        let camera_profile = self
            .develop
            .selected_camera_profile
            .as_ref()
            .and_then(|selected| {
                let root = self.preferences.camera_profile_folder.as_ref()?;
                if selected == root {
                    return Some(std::path::PathBuf::from("."));
                }
                let relative = selected.strip_prefix(root).ok()?;
                (!relative.as_os_str().is_empty()).then(|| relative.to_path_buf())
            });
        SidecarEditState {
            exposure: self.develop.exposure,
            geometry: self.develop.geometry.sanitized(),
            camera_profile,
            subject_refinement: (!masks.subject_refinement.is_empty())
                .then(|| masks.subject_refinement.clone()),
            masks,
            lens: SidecarLensEditState {
                enabled: self.develop.lens_correction.enabled,
                maker: self.develop.lens_correction.selected_maker.clone(),
                model: self.develop.lens_correction.selected_model.clone(),
                geometry: self.develop.lens_correction.corrections.geometry,
                vignetting: self.develop.lens_correction.corrections.vignetting,
            },
            color_lut: self.develop.color_lut.clone(),
            remove: self.committed_remove_state_for_persistence(),
            ai_masks_need_update: self.ai.update_needed,
        }
    }

    pub(super) fn begin_sidecar_open(&mut self) -> u64 {
        self.commit_edit_history_now();
        let revision = self.edit_commit_revision();
        let pending_latest = self
            .persistence
            .sidecar_pending
            .iter_mut()
            .find(|request| {
                request.generation == self.persistence.document_generation
                    && request.revision == revision
            })
            .map(|request| request.explicit = true)
            .is_some();
        let already_queued = self.persistence.sidecar_in_flight.is_some_and(|job| {
            job.generation == self.persistence.document_generation && job.revision == revision
        }) || pending_latest;
        if self.persistence.sidecar_saved_revision != Some(revision) && !already_queued {
            self.queue_current_sidecar_save(true);
        }
        self.start_next_sidecar_save();

        self.persistence.document_generation = self.persistence.document_generation.wrapping_add(1);
        self.persistence.sidecar_target = None;
        self.persistence.sidecar_saved_revision = None;
        self.persistence.sidecar_failed_revision = None;
        self.persistence.sidecar_autosave_deadline = None;
        self.clear_raw_edit_timer();
        self.persistence.document_generation
    }

    pub(super) fn install_sidecar_target(
        &mut self,
        target: crate::sidecar::SidecarTarget,
        generation: u64,
        needs_rewrite: bool,
    ) {
        if generation != self.persistence.document_generation {
            return;
        }
        self.persistence.sidecar_target = Some(target);
        self.persistence.sidecar_failed_revision = None;
        self.persistence.sidecar_saved_revision =
            (!needs_rewrite).then(|| self.edit_commit_revision());
        if needs_rewrite {
            self.queue_current_sidecar_save(false);
            self.start_next_sidecar_save();
        } else {
            #[cfg(not(target_os = "android"))]
            if self
                .persistence
                .sidecar_target
                .as_ref()
                .is_some_and(|target| match target {
                    crate::sidecar::SidecarTarget::Desktop { raw_path } => {
                        crate::sidecar::sidecar_path_for_raw(raw_path).is_file()
                    }
                })
            {
                self.queue_developed_thumbnail_refresh(generation, self.edit_commit_revision());
            }
        }
    }

    pub(crate) fn queue_explicit_sidecar_save(&mut self) {
        self.commit_edit_history_now();
        self.queue_current_sidecar_save(true);
        self.start_next_sidecar_save();
    }

    pub(super) fn queue_current_sidecar_save(&mut self, explicit: bool) {
        let Some(target) = self.persistence.sidecar_target.clone() else {
            return;
        };
        let generation = self.persistence.document_generation;
        let revision = self.edit_commit_revision();

        if !explicit
            && (self.persistence.sidecar_saved_revision == Some(revision)
                || self.persistence.sidecar_failed_revision == Some(revision)
                || self
                    .persistence
                    .sidecar_in_flight
                    .is_some_and(|job| job.generation == generation && job.revision == revision)
                || self.persistence.sidecar_pending.iter().any(|request| {
                    request.generation == generation && request.revision == revision
                }))
        {
            return;
        }

        let mut request = SidecarSaveRequest {
            target,
            generation,
            revision,
            explicit,
            edits: self.capture_sidecar_edit_state(),
            editing_time_ms: self.raw_editing_time_ms(),
            #[cfg(target_os = "android")]
            review: self.develop.review,
        };
        if let Some(index) = self
            .persistence
            .sidecar_pending
            .iter()
            .position(|pending| pending.generation == generation)
        {
            request.explicit |= self.persistence.sidecar_pending[index].explicit;
            self.persistence.sidecar_pending[index] = request;
        } else {
            self.persistence.sidecar_pending.push_back(request);
        }
    }

    pub(super) fn schedule_sidecar_autosave(
        &mut self,
        ctx: &egui::Context,
        interaction_active: bool,
    ) {
        if self.develop.loaded_raw.is_none() || self.persistence.sidecar_target.is_none() {
            self.persistence.sidecar_autosave_deadline = None;
            return;
        }

        let generation = self.persistence.document_generation;
        let revision = self.edit_commit_revision();
        let revision_is_covered =
            self.persistence.sidecar_saved_revision == Some(revision)
                || self.persistence.sidecar_failed_revision == Some(revision)
                || self
                    .persistence
                    .sidecar_in_flight
                    .is_some_and(|job| job.generation == generation && job.revision == revision)
                || self.persistence.sidecar_pending.iter().any(|request| {
                    request.generation == generation && request.revision == revision
                });
        if revision_is_covered {
            self.persistence.sidecar_autosave_deadline = None;
            self.start_next_sidecar_save();
            return;
        }

        let stale_pending = self
            .persistence
            .sidecar_pending
            .iter()
            .any(|request| request.generation == generation && request.revision != revision);
        if stale_pending && !interaction_active {
            self.persistence.sidecar_autosave_deadline = None;
            self.queue_current_sidecar_save(false);
            self.start_next_sidecar_save();
            return;
        }

        let now = Instant::now();
        let deadline =
            autosave_deadline(self.persistence.sidecar_autosave_deadline, generation, now);
        self.persistence.sidecar_autosave_deadline = Some(deadline);
        if now < deadline.due_at {
            ctx.request_repaint_after(deadline.due_at.duration_since(now));
            return;
        }
        if interaction_active {
            ctx.request_repaint_after(SIDECAR_AUTOSAVE_ACTIVE_POLL);
            return;
        }

        self.persistence.sidecar_autosave_deadline = None;
        self.queue_current_sidecar_save(false);
        self.start_next_sidecar_save();
    }

    pub(crate) fn can_save_edits(&self) -> bool {
        self.develop.loaded_raw.is_some() && self.persistence.sidecar_target.is_some()
    }

    pub(crate) fn sidecar_save_in_progress(&self) -> bool {
        self.persistence
            .sidecar_in_flight
            .is_some_and(|job| job.generation == self.persistence.document_generation)
            || self
                .persistence
                .sidecar_pending
                .iter()
                .any(|request| request.generation == self.persistence.document_generation)
    }

    pub(crate) fn sidecar_save_succeeded_recently(&self) -> bool {
        self.persistence
            .sidecar_save_feedback_until
            .is_some_and(|until| Instant::now() < until)
    }

    pub(crate) fn save_edits_now(&mut self) {
        if !self.can_save_edits() {
            return;
        }
        self.commit_edit_history_now();
        self.persistence.sidecar_save_feedback_until = None;
        self.persistence.sidecar_failed_revision = None;
        self.queue_current_sidecar_save(true);
        self.start_next_sidecar_save();
        self.ui.notice = Some("Saving edits…".to_owned());
    }

    pub(crate) fn handle_sidecar_shortcut(&mut self, ctx: &egui::Context) {
        if !self.app_shortcuts_allowed(ctx) {
            return;
        }
        let save = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::S);
        let action = AppAction::SaveEdits;
        if self.action_enabled(action) && ctx.input_mut(|input| input.consume_shortcut(&save)) {
            self.dispatch_action(action);
        }
    }

    pub(super) fn start_next_sidecar_save(&mut self) {
        if self.persistence.sidecar_in_flight.is_some() {
            return;
        }
        if self
            .persistence
            .sidecar_pending
            .front()
            .is_some_and(|request| {
                !request.explicit
                    && request.generation == self.persistence.document_generation
                    && sidecar_interaction_active(&self.egui_ctx)
            })
        {
            self.egui_ctx
                .request_repaint_after(SIDECAR_AUTOSAVE_ACTIVE_POLL);
            return;
        }
        let Some(request) = self.persistence.sidecar_pending.pop_front() else {
            return;
        };
        let job = SidecarSaveJob {
            generation: request.generation,
            revision: request.revision,
            explicit: request.explicit,
        };
        let repaint = self.egui_ctx.clone();
        let (sender, receiver) = mpsc::channel();
        #[cfg(target_os = "android")]
        let android_app = self.android.android_app.clone();

        let spawn = std::thread::Builder::new()
            .name("calibraw-sidecar-save".to_owned())
            .spawn(move || {
                let recovery = request.clone();
                let result = save_sidecar_request(
                    request,
                    #[cfg(target_os = "android")]
                    &android_app,
                );
                let recovery = matches!(&result, Err(crate::sidecar::SidecarError::Unsupported(_)))
                    .then_some(recovery);
                let _ = sender.send(SidecarSaveEvent {
                    job,
                    result,
                    recovery,
                });
                repaint.request_repaint();
            });

        match spawn {
            Ok(_) => {
                self.persistence.sidecar_in_flight = Some(job);
                self.persistence.sidecar_receiver = Some(receiver);
            }
            Err(error) => {
                if job.generation == self.persistence.document_generation {
                    self.report_sidecar_save_failure(
                        Some(job.revision),
                        format!("could not start the edit-save worker: {error}"),
                    );
                } else {
                    self.report_sidecar_save_failure(
                        None,
                        format!(
                            "could not start the edit-save worker for a previously opened RAW: {error}"
                        ),
                    );
                }
            }
        }
    }

    pub(super) fn poll_sidecar_save(&mut self) {
        let received = self
            .persistence
            .sidecar_receiver
            .as_ref()
            .map(|receiver| receiver.try_recv());
        let event = match received {
            Some(Ok(event)) => Some(event),
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                let job = self.persistence.sidecar_in_flight.take();
                self.persistence.sidecar_receiver = None;
                if let Some(job) =
                    job.filter(|job| job.generation == self.persistence.document_generation)
                {
                    self.report_sidecar_save_failure(
                        Some(job.revision),
                        "the edit-save worker stopped unexpectedly",
                    );
                } else if job.is_some() {
                    self.report_sidecar_save_failure(
                        None,
                        "the edit-save worker for a previously opened RAW stopped unexpectedly",
                    );
                }
                None
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => None,
        };

        if let Some(event) = event {
            self.finish_sidecar_save(event);
        }

        self.start_next_sidecar_save();
    }

    pub(super) fn finish_sidecar_save(&mut self, event: SidecarSaveEvent) {
        self.persistence.sidecar_receiver = None;
        self.persistence.sidecar_in_flight = None;
        if event.job.generation == self.persistence.document_generation {
            match event.result {
                Ok(location) => {
                    let recovered_from_failure =
                        self.persistence.sidecar_failed_revision.take().is_some();
                    self.persistence.sidecar_saved_revision = Some(event.job.revision);
                    self.queue_developed_thumbnail_refresh(
                        event.job.generation,
                        event.job.revision,
                    );
                    if event.job.explicit || recovered_from_failure {
                        self.ui.notice = Some(format!("Edits saved to {location}."));
                    }
                    if event.job.explicit {
                        self.persistence.sidecar_save_feedback_until =
                            Some(Instant::now() + Duration::from_millis(1_200));
                        self.egui_ctx
                            .request_repaint_after(Duration::from_millis(1_200));
                    }
                }
                Err(error) => {
                    self.report_sidecar_save_failure(Some(event.job.revision), error.to_string());
                    self.persistence.sidecar_recovery = event.recovery;
                }
            }
        } else if let Err(error) = event.result {
            self.report_sidecar_save_failure(
                None,
                format!("saving a previously opened RAW failed: {error}"),
            );
            self.persistence.sidecar_recovery = event.recovery;
        }
    }

    pub(super) fn flush_sidecar_on_exit(&mut self) {
        self.commit_edit_history_now();
        let revision = self.edit_commit_revision();
        for request in &mut self.persistence.sidecar_pending {
            if request.generation == self.persistence.document_generation
                && request.revision == revision
            {
                request.explicit = true;
            }
        }
        if self.persistence.sidecar_saved_revision != Some(revision)
            && !self.persistence.sidecar_in_flight.is_some_and(|job| {
                job.generation == self.persistence.document_generation && job.revision == revision
            })
            && !self.persistence.sidecar_pending.iter().any(|request| {
                request.generation == self.persistence.document_generation
                    && request.revision == revision
            })
        {
            self.queue_current_sidecar_save(true);
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            self.start_next_sidecar_save();
            if self.persistence.sidecar_in_flight.is_none()
                && self.persistence.sidecar_pending.is_empty()
            {
                break;
            }
            let Some(receiver) = self.persistence.sidecar_receiver.as_ref() else {
                let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                    break;
                };
                std::thread::sleep(remaining.min(Duration::from_millis(10)));
                continue;
            };
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            match receiver.recv_timeout(remaining) {
                Ok(event) => self.finish_sidecar_save(event),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let job = self.persistence.sidecar_in_flight.take();
                    self.persistence.sidecar_receiver = None;
                    let revision = job
                        .filter(|job| job.generation == self.persistence.document_generation)
                        .map(|job| job.revision);
                    self.report_sidecar_save_failure(
                        revision,
                        "the edit-save worker stopped unexpectedly while finishing the save",
                    );
                }
                Err(mpsc::RecvTimeoutError::Timeout) => break,
            }
        }
        if self.persistence.sidecar_in_flight.is_some()
            || !self.persistence.sidecar_pending.is_empty()
        {
            let revision = self
                .persistence
                .sidecar_in_flight
                .filter(|job| job.generation == self.persistence.document_generation)
                .map(|job| job.revision);
            self.report_sidecar_save_failure(
                revision,
                "timed out while finishing the edit save during shutdown",
            );
        }
    }
}

pub(super) fn save_sidecar_request(
    request: SidecarSaveRequest,
    #[cfg(target_os = "android")] android_app: &calibraw_ffi::AndroidApp,
) -> Result<String, crate::sidecar::SidecarError> {
    match request.target {
        crate::sidecar::SidecarTarget::Desktop { raw_path } => {
            crate::sidecar::save_desktop_with_editing_time(
                &raw_path,
                request.edits,
                request.editing_time_ms,
            )
            .map(|path| path.display().to_string())
        }
        #[cfg(target_os = "android")]
        crate::sidecar::SidecarTarget::Android {
            raw_uri,
            display_name,
        } => crate::sidecar::save_android_with_review_and_editing_time(
            android_app,
            &raw_uri,
            &display_name,
            request.edits,
            request.review,
            request.editing_time_ms,
        ),
    }
}

#[cfg(test)]
mod sidecar_persistence_tests {
    use super::*;

    #[test]
    fn autosave_deadline_does_not_slide_for_continuous_commits() {
        let started = Instant::now();
        let first = autosave_deadline(None, 7, started);
        let later = autosave_deadline(Some(first), 7, started + Duration::from_millis(500));

        assert_eq!(later.generation, 7);
        assert_eq!(later.due_at, first.due_at);
    }

    #[test]
    fn autosave_deadline_is_scoped_to_the_open_image() {
        let started = Instant::now();
        let old = autosave_deadline(None, 2, started);
        let switched_at = started + Duration::from_millis(100);
        let new = autosave_deadline(Some(old), 3, switched_at);

        assert_eq!(new.generation, 3);
        assert_eq!(new.due_at, switched_at + SIDECAR_AUTOSAVE_INTERVAL);
    }

    #[test]
    fn developed_thumbnail_waits_for_edited_preview_after_before_after_use() {
        let revision = 41;

        assert!(!edited_preview_is_current_for_thumbnail(
            true,
            Some((true, revision)),
            revision,
        ));
        assert!(!edited_preview_is_current_for_thumbnail(
            false,
            Some((true, revision)),
            revision,
        ));
        assert!(edited_preview_is_current_for_thumbnail(
            false,
            Some((false, revision)),
            revision,
        ));
    }
}
