//! Refreshing everything an edit derives from image content (AI masks, scene
//! depth for masks and effects, and range-mask sources) after that image
//! changed. Updates only run when the user asks for one; source changes,
//! loads and pastes just mark the results stale.

use super::*;
use crate::pipeline::ContentDependencies;
use calibraw_ai::AiFeature;

/// One pass over every content result, run one model job at a time.
pub(crate) struct AiUpdate {
    steps: VecDeque<AiUpdateStep>,
    failed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AiUpdateStep {
    /// A shared result: one subject, sky or scene-depth run serves every user.
    Generated(AiMaskModel),
    Object {
        mask_index: usize,
        component_index: usize,
    },
}

impl AiUpdate {
    pub(in crate::app) fn planned(dependencies: &ContentDependencies) -> Self {
        Self {
            steps: AiUpdateStep::plan(dependencies),
            failed: false,
        }
    }
}

impl AiUpdateStep {
    fn plan(dependencies: &ContentDependencies) -> VecDeque<Self> {
        let generated = [
            (dependencies.subject, AiMaskModel::Subject),
            (dependencies.sky, AiMaskModel::Sky),
            (dependencies.scene_depth, AiMaskModel::Depth),
        ];
        generated
            .into_iter()
            .filter_map(|(needed, model)| needed.then_some(Self::Generated(model)))
            .chain(
                dependencies
                    .objects
                    .iter()
                    .map(|&(mask_index, component_index)| Self::Object {
                        mask_index,
                        component_index,
                    }),
            )
            .collect()
    }

    fn still_needed(self, dependencies: &ContentDependencies) -> bool {
        match self {
            Self::Generated(AiMaskModel::Subject) => dependencies.subject,
            Self::Generated(AiMaskModel::Sky) => dependencies.sky,
            Self::Generated(AiMaskModel::Depth) => dependencies.scene_depth,
            Self::Object {
                mask_index,
                component_index,
            } => dependencies
                .objects
                .contains(&(mask_index, component_index)),
        }
    }
}

impl CalibRawApp {
    /// Content results are stale, or something requires scene depth that the
    /// image does not have yet.
    pub(crate) fn ai_update_needed(&self) -> bool {
        self.ai.update_needed || self.masks.stack.scene_depth_missing()
    }

    pub(crate) fn ai_update_busy(&self) -> bool {
        self.ai.update.is_some() || self.content_job_active() || self.content_consent_open()
    }

    /// Model runs left in the current update, including the one in flight.
    pub(crate) fn ai_update_remaining_runs(&self) -> usize {
        let Some(update) = self.ai.update.as_ref() else {
            return 0;
        };
        let in_flight = self.content_job_active()
            || self.content_consent_open()
            || self.ai.object_pending_target.is_some();
        update.steps.len() + usize::from(in_flight)
    }

    pub(crate) fn request_ai_update(&mut self, frame: &eframe::Frame) {
        if self.ai_update_busy() {
            self.ui.notice = Some("Wait for the current AI operation to finish.".to_owned());
            return;
        }
        let dependencies = self.masks.stack.content_dependencies();
        if dependencies.is_empty() {
            self.save_completed_ai_update();
            return;
        }
        #[cfg(not(target_os = "android"))]
        if dependencies.needs_ai() && !self.validate_onnx_runtime_for_ai() {
            return;
        }

        self.masks.source_cache = None;
        self.masks.clear_generated_caches();
        self.ai.object_cache = None;
        if let Err(error) = self.capture_mask_source(frame) {
            self.ui.notice = Some(error);
            return;
        }
        self.refresh_range_mask_sources();

        if !dependencies.needs_ai() {
            self.save_completed_ai_update();
            self.ui.notice = Some("AI results were refreshed for the current image.".to_owned());
            self.egui_ctx.request_repaint();
            return;
        }
        self.ai.update = Some(AiUpdate::planned(&dependencies));
        self.continue_ai_update();
    }

