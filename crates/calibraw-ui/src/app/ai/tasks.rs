//! Every local-AI job goes through one path, wherever in the UI it was asked
//! for: its inputs are staged, [`CalibRawApp::ai_job_may_start`] asks for
//! consent when a model or the runtime must be downloaded, and the job then
//! starts ([`CalibRawApp::start_ai_job`]) or is dropped
//! ([`CalibRawApp::abandon_ai_job`]). The open tab only decides which models
//! stay loaded between jobs; it never cancels a job.
//!
//! Work that runs a model outside the editor, such as a Library merge's AI
//! denoise, asks through the same consent dialog with
//! [`CalibRawApp::request_ai_consent`], naming itself as an
//! [`AiConsentRequester`], and takes the answer with
//! [`CalibRawApp::take_ai_consent_answer`]. It then runs the model itself,
//! downloading it only if the user accepted the download.

use super::*;
use crate::i18n::tr;
use calibraw_ai::{AiDownloads, AiFeature, AiFeatureSet};

/// Features whose results are derived from image content (masks and scene
/// depth). They go stale together when the source image changes.
pub(in crate::app) const fn is_content_feature(feature: AiFeature) -> bool {
    matches!(
        feature,
        AiFeature::Subject
            | AiFeature::Sky
            | AiFeature::SceneDepth
            | AiFeature::Object
            | AiFeature::People
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
        AiFeature::Object | AiFeature::People | AiFeature::Remove | AiFeature::Denoise => None,
    }
}

impl ForegroundOperationKind {
    pub(crate) const fn ai_feature(self) -> Option<AiFeature> {
        match self {
            Self::Ai(feature) => Some(feature),
            Self::LensCorrection | Self::AutoGeometry(_) | Self::CameraMatch => None,
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Ai(AiFeature::Subject) => tr("Preparing subject mask"),
            Self::Ai(AiFeature::Sky) => tr("Preparing sky mask"),
            Self::Ai(AiFeature::SceneDepth) => tr("Estimating scene depth"),
            Self::Ai(AiFeature::Object) => tr("Preparing object mask"),
            Self::Ai(AiFeature::People) => tr("Preparing people mask"),
            Self::Ai(AiFeature::Remove) => tr("Applying Remove"),
            Self::Ai(AiFeature::Denoise) => tr("Applying AI denoise"),
            Self::LensCorrection => tr("Applying lens correction"),
            Self::AutoGeometry(kind) => kind.title(),
            Self::CameraMatch => tr("Matching the camera's look"),
        }
    }
}

impl CalibRawApp {
    /// A local-AI job is running and may use the GPU, where the editor's own
    /// textures compete with it for memory.
    pub(crate) fn ai_job_may_use_gpu(&self) -> bool {
        let gpu = calibraw_ai::runtime::ai_sessions_may_use_gpu(self.manual_onnx_runtime());
        gpu && self
            .foreground_operation_kind()
            .and_then(ForegroundOperationKind::ai_feature)
            .is_some()
    }

    /// A mask segmentation or scene-depth job is running.
    pub(crate) fn content_job_active(&self) -> bool {
        self.foreground_operation_kind()
            .and_then(ForegroundOperationKind::ai_feature)
            .is_some_and(is_content_feature)
    }

    /// Whether the consent dialog asks about the editor's staged `feature` job.
    pub(crate) fn ai_consent_is_for(&self, feature: AiFeature) -> bool {
        self.ai.consent.is_some_and(|consent| {
            consent.feature == feature && consent.requester == AiConsentRequester::Editor
        })
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
                let precision = self.subject_model_precision();
                !self.generated_model_is_verified(
                    model,
                    precision,
                    &self.generated_model_path(model, precision),
                )
            }
            AiFeature::Object => {
                let (encoder, decoder) = self.sam21_model_paths();
                !calibraw_ai::models::object::object_models_are_verified(&encoder, &decoder)
            }
            AiFeature::People => !self.people_models().are_verified(),
            AiFeature::Remove => !calibraw_ai::models::remove::big_lama_model_is_verified(
                &self.big_lama_model_path(),
            ),
            AiFeature::Denoise => {
                !calibraw_ai::models::denoise::models_are_verified(&self.rawnind_model_dir())
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
        if let Some(consent) = self.consent_needed(feature, origin, AiConsentRequester::Editor) {
            self.ask_ai_consent(consent);
            return false;
        }
        if self.ai_consent_is_for(feature) {
            self.ai.consent = None;
        }
        true
    }

