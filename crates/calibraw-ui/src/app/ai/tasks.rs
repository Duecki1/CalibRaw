//! Every local-AI job goes through one path, wherever in the UI it was asked
//! for: its inputs are staged, [`CalibRawApp::ai_job_may_start`] asks for
//! consent when a model or the runtime must be downloaded, and the job then
//! starts ([`CalibRawApp::start_ai_job`]) or is dropped
//! ([`CalibRawApp::abandon_ai_job`]). The open tab only decides which models
//! stay loaded between jobs; it never cancels a job.

use super::*;
use calibraw_ai::{AiFeature, AiFeatureSet};

/// Features whose results are derived from image content (masks and scene
/// depth). They go stale together when the source image changes.
pub(in crate::app) const fn is_content_feature(feature: AiFeature) -> bool {
    matches!(
        feature,
        AiFeature::Subject | AiFeature::Sky | AiFeature::SceneDepth | AiFeature::Object
    )
}

pub(in crate::app) const fn generated_feature(model: AiMaskModel) -> AiFeature {
    match model {
        AiMaskModel::Subject => AiFeature::Subject,
        AiMaskModel::Sky => AiFeature::Sky,
        AiMaskModel::Depth => AiFeature::SceneDepth,
    }
}

pub(in crate::app) const fn generated_model(feature: AiFeature) -> Option<AiMaskModel> {
    match feature {
        AiFeature::Subject => Some(AiMaskModel::Subject),
        AiFeature::Sky => Some(AiMaskModel::Sky),
        AiFeature::SceneDepth => Some(AiMaskModel::Depth),
        AiFeature::Object | AiFeature::Remove | AiFeature::Denoise => None,
    }
}

impl ForegroundOperationKind {
    pub(crate) const fn ai_feature(self) -> Option<AiFeature> {
        match self {
            Self::Ai(feature) => Some(feature),
            Self::LensCorrection => None,
        }
    }

    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Ai(AiFeature::Subject) => "Preparing subject mask",
            Self::Ai(AiFeature::Sky) => "Preparing sky mask",
            Self::Ai(AiFeature::SceneDepth) => "Estimating scene depth",
            Self::Ai(AiFeature::Object) => "Preparing object mask",
            Self::Ai(AiFeature::Remove) => "Applying Remove",
            Self::Ai(AiFeature::Denoise) => "Applying AI denoise",
            Self::LensCorrection => "Applying lens correction",
        }
    }
}

impl CalibRawApp {
    /// A mask segmentation or scene-depth job is running.
    pub(crate) fn content_job_active(&self) -> bool {
        self.foreground_operation_kind()
            .and_then(ForegroundOperationKind::ai_feature)
            .is_some_and(is_content_feature)
    }

    pub(crate) fn ai_consent_is_for(&self, feature: AiFeature) -> bool {
        self.ai
            .consent
            .is_some_and(|consent| consent.feature == feature)
    }

    pub(crate) fn content_consent_open(&self) -> bool {
        self.ai
            .consent
            .is_some_and(|consent| is_content_feature(consent.feature))
    }

    pub(in crate::app) fn model_download_needed(&self, feature: AiFeature) -> bool {
        match feature {
            AiFeature::Subject | AiFeature::Sky | AiFeature::SceneDepth => {
                let model = generated_model(feature).expect("generated feature has a model");
                !self.generated_model_is_verified(model, &self.generated_model_path(model))
            }
            AiFeature::Object => {
                let (encoder, decoder) = self.sam21_model_paths();
                !calibraw_ai::ai_masks::object_models_are_verified(&encoder, &decoder)
            }
            AiFeature::Remove => {
                !calibraw_ai::remove::big_lama_model_is_verified(&self.big_lama_model_path())
            }
            AiFeature::Denoise => {
                !calibraw_ai::ai_denoise::models_are_verified(&self.rawnind_model_dir())
            }
        }
    }

    /// The single gate every local-AI job passes, with its inputs (mask
    /// source, object target, brush) already staged. Returns whether a job the
    /// user just asked for may start now; otherwise it waits for consent, or
    /// could not run and was abandoned.
    pub(in crate::app) fn ai_job_may_start(&mut self, feature: AiFeature) -> bool {
        self.ai_job_may_start_from(feature, AiJobOrigin::Requested)
    }

