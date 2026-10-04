use super::*;
use calibraw_ai::ai_denoise::{AiDenoiseEvent, RawNindJob};
use calibraw_ai::AiFeature;
use std::{
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
};

const DENOISE_JOB: ForegroundOperationKind = ForegroundOperationKind::Ai(AiFeature::Denoise);

impl CalibRawApp {
    pub(crate) fn ai_denoise_running(&self) -> bool {
        self.foreground_operation_is(DENOISE_JOB)
    }

    pub(in crate::app) fn rawnind_model_dir(&self) -> PathBuf {
        #[cfg(target_os = "android")]
        {
            self.android
                .android_app
                .internal_data_path()
                .unwrap_or_else(std::env::temp_dir)
                .join("models/rawdenoise-nind-1.0")
        }
        #[cfg(not(target_os = "android"))]
        {
            calibraw_ai::ai_denoise::model_cache_dir()
        }
    }

    /// The AI-denoise result saved next to the RAW. `None` when the RAW has no
    /// writable location, e.g. an Android URI outside CalibRaw's library.
    pub(crate) fn ai_denoise_result_path_for_target(
        &self,
        target: &crate::sidecar::SidecarTarget,
    ) -> Option<PathBuf> {
        match target {
            #[cfg(not(target_os = "android"))]
            crate::sidecar::SidecarTarget::Desktop { raw_path } => {
                Some(crate::sidecar::ai_denoise_path_for_raw(raw_path))
            }
            #[cfg(target_os = "android")]
            crate::sidecar::SidecarTarget::Desktop { .. } => None,
            #[cfg(target_os = "android")]
            crate::sidecar::SidecarTarget::Android {
                raw_uri,
                display_name,
            } => calibraw_ffi::ai_denoise_result_path(
                &self.android.android_app,
                raw_uri,
                display_name,
            )
            .map_err(|error| log::warn!("{error}"))
            .ok(),
        }
    }

    fn ai_denoise_result_path(&self) -> Option<PathBuf> {
        self.persistence
            .sidecar_target
            .as_ref()
            .and_then(|target| self.ai_denoise_result_path_for_target(target))
    }

    /// Results used to live in an app-data cache keyed by the RAW's path;
    /// they now live next to the RAW. Removes that old folder and nothing else.
    pub(in crate::app) fn remove_legacy_ai_denoise_cache(&self) {
        #[cfg(target_os = "android")]
        let root = self
            .android
            .android_app
            .internal_data_path()
            .map(|data| data.join("ai-denoise-results-v2"));
        #[cfg(not(target_os = "android"))]
        let root = self
            .rawnind_model_dir()
            .parent()
            .and_then(std::path::Path::parent)
            .map(|cache_root| cache_root.join("ai-denoise-results-v2"));
        let Some(root) = root.filter(|root| root.is_dir()) else {
            return;
        };
        let spawned = std::thread::Builder::new()
            .name("calibraw-legacy-denoise-cleanup".to_owned())
            .spawn(move || match std::fs::remove_dir_all(&root) {
                Ok(()) => log::info!("removed legacy AI-denoise cache {}", root.display()),
                Err(error) => log::warn!("could not remove {}: {error}", root.display()),
            });
        if let Err(error) = spawned {
            log::warn!("could not start legacy AI-denoise cleanup: {error}");
        }
    }

    pub(crate) fn set_ai_denoise_enabled(&mut self, enabled: bool, frame: &eframe::Frame) {
        if !enabled {
            if self.ai_consent_is_for(AiFeature::Denoise) {
                self.ai.consent = None;
            }
            self.cancel_foreground_operation_if(DENOISE_JOB);
            let changed = self.develop.exposure.ai_denoise_enabled;
            self.develop.exposure.ai_denoise_enabled = false;
            self.develop.target_exposure.ai_denoise_enabled = false;
            self.preview.quality_dirty = true;
            self.discard_auxiliary_previews();
            self.preview.pending_stage = None;
            self.preview.detail_pending_stage = None;
            self.preview.navigation_pending_stage = None;
            if changed {
                self.note_edit_changed();
            }
            self.egui_ctx.request_repaint();
            return;
        }
        self.enable_ai_denoise(frame, AiJobOrigin::Requested);
    }