    fn refresh_range_mask_sources(&mut self) {
        let source = self.masks.source_cache.clone();
        let mut changed_layers = Vec::new();
        for (mask_index, mask) in self.masks.stack.masks.iter_mut().enumerate() {
            let mut changed = false;
            for component in &mut mask.components {
                if let MaskGeometry::LuminanceRange { source: target, .. }
                | MaskGeometry::ColorRange { source: target, .. } = &mut component.geometry
                {
                    *target = source.clone();
                    changed = true;
                }
            }
            if changed {
                changed_layers.push(mask_index);
            }
        }
        for mask_index in changed_layers {
            self.mark_mask_geometry_dirty(mask_index);
        }
    }

    /// Starts the next step once the previous job and its consent are done.
    pub(in crate::app) fn continue_ai_update(&mut self) {
        while !self.content_job_active() && !self.content_consent_open() {
            let dependencies = self.masks.stack.content_dependencies();
            let Some(step) = self
                .ai
                .update
                .as_mut()
                .and_then(|update| update.steps.pop_front())
            else {
                break;
            };
            if !step.still_needed(&dependencies) {
                continue;
            }
            let feature = match step {
                AiUpdateStep::Generated(model) => generated_feature(model),
                AiUpdateStep::Object {
                    mask_index,
                    component_index,
                } => {
                    self.ai.object_pending_target = Some((mask_index, component_index));
                    AiFeature::Object
                }
            };
            self.request_content_job(feature);
            if self.content_job_active() || self.content_consent_open() {
                return;
            }
            // The job could not start and reported why; abandoning it may also
            // have ended the update.
            let Some(update) = self.ai.update.as_mut() else {
                return;
            };
            update.failed = true;
        }
        if !self.content_job_active() && !self.content_consent_open() {
            self.finish_ai_update();
        }
    }

    /// Records how a content job that belongs to the running update ended.
    pub(in crate::app) fn advance_ai_update(
        &mut self,
        succeeded: bool,
        interrupted: bool,
        error_message: Option<String>,
    ) {
        if interrupted {
            self.cancel_ai_update();
            return;
        }
        if let Some(update) = self.ai.update.as_mut() {
            update.failed |= !succeeded;
        }
        if let Some(message) = error_message {
            self.ui.notice = Some(message);
        }
        self.continue_ai_update();
    }

    /// Skips the remaining objects after one failed: they would fail the same way.
    pub(in crate::app) fn skip_remaining_object_updates(&mut self) {
        if let Some(update) = self.ai.update.as_mut() {
            update
                .steps
                .retain(|step| !matches!(step, AiUpdateStep::Object { .. }));
        }
    }

    fn finish_ai_update(&mut self) {
        let Some(update) = self.ai.update.take() else {
            return;
        };
        if update.failed {
            self.ai.update_needed = true;
            self.ui.notice = Some(
                "Some AI results could not be updated. The update stays available.".to_owned(),
            );
        } else {
            self.save_completed_ai_update();
            self.ui.notice = Some("AI results were refreshed for the current image.".to_owned());
        }
        self.egui_ctx.request_repaint();
    }

    fn save_completed_ai_update(&mut self) {
        self.ai.update_needed = false;
        // Refreshing may reproduce identical pixels, leaving the edit revision
        // unchanged. Still persist the cleared flag, including after a pending
        // save that captured the old flag.
        self.queue_explicit_sidecar_save();
    }

    pub(in crate::app) fn cancel_ai_update(&mut self) {
        self.ai.update = None;
        self.ai.object_pending_target = None;
        self.ai.update_needed = true;
        self.ui.notice = Some("AI update canceled.".to_owned());
        self.egui_ctx.request_repaint();
    }

    /// Forgets update progress and staged content jobs, e.g. when the document
    /// or its mask stack is replaced.
    pub(in crate::app) fn reset_ai_update_state(&mut self) {
        self.ai.update = None;
        if self.content_consent_open() {
            self.ai.consent = None;
        }
        self.ai.object_pending_target = None;
        self.ai.object_cache = None;
    }