    /// Like [`Self::ai_job_may_start`], but a restored job always asks first
    /// because it re-runs a model the user did not just ask for.
    pub(in crate::app) fn ai_job_may_start_from(
        &mut self,
        feature: AiFeature,
        origin: AiJobOrigin,
    ) -> bool {
        if !self.ai_runtime_ready() {
            self.abandon_ai_job(feature);
            return false;
        }
        let runtime_download_needed = self.automatic_onnx_runtime_download_needed();
        if runtime_download_needed
            || self.model_download_needed(feature)
            || origin == AiJobOrigin::Restore
        {
            self.ai.consent = Some(AiConsent {
                feature,
                runtime_download_needed,
                origin,
            });
            self.egui_ctx.request_repaint();
            return false;
        }
        if self.ai_consent_is_for(feature) {
            self.ai.consent = None;
        }
        true
    }

    /// Gate and start for a content job; these need no GPU frame.
    pub(in crate::app) fn request_content_job(&mut self, feature: AiFeature) {
        if self.ai_job_may_start(feature) {
            self.start_content_job(feature, false);
        }
    }

    /// Starts a staged job once its download was accepted.
    pub(in crate::app) fn start_ai_job(
        &mut self,
        feature: AiFeature,
        allow_download: bool,
        frame: &eframe::Frame,
    ) {
        match feature {
            AiFeature::Remove => {
                if let Some(brush) = self.inpaint.pending_brush.take() {
                    let existing = self.inpaint.edits.as_ref().clone();
                    let _ = self.start_remove_request(frame, existing, brush, allow_download);
                }
            }
            AiFeature::Denoise => self.start_ai_denoise(frame, allow_download),
            AiFeature::Subject | AiFeature::Sky | AiFeature::SceneDepth | AiFeature::Object => {
                self.start_content_job(feature, allow_download);
            }
        }
    }

    fn start_content_job(&mut self, feature: AiFeature, allow_download: bool) {
        if let Some(model) = generated_model(feature) {
            self.start_generated_mask_worker(model, allow_download);
        } else if let Some((mask_index, component_index)) = self.ai.object_pending_target.take() {
            let (encoder, decoder) = self.sam21_model_paths();
            self.start_object_worker(
                mask_index,
                component_index,
                encoder,
                decoder,
                allow_download,
            );
        }
    }

    /// Drops a staged job that the user declined or that cannot run.
    pub(in crate::app) fn abandon_ai_job(&mut self, feature: AiFeature) {
        if self.ai_consent_is_for(feature) {
            self.ai.consent = None;
        }
        match feature {
            AiFeature::Object => self.ai.object_pending_target = None,
            AiFeature::Remove => {
                self.inpaint.pending_brush = None;
                self.inpaint.pending_retouch = None;
                self.inpaint.last_brush_uv = None;
            }
            AiFeature::Denoise => {
                // Declining a restore turns off a saved edit, so record it.
                let changed = self.develop.exposure.ai_denoise_enabled;
                self.develop.exposure.ai_denoise_enabled = false;
                self.develop.target_exposure.ai_denoise_enabled = false;
                if changed {
                    self.note_edit_changed();
                }
            }
            AiFeature::Subject | AiFeature::Sky | AiFeature::SceneDepth => {}
        }
        if is_content_feature(feature) && self.ai.update.is_some() {
            self.cancel_ai_update();
        }
    }

    /// Features whose models may stay loaded between jobs: those whose tools
    /// are on screen. Every other model unloads as soon as its job ends.
    fn warm_ai_features(&self) -> AiFeatureSet {
        if self.ui.active_tab != AppTab::Develop {
            return AiFeatureSet::EMPTY;
        }
        match self.ui.sidebar_tab {
            SidebarTab::Masks => [
                AiFeature::Subject,
                AiFeature::Sky,
                AiFeature::SceneDepth,
                AiFeature::Object,
            ]
            .into_iter()
            .collect(),
            SidebarTab::Inpainting => AiFeatureSet::EMPTY.with(AiFeature::Remove),
            SidebarTab::Adjustments
            | SidebarTab::Presets
            | SidebarTab::Crop
            | SidebarTab::Export
            | SidebarTab::Info => AiFeatureSet::EMPTY,
        }
    }