    fn enable_ai_denoise(&mut self, frame: &eframe::Frame, origin: AiJobOrigin) {
        if self.foreground_operation_active() {
            self.ui.notice =
                Some("Finish or cancel the current editing operation first.".to_owned());
            return;
        }
        if self.develop.loaded_raw.is_none() {
            self.develop.exposure.ai_denoise_enabled = false;
            self.develop.target_exposure.ai_denoise_enabled = false;
            self.ui.notice = Some("Open a photo before enabling AI denoise.".to_owned());
            self.egui_ctx.request_repaint();
            return;
        }
        if self
            .develop
            .loaded_raw
            .as_ref()
            .is_some_and(|raw| raw.is_pre_demosaiced_raster())
        {
            self.develop.exposure.ai_denoise_enabled = false;
            self.develop.target_exposure.ai_denoise_enabled = false;
            self.ui.notice = Some(
                "AI denoise works on sensor RAW data; rendered photos (JPEG, PNG, HEIC, TIFF) use the standard Detail controls."
                    .to_owned(),
            );
            self.egui_ctx.request_repaint();
            return;
        }
        if self
            .develop
            .loaded_raw
            .as_ref()
            .is_some_and(|raw| raw.ai_denoised_image().is_some())
        {
            self.develop.exposure.ai_denoise_enabled = true;
            self.note_edit_changed();
            self.preview.quality_dirty = true;
            self.discard_auxiliary_previews();
            return;
        }
        let saved_result_exists = self
            .ai_denoise_result_path()
            .is_some_and(|path| path.is_file());
        // A saved result restores without running the model, so it never asks.
        if saved_result_exists || self.ai_job_may_start_from(AiFeature::Denoise, origin) {
            self.start_ai_denoise(frame, false);
        }
    }

    pub(in crate::app) fn start_ai_denoise(
        &mut self,
        frame: &eframe::Frame,
        allow_model_download: bool,
    ) {
        if self.foreground_operation_active() {
            return;
        }
        let Some(raw) = self.develop.loaded_raw.as_ref().map(Arc::clone) else {
            self.ui.notice = Some("Open a photo before enabling AI denoise.".to_owned());
            return;
        };
        let result_path = self.ai_denoise_result_path();
        let saved_result_exists = result_path.as_ref().is_some_and(|path| path.is_file());
        #[cfg(not(target_os = "android"))]
        if !saved_result_exists && !self.validate_onnx_runtime_for_ai() {
            self.develop.exposure.ai_denoise_enabled = false;
            return;
        }
        #[cfg(target_os = "android")]
        if !saved_result_exists {
            if let Err(error) = calibraw_ai::ai_masks::initialize_runtime(None, None) {
                self.develop.exposure.ai_denoise_enabled = false;
                self.develop.target_exposure.ai_denoise_enabled = false;
                self.ui.notice = Some(format!(
                    "Could not initialize Android AI denoise: {error:#}"
                ));
                calibraw_core::diagnostics::record(format!(
                    "Android RawNIND runtime initialization failed before worker start: {error:#}"
                ));
                self.egui_ctx.request_repaint();
                return;
            }
        }
        let Some(render_state) = frame.wgpu_render_state() else {
            self.ui.notice = Some("AI denoise requires CalibRaw's wgpu renderer.".to_owned());
            self.develop.exposure.ai_denoise_enabled = false;
            self.develop.target_exposure.ai_denoise_enabled = false;
            return;
        };
        raw.clear_ai_denoised_image();
        #[cfg(target_os = "android")]
        {
            let previous_pipeline = self.take_preview_pipeline_and_release_textures();
            drop(previous_pipeline);
        }
        #[cfg(not(target_os = "android"))]
        self.discard_auxiliary_previews();
        self.preview.pending_stage = None;
        self.preview.detail_pending_stage = None;
        self.preview.navigation_pending_stage = None;
        self.preview.detail_urgent = false;
        self.preview.quality_dirty = false;
        let cancellation = Arc::new(AtomicBool::new(false));
        let receiver = calibraw_ai::ai_denoise::spawn_rawnind_denoise(
            self.rawnind_model_dir(),
            {
                #[cfg(not(target_os = "android"))]
                {
                    self.onnx_runtime_for_ai().0
                }
                #[cfg(target_os = "android")]
                {
                    None
                }
            },
            {
                #[cfg(not(target_os = "android"))]
                {
                    self.onnx_runtime_for_ai().1
                }
                #[cfg(target_os = "android")]
                {
                    None
                }
            },
            raw,
            Some(render_state.device.clone()),
            Some(render_state.queue.clone()),
            match result_path {
                Some(path) if saved_result_exists => RawNindJob::Restore { path },
                save_to => RawNindJob::Infer {
                    save_to,
                    allow_model_download,
                },
            },
            Arc::clone(&cancellation),
        );
        if self.ai_consent_is_for(AiFeature::Denoise) {
            self.ai.consent = None;
        }
        let progress = ForegroundProgress::indeterminate(if saved_result_exists {
            "Restoring saved AI denoise…"
        } else {
            "Preparing RawNIND models…"
        });
        self.begin_foreground_operation(ForegroundOperation {
            kind: DENOISE_JOB,
            document_id: self.persistence.document_generation,
            cancellation,
            progress,
            cancelling: false,
            receiver: ForegroundOperationReceiver::AiDenoise(receiver),
            context: ForegroundOperationContext::AiDenoise,
        });
        let changed = !self.develop.exposure.ai_denoise_enabled;
        self.develop.exposure.ai_denoise_enabled = true;
        self.develop.target_exposure.ai_denoise_enabled =
            self.preview_exposure().ai_denoise_enabled;
        if changed {
            self.note_edit_changed();
        }
        calibraw_core::diagnostics::record(format!(
            "RawNIND worker started for document {} on {}",
            self.persistence.document_generation,
            if cfg!(target_os = "android") {
                "Android"
            } else {
                "desktop"
            }
        ));
        self.egui_ctx.request_repaint();
    }

