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
    /// Every people mask: one people detection, then one body-part
    /// segmentation per person.
    People,
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
            .chain(dependencies.people.then_some(Self::People))
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
            Self::People => dependencies.people,
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
            || self.ai.object_pending_target.is_some()
            || !self.ai.people_pending.is_empty();
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
        if self.caps().configurable_ai_runtime
            && dependencies.needs_ai()
            && !self.validate_onnx_runtime_for_ai()
        {
            return;
        }

        self.masks.source_cache = None;
        self.masks.clear_generated_caches();
        self.ai.object_cache = None;
        self.forget_people();
        if let Err(error) = self.capture_mask_source(frame) {
            self.report_error(ErrorKind::AiUpdate, error);
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
                AiUpdateStep::People if !calibraw_ai::models::people::AVAILABLE => {
                    self.report_error(ErrorKind::AiUpdate, Self::PEOPLE_UNAVAILABLE.to_owned());
                    if let Some(update) = self.ai.update.as_mut() {
                        update.failed = true;
                    }
                    continue;
                }
                AiUpdateStep::People => {
                    self.queue_all_people_masks();
                    AiFeature::People
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
            self.report_error(ErrorKind::AiUpdate, message);
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
            self.report_error(
                ErrorKind::AiUpdate,
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
        self.ai.people_pending.clear();
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
        self.ai.people_pending.clear();
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