    /// Requests content an edit newly depends on, such as scene depth after
    /// depth fog is turned on. Results that were already required are left
    /// to the update flow, so reloads and pastes never start a model.
    pub(crate) fn request_new_content_dependencies(
        &mut self,
        before: &ContentDependencies,
        frame: &eframe::Frame,
    ) {
        if self.ai_update_busy() || self.foreground_operation_active() {
            return;
        }
        let now = self.masks.stack.content_dependencies();
        let newly_required = [
            (now.subject && !before.subject, AiMaskModel::Subject),
            (now.sky && !before.sky, AiMaskModel::Sky),
            (now.scene_depth && !before.scene_depth, AiMaskModel::Depth),
        ];
        if let Some((_, model)) = newly_required
            .into_iter()
            .find(|&(new, model)| new && self.generated_result_missing(model))
        {
            let _ = self.request_generated_mask(model, frame);
        }
    }

    fn generated_result_missing(&self, model: AiMaskModel) -> bool {
        if model == AiMaskModel::Depth {
            return self.masks.stack.scene_depth_image().is_none();
        }
        self.masks
            .stack
            .masks
            .iter()
            .flat_map(|mask| &mask.components)
            .any(|component| {
                generated_mask_model(component.kind) == Some(model)
                    && matches!(component.geometry, MaskGeometry::Ai { mask: None, .. })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_plan_covers_masks_and_depth_effects_once_each() {
        let mut stack = crate::pipeline::MaskStack::default();
        for kind in [
            MaskKind::Subject,
            MaskKind::Background,
            MaskKind::Sky,
            MaskKind::DepthRange,
        ] {
            stack.add_mask(kind).unwrap();
        }
        stack
            .global_effects
            .push(crate::pipeline::EffectComponent::new(
                crate::pipeline::MaskEffect::Fog,
            ));
        let plan = AiUpdateStep::plan(&stack.content_dependencies());
        assert_eq!(
            plan,
            [
                AiUpdateStep::Generated(AiMaskModel::Subject),
                AiUpdateStep::Generated(AiMaskModel::Sky),
                AiUpdateStep::Generated(AiMaskModel::Depth),
            ]
        );

        // Depth fog alone still schedules scene depth.
        stack.masks.clear();
        assert_eq!(
            AiUpdateStep::plan(&stack.content_dependencies()),
            [AiUpdateStep::Generated(AiMaskModel::Depth)]
        );
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn ai_update_persists_completion_when_pixels_are_unchanged() {
        let mut app = CalibRawApp::empty(&egui::Context::default());
        let directory = tempfile::tempdir().unwrap();
        app.persistence.sidecar_target = Some(crate::sidecar::SidecarTarget::Desktop {
            raw_path: directory.path().join("copied-masks.ARW"),
        });
        app.masks.stack.add_mask(MaskKind::Object);
        app.reset_edit_history();
        let revision = app.edit_commit_revision();
        app.persistence.sidecar_saved_revision = Some(revision);
        // Keep the new save queued behind a save of the same edit revision.
        app.persistence.sidecar_in_flight = Some(SidecarSaveJob {
            generation: app.persistence.document_generation,
            revision,
            explicit: false,
        });
        app.ai.update_needed = true;
        app.ai.update = Some(AiUpdate {
            steps: VecDeque::new(),
            failed: false,
        });

        app.finish_ai_update();

        assert!(!app.ai.update_needed);
        assert!(app.ai.update.is_none());
        assert_eq!(app.edit_commit_revision(), revision);
        let save = app.persistence.sidecar_pending.front().unwrap();
        assert!(!save.edits.ai_masks_need_update);
        assert_eq!(save.revision, revision);
        assert_eq!(save.edits.masks.masks, app.masks.stack.masks);
    }
}