    pub(crate) fn poll_ai_denoise_worker(&mut self) {
        if !self.ai_denoise_running() {
            return;
        }
        let Some(mut operation) = self.foreground_operation.take() else {
            return;
        };
        let ForegroundOperationReceiver::AiDenoise(receiver) = &operation.receiver else {
            self.foreground_operation = Some(operation);
            return;
        };
        let (events, disconnected) = drain_worker_events(Some(receiver), |event| {
            matches!(event, AiDenoiseEvent::Finished(_))
        });
        let mut finished = None;
        let mut saved_result_unusable = false;
        let mut save_error = None;
        for event in events {
            match event {
                AiDenoiseEvent::SavedResultUnusable => saved_result_unusable = true,
                AiDenoiseEvent::ResultNotSaved(error) => save_error = Some(error),
                AiDenoiseEvent::DownloadProgress { downloaded, total } => {
                    operation.progress = ForegroundProgress::units(
                        downloaded,
                        total,
                        Some("bytes".to_owned()),
                        "Downloading verified RawNIND model package",
                    )
                    .with_detail(format!(
                        "{:.1} / {:.1} MB",
                        downloaded as f64 / 1_000_000.0,
                        total as f64 / 1_000_000.0
                    ));
                }
                AiDenoiseEvent::Progress {
                    phase,
                    completed,
                    total,
                } => {
                    operation.progress = if total > 0 {
                        ForegroundProgress::units(
                            completed as u64,
                            total as u64,
                            Some("tiles".to_owned()),
                            phase,
                        )
                    } else {
                        ForegroundProgress::indeterminate(format!("{phase}…"))
                    };
                }
                AiDenoiseEvent::Finished(result) => finished = Some(result),
            }
        }
        if disconnected && finished.is_none() {
            finished = Some(Err("RawNIND worker stopped unexpectedly.".to_owned()));
        }
        let Some(result) = finished else {
            self.foreground_operation = Some(operation);
            return;
        };
        let stale = !operation.is_for_document(self.persistence.document_generation);
        if stale {
            return;
        }
        self.preview.quality_dirty = true;
        self.discard_auxiliary_previews();
        self.preview.pending_stage = None;
        self.preview.detail_pending_stage = None;
        self.preview.navigation_pending_stage = None;
        self.preview.detail_urgent = false;
        match result {
            Ok(image) if self.develop.exposure.ai_denoise_enabled && !operation.is_cancelled() => {
                let install = self
                    .develop
                    .loaded_raw
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("the RAW was closed"))
                    .and_then(|raw| raw.set_ai_denoised_image(image));
                match install {
                    Ok(()) => {
                        self.ui.notice = Some(match save_error {
                            Some(error) => format!(
                                "AI denoise applied, but its result could not be saved next to the photo ({error}). It will need to run again the next time the photo is opened."
                            ),
                            None => "AI denoise applied locally. Standard denoise values were preserved."
                                .to_owned(),
                        });
                    }
                    Err(error) => {
                        let changed = self.develop.exposure.ai_denoise_enabled;
                        self.develop.exposure.ai_denoise_enabled = false;
                        self.develop.target_exposure.ai_denoise_enabled = false;
                        if changed {
                            self.note_edit_changed();
                        }
                        self.ui.notice = Some(format!("Could not install AI denoise: {error:#}"));
                    }
                }
            }
            Ok(_) => {
                self.develop.exposure.ai_denoise_enabled = false;
                self.develop.target_exposure.ai_denoise_enabled = false;
            }
            Err(_) if saved_result_unusable && !operation.is_cancelled() => {
                // The saved result no longer matches this RAW. Keep the edit and
                // ask before running the model again.
                self.ai.denoise_resume_pending = true;
            }
            Err(error) => {
                let changed = self.develop.exposure.ai_denoise_enabled;
                self.develop.exposure.ai_denoise_enabled = false;
                self.develop.target_exposure.ai_denoise_enabled = false;
                if changed {
                    self.note_edit_changed();
                }
                if !error.contains("cancelled") {
                    self.ui.notice = Some(format!("AI denoise failed: {error}"));
                }
            }
        }
        self.egui_ctx.request_repaint();
    }

    pub(crate) fn abandon_ai_denoise_worker(&mut self) {
        self.cancel_foreground_operation_if(DENOISE_JOB);
        if self.ai_consent_is_for(AiFeature::Denoise) {
            self.ai.consent = None;
        }
    }

    pub(crate) fn resume_persisted_ai_denoise(&mut self, frame: &eframe::Frame) {
        self.ai.denoise_resume_pending = false;
        if self.develop.exposure.ai_denoise_enabled && self.develop.loaded_raw.is_some() {
            if self.foreground_operation_active() {
                self.ai.denoise_resume_pending = true;
                return;
            }
            if self
                .develop
                .loaded_raw
                .as_ref()
                .is_some_and(|raw| raw.ai_denoised_image().is_some())
            {
                self.develop.target_exposure.ai_denoise_enabled =
                    self.preview_exposure().ai_denoise_enabled;
                calibraw_core::diagnostics::record(
                    "Restored the persisted AI-denoise scene without rerunning RawNIND",
                );
                return;
            }
            // Background opens (batch export, library AI refresh) must not stop
            // for a prompt; the result is restored the next time the image is
            // opened interactively.
            if self.document_load_is_background() {
                return;
            }
            self.enable_ai_denoise(frame, AiJobOrigin::Restore);
        }
    }

    /// Restores AI denoise that an undo or paste turned on, once the image is
    /// on screen so the prompt clearly refers to it.
    pub(crate) fn resume_pending_ai_denoise(&mut self, frame: &eframe::Frame) {
        if self.ai.denoise_resume_pending && self.ui.active_tab == AppTab::Develop {
            self.resume_persisted_ai_denoise(frame);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> CalibRawApp {
        let mut app = CalibRawApp::empty(&egui::Context::default());
        #[cfg(not(target_os = "android"))]
        {
            app.ai.runtime_mode = OnnxRuntimeMode::Automatic;
        }
        app
    }

    #[test]
    fn restoring_ai_denoise_always_asks_first() {
        let mut app = app();
        assert!(!app.ai_job_may_start_from(AiFeature::Denoise, AiJobOrigin::Restore));
        assert_eq!(
            app.ai
                .consent
                .map(|consent| (consent.feature, consent.origin)),
            Some((AiFeature::Denoise, AiJobOrigin::Restore))
        );
        assert!(!app.foreground_operation_active());
    }

    #[test]
    fn declining_a_restore_turns_ai_denoise_off_as_an_edit() {
        let mut app = app();
        app.develop.exposure.ai_denoise_enabled = true;
        app.reset_edit_history();
        let revision = app.edit_commit_revision();
        assert!(!app.ai_job_may_start_from(AiFeature::Denoise, AiJobOrigin::Restore));

        app.abandon_ai_job(AiFeature::Denoise);
        app.commit_edit_history_now();

        assert!(app.ai.consent.is_none());
        assert!(!app.develop.exposure.ai_denoise_enabled);
        assert_ne!(app.edit_commit_revision(), revision);
    }

    #[test]
    fn pending_restore_waits_until_the_image_is_on_screen() {
        let mut app = app();
        let frame = eframe::Frame::_new_kittest();
        app.ai.denoise_resume_pending = true;

        app.ui.active_tab = AppTab::Library;
        app.resume_pending_ai_denoise(&frame);
        assert!(app.ai.denoise_resume_pending);

        app.ui.active_tab = AppTab::Develop;
        app.resume_pending_ai_denoise(&frame);
        assert!(!app.ai.denoise_resume_pending);
    }
}