    /// The consent gate for a model run outside the editor by `requester`.
    /// `Ready` when nothing must be downloaded: the work may run its model now,
    /// without downloading. `Asking` while the consent dialog asks, whose
    /// answer [`Self::take_ai_consent_answer`] gives. `Unavailable` when the
    /// ONNX Runtime cannot be used, which has been reported to the user.
    pub(crate) fn request_ai_consent(
        &mut self,
        feature: AiFeature,
        requester: AiConsentRequester,
    ) -> AiConsentCheck {
        debug_assert_ne!(requester, AiConsentRequester::Editor);
        if !self.ai_runtime_ready() {
            return AiConsentCheck::Unavailable;
        }
        match self.consent_needed(feature, AiJobOrigin::Requested, requester) {
            Some(consent) => {
                self.ask_ai_consent(consent);
                AiConsentCheck::Asking
            }
            None => AiConsentCheck::Ready,
        }
    }

    /// The answer to `requester`'s [`Self::request_ai_consent`], taken once;
    /// `None` while the dialog still asks. A question withdrawn without an
    /// answer, as when another job's question replaced it or the runtime mode
    /// changed, answers `Declined`, so a requester never waits forever. Call
    /// it only while waiting for an answer.
    pub(crate) fn take_ai_consent_answer(
        &mut self,
        requester: AiConsentRequester,
    ) -> Option<AiConsentAnswer> {
        if let Some((asked, answer)) = self.ai.consent_answer {
            if asked == requester {
                self.ai.consent_answer = None;
                return Some(answer);
            }
        }
        let asking = self
            .ai
            .consent
            .is_some_and(|consent| consent.requester == requester);
        (!asking).then_some(AiConsentAnswer::Declined)
    }

    /// The question to ask before `feature` runs for `requester`: when its
    /// model or the ONNX Runtime must be downloaded, or the job restores an
    /// edit's result the user did not just ask for.
    fn consent_needed(
        &self,
        feature: AiFeature,
        origin: AiJobOrigin,
        requester: AiConsentRequester,
    ) -> Option<AiConsent> {
        let runtime_download_needed = self.automatic_onnx_runtime_download_needed();
        let model_download_needed = self.model_download_needed(feature);
        (runtime_download_needed || model_download_needed || origin == AiJobOrigin::Restore)
            .then_some(AiConsent {
                feature,
                model_download_needed,
                runtime_download_needed,
                origin,
                requester,
            })
    }

    /// Opens the consent dialog on `consent`, replacing a question it showed.
    fn ask_ai_consent(&mut self, consent: AiConsent) {
        self.ai.consent = Some(consent);
        self.ai.consent_answer = None;
        self.egui_ctx.request_repaint();
    }

    /// Gate and start for a content job; these need no GPU frame.
    pub(in crate::app) fn request_content_job(&mut self, feature: AiFeature) {
        if self.ai_job_may_start(feature) {
            self.start_content_job(feature, AiDownloads::NONE);
        }
    }

    /// Starts a staged job once its download was accepted.
    pub(in crate::app) fn start_ai_job(
        &mut self,
        feature: AiFeature,
        downloads: AiDownloads,
        frame: &eframe::Frame,
    ) {
        match feature {
            AiFeature::Remove => {
                if let Some(brush) = self.inpaint.pending_brush.take() {
                    let existing = self.inpaint.edits.as_ref().clone();
                    let _ = self.start_remove_request(frame, existing, brush, downloads);
                }
            }
            AiFeature::Denoise => self.start_ai_denoise(frame, downloads),
            AiFeature::Subject
            | AiFeature::Sky
            | AiFeature::SceneDepth
            | AiFeature::Object
            | AiFeature::People => self.start_content_job(feature, downloads),
        }
    }

    fn start_content_job(&mut self, feature: AiFeature, downloads: AiDownloads) {
        if let Some(model) = generated_model(feature) {
            self.start_generated_mask_worker(model, downloads);
        } else if feature == AiFeature::People {
            self.start_people_worker(downloads);
        } else if let Some((mask_index, component_index)) = self.ai.object_pending_target.take() {
            let (encoder, decoder) = self.sam21_model_paths();
            self.start_object_worker(mask_index, component_index, encoder, decoder, downloads);
        }
    }

    /// Drops a staged job that the user declined or that cannot run.
    pub(in crate::app) fn abandon_ai_job(&mut self, feature: AiFeature) {
        if self.ai_consent_is_for(feature) {
            self.ai.consent = None;
        }
        match feature {
            AiFeature::Object => self.ai.object_pending_target = None,
            AiFeature::People => self.ai.people.pending.clear(),
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
                AiFeature::People,
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
    fn declining_a_content_job_cancels_the_update_it_belongs_to() {
        let mut app = CalibRawApp::empty(&egui::Context::default());
        app.masks.stack.add_mask(MaskKind::Sky).unwrap();
        app.ai.update = Some(AiUpdate::planned(&app.masks.stack.content_dependencies()));
        app.ai.consent = Some(AiConsent {
            feature: AiFeature::Sky,
            model_download_needed: false,
            runtime_download_needed: true,
            origin: AiJobOrigin::Requested,
            requester: AiConsentRequester::Editor,
        });

        app.abandon_ai_job(AiFeature::Sky);

        assert!(app.ai.update.is_none());
        assert!(app.ai.update_needed);
    }
}