    pub(crate) fn sync_ai_runtime(&self) {
        calibraw_ai::set_warm_ai_features(self.warm_ai_features());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running_job(app: &mut CalibRawApp, feature: AiFeature) -> Arc<AtomicBool> {
        let (_sender, receiver) = std::sync::mpsc::channel::<AiMaskEvent>();
        let cancellation = Arc::new(AtomicBool::new(false));
        assert!(app.begin_foreground_operation(ForegroundOperation {
            kind: ForegroundOperationKind::Ai(feature),
            document_id: app.persistence.document_generation,
            cancellation: Arc::clone(&cancellation),
            progress: ForegroundProgress::indeterminate("Testing"),
            cancelling: false,
            receiver: ForegroundOperationReceiver::AiMask(receiver),
            context: ForegroundOperationContext::AiMask,
        }));
        cancellation
    }

    #[test]
    fn navigation_never_cancels_a_running_job() {
        for feature in [AiFeature::Subject, AiFeature::SceneDepth, AiFeature::Object] {
            let mut app = CalibRawApp::empty(&egui::Context::default());
            app.ui.active_tab = AppTab::Develop;
            app.ui.sidebar_tab = SidebarTab::Masks;
            let cancellation = running_job(&mut app, feature);

            app.ui.sidebar_tab = SidebarTab::Inpainting;
            app.sync_ai_runtime();
            app.activate_tab(AppTab::Library);

            assert!(app.foreground_operation_is(ForegroundOperationKind::Ai(feature)));
            assert!(!cancellation.load(std::sync::atomic::Ordering::Acquire));
        }
    }

    #[test]
    fn on_screen_tools_keep_their_models_warm() {
        let mut app = CalibRawApp::empty(&egui::Context::default());
        assert_eq!(app.warm_ai_features(), AiFeatureSet::EMPTY);

        app.ui.active_tab = AppTab::Develop;
        app.ui.sidebar_tab = SidebarTab::Masks;
        let masks = app.warm_ai_features();
        for feature in AiFeature::ALL {
            assert_eq!(masks.contains(feature), is_content_feature(feature));
        }

        app.ui.sidebar_tab = SidebarTab::Inpainting;
        assert_eq!(
            app.warm_ai_features(),
            AiFeatureSet::EMPTY.with(AiFeature::Remove)
        );

        app.ui.sidebar_tab = SidebarTab::Adjustments;
        assert_eq!(app.warm_ai_features(), AiFeatureSet::EMPTY);
    }

    #[test]
    fn declining_consent_drops_the_staged_job() {
        let mut app = CalibRawApp::empty(&egui::Context::default());
        app.develop.exposure.ai_denoise_enabled = true;
        app.ai.object_pending_target = Some((0, 0));
        app.inpaint.pending_brush = Some(crate::pipeline::RemoveBrushStroke::default());

        for feature in [AiFeature::Denoise, AiFeature::Object, AiFeature::Remove] {
            app.ai.consent = Some(AiConsent {
                feature,
                runtime_download_needed: false,
                origin: AiJobOrigin::Requested,
            });
            app.abandon_ai_job(feature);
            assert!(app.ai.consent.is_none());
        }
        assert!(!app.develop.exposure.ai_denoise_enabled);
        assert!(app.ai.object_pending_target.is_none());
        assert!(app.inpaint.pending_brush.is_none());
    }

    #[test]
    fn declining_a_content_job_cancels_the_update_it_belongs_to() {
        let mut app = CalibRawApp::empty(&egui::Context::default());
        app.masks.stack.add_mask(MaskKind::Sky).unwrap();
        app.ai.update = Some(AiUpdate::planned(&app.masks.stack.content_dependencies()));
        app.ai.consent = Some(AiConsent {
            feature: AiFeature::Sky,
            runtime_download_needed: true,
            origin: AiJobOrigin::Requested,
        });

        app.abandon_ai_job(AiFeature::Sky);

        assert!(app.ai.update.is_none());
        assert!(app.ai.update_needed);
    }
}
