//! People masks. The people in the mask source are found once and kept in
//! `AiState::people` with the body parts of each person segmented so far, so
//! choosing another person segments only them and choosing other parts runs
//! no model. Every change goes through one job that makes the masks of a list
//! of people components from their own person and parts.

use super::*;
use crate::pipeline::PersonPart;
use calibraw_ai::models::people::{
    people_models_are_verified, spawn_people_mask, DetectedPerson, PeopleMaskProgress,
    PeopleMaskRequest, PeopleMaskResult, PeopleMaskWorkerRequest, PeopleTarget, AVAILABLE,
};
use calibraw_ai::AiFeature;
use std::collections::BTreeSet;

const PEOPLE_JOB: ForegroundOperationKind = ForegroundOperationKind::Ai(AiFeature::People);
/// Height of a person's chip thumbnail, in pixels.
const CHIP_HEIGHT: u32 = 96;

impl CalibRawApp {
    /// Makes the masks of these people components, finding the people first
    /// unless they were found in the current mask source.
    pub(crate) fn request_people_masks(
        &mut self,
        components: Vec<(usize, usize)>,
        frame: &eframe::Frame,
    ) {
        self.ui.error_dialogs.dismiss_kind(ErrorKind::AiMask);
        if !AVAILABLE {
            self.report_ai_mask_error(Self::PEOPLE_UNAVAILABLE.to_owned());
            return;
        }
        if self.foreground_operation_active() {
            self.ui.notice =
                Some("Finish or cancel the current editing operation first.".to_owned());
            return;
        }
        if let Err(error) = self.capture_mask_source(frame) {
            self.report_ai_mask_error(error);
            return;
        }
        self.ai.people_pending = components;
        self.request_content_job(AiFeature::People);
    }

    pub(in crate::app) const PEOPLE_UNAVAILABLE: &str =
        "People masks are not available on this device. Existing people masks keep the result \
         they were made with.";

    /// Every people component of the stack, for an update.
    pub(in crate::app) fn queue_all_people_masks(&mut self) {
        self.ai.people_pending = self
            .masks
            .stack
            .masks
            .iter()
            .enumerate()
            .flat_map(|(mask_index, mask)| {
                mask.components
                    .iter()
                    .enumerate()
                    .filter(|(_, component)| component.kind == MaskKind::Person)
                    .map(move |(component_index, _)| (mask_index, component_index))
            })
            .collect();
    }

    /// Forgets the people found, when the mask source they were found in
    /// changes or goes: wherever `MaskState::source_cache` is replaced or
    /// cleared.
    pub(in crate::app) fn forget_people(&mut self) {
        self.ai.people = None;
        self.ai.people_chips.clear();
    }

    pub(in crate::app) fn start_people_worker(&mut self, allow_download: bool) {
        let components = std::mem::take(&mut self.ai.people_pending);
        if self.foreground_operation_active() || components.is_empty() {
            return;
        }
        let Some(source) = self.masks.source_cache.clone() else {
            self.report_error(
                ErrorKind::AiMask,
                "The preview could not be prepared for people selection.".to_owned(),
            );
            return;
        };
        let (targets, captured): (Vec<_>, Vec<_>) = components
            .into_iter()
            .filter_map(|(mask_index, component_index)| {
                let target = self.masks.capture_ai_target(mask_index, component_index)?;
                let MaskGeometry::Person { anchor, parts, .. } = &target.geometry else {
                    return None;
                };
                let people_target = PeopleTarget {
                    anchor: *anchor,
                    parts: parts.clone(),
                };
                Some((people_target, target))
            })
            .unzip();
        if captured.is_empty() {
            self.report_error(
                ErrorKind::AiMask,
                "The people mask is no longer available.".to_owned(),
            );
            return;
        }
        let analysis = self.ai.people.clone();
        let detects = analysis.is_none();
        let (detector_path, body_parts_path) = self.people_model_paths();
        let needs_download = !people_models_are_verified(&detector_path, &body_parts_path);
        let cancellation = Arc::new(AtomicBool::new(false));
        let receiver = spawn_people_mask(PeopleMaskWorkerRequest {
            detector_path,
            body_parts_path,
            allow_download,
            request: PeopleMaskRequest {
                source_width: source.width,
                source_height: source.height,
                source_rgba: source.rgba.to_vec(),
                analysis,
                targets,
            },
            cancellation: Arc::clone(&cancellation),
        });
        let phase = if needs_download {
            "Preparing people models…"
        } else if detects {
            "Finding people…"
        } else {
            "Updating the people mask…"
        };
        self.begin_foreground_operation(ForegroundOperation {
            kind: PEOPLE_JOB,
            document_id: self.persistence.document_generation,
            cancellation,
            progress: ForegroundProgress::indeterminate(phase),
            cancelling: false,
            receiver: ForegroundOperationReceiver::People(receiver),
            context: ForegroundOperationContext::People {
                targets: captured,
                detects,
            },
        });
    }

    pub(in crate::app) fn poll_people_worker(&mut self) {
        if !self.foreground_operation_is(PEOPLE_JOB) {
            return;
        }
        let Some(mut operation) = self.foreground_operation.take() else {
            return;
        };
        let ForegroundOperationReceiver::People(receiver) = &operation.receiver else {
            self.foreground_operation = Some(operation);
            return;
        };
        let finished = poll_ai_job(
            self.platform.as_ref(),
            receiver,
            || "The people-mask worker stopped unexpectedly.".to_owned(),
            |update| {
                operation.progress = match update {
                    AiJobUpdate::Download(progress) => model_download_progress(progress),
                    AiJobUpdate::Progress(PeopleMaskProgress::FindingPeople) => {
                        ForegroundProgress::indeterminate("Finding people…")
                    }
                    AiJobUpdate::Progress(PeopleMaskProgress::SegmentingBodyParts) => {
                        ForegroundProgress::indeterminate("Segmenting body parts…")
                    }
                }
            },
        );
        let Some(result) = finished else {
            self.foreground_operation = Some(operation);
            return;
        };
        let cancelled = operation.is_cancelled();
        let stale = !operation.is_for_document(self.persistence.document_generation);
        let ForegroundOperationContext::People { targets, detects } = operation.context else {
            return;
        };

        let mut error_message = None;
        if !cancelled && !stale {
            match result {
                Ok(result) => error_message = self.apply_people_masks(&targets, result, detects),
                Err(error) => error_message = Some(format!("People selection failed: {error}")),
            }
        }
        let succeeded = !cancelled && !stale && error_message.is_none();
        if self.ai.update.is_some() {
            self.advance_ai_update(succeeded, cancelled || stale, error_message);
        } else if let Some(message) = error_message {
            self.report_error(ErrorKind::AiMask, message);
        } else if stale && !cancelled {
            self.report_error(
                ErrorKind::AiMask,
                "People selection became stale before inference completed.".to_owned(),
            );
        }
        self.egui_ctx.request_repaint();
    }

    /// Writes each mask and its person's anchor to its component. Returns
    /// the error to report; finding no one for a mask that has not chosen a
    /// person yet is not one.
    fn apply_people_masks(
        &mut self,
        targets: &[AiMaskTarget],
        result: PeopleMaskResult,
        detects: bool,
    ) -> Option<String> {
        let PeopleMaskResult { analysis, masks } = result;
        if detects {
            self.ai.people_chips = match &self.masks.source_cache {
                Some(source) => person_chips(&self.egui_ctx, source, analysis.people()),
                None => Vec::new(),
            };
        }
        self.ai.people = Some(analysis);

        let mut error = None;
        let mut found_no_one = false;
        let mut changed_layers = BTreeSet::new();
        for (target, mask) in targets.iter().zip(masks) {
            let Some(mask) = mask else {
                // A refreshed mask keeps its last result; a new one stays empty.
                if matches!(
                    target.geometry,
                    MaskGeometry::Person {
                        anchor: Some(_),
                        ..
                    }
                ) {
                    error = Some(
                        "No one was found in this photo for a people mask; it keeps its last \
                         result."
                            .to_owned(),
                    );
                } else {
                    found_no_one = true;
                }
                continue;
            };
            let location = match self.masks.resolve_ai_target(target) {
                Ok(location) => location,
                Err(message) => {
                    error = Some(message);
                    continue;
                }
            };
            let anchor = mask.anchor;
            let Some(image) = mask.into_probability_mask() else {
                error = Some("People selection returned an invalid mask image.".to_owned());
                continue;
            };
            let (mask_index, component_index) = location;
            if let Some(MaskGeometry::Person {
                anchor: target_anchor,
                mask: target_mask,
                ..
            }) = self
                .masks
                .stack
                .masks
                .get_mut(mask_index)
                .and_then(|local| local.components.get_mut(component_index))
                .map(|component| &mut component.geometry)
            {
                *target_anchor = Some(anchor);
                *target_mask = Some(image);
                changed_layers.insert(mask_index);
            }
        }
        if found_no_one && self.ai.update.is_none() {
            self.ui.notice = Some("No people were found in this photo.".to_owned());
        }
        if !changed_layers.is_empty() {
            for mask_index in changed_layers {
                self.mark_mask_geometry_dirty(mask_index);
            }
            self.blink_selected_component();
        }
        error
    }

    /// The person a people component selects, by index into
    /// `AiState::people`, while those are known.
    pub(crate) fn selected_person(&self, geometry: &MaskGeometry) -> Option<usize> {
        let MaskGeometry::Person {
            anchor: Some(anchor),
            ..
        } = geometry
        else {
            return None;
        };
        self.ai.people.as_ref()?.person_at(*anchor)
    }

    /// Keeps the first part of a people component and moves each other part
    /// to a mask group of its own for the same person, so each part takes its
    /// own adjustments, then makes their masks.
    pub(crate) fn separate_person_parts(
        &mut self,
        mask_index: usize,
        component_index: usize,
        frame: &eframe::Frame,
    ) {
        let Some(MaskGeometry::Person {
            anchor,
            parts,
            grow,
            feather,
            ..
        }) = self
            .masks
            .stack
            .masks
            .get(mask_index)
            .and_then(|mask| mask.components.get(component_index))
            .map(|component| &component.geometry)
        else {
            return;
        };
        let (anchor, grow, feather) = (*anchor, *grow, *feather);
        let mut parts = parts.iter().copied();
        let Some(first) = parts.next() else {
            return;
        };
        let others: Vec<PersonPart> = parts.collect();
        if others.is_empty() {
            return;
        }

        let mut candidate = self.masks.stack.clone();
        if let Some(MaskGeometry::Person { parts, .. }) = candidate
            .masks
            .get_mut(mask_index)
            .and_then(|mask| mask.components.get_mut(component_index))
            .map(|component| &mut component.geometry)
        {
            *parts = BTreeSet::from([first]);
        }
        let mut components = vec![(mask_index, component_index)];
        for part in others {
            let Some((new_mask, new_component)) = candidate.add_mask(MaskKind::Person) else {
                self.report_error(
                    ErrorKind::EditNotApplied,
                    format!(
                        "The parts were not separated: a photo holds at most {MAX_LOCAL_MASKS} mask groups."
                    ),
                );
                return;
            };
            let group = &mut candidate.masks[new_mask];
            group.common.rename(part.label());
            group.components[new_component].geometry = MaskGeometry::Person {
                anchor,
                parts: BTreeSet::from([part]),
                mask: None,
                grow,
                feather,
            };
            components.push((new_mask, new_component));
        }
        candidate.select_component(mask_index, component_index);
        if let Err(error) = crate::sidecar::preflight_mask_change(&candidate) {
            self.report_mask_persistence_limit("Separating the parts", &error);
            return;
        }
        self.masks.stack = candidate;
        self.mark_all_mask_layers_dirty();
        self.sync_selected_mask_tool();
        self.request_people_masks(components, frame);
    }
}

/// A thumbnail of each person, cut from the mask source at their box.
fn person_chips(
    ctx: &egui::Context,
    source: &MaskRgbImage,
    people: &[DetectedPerson],
) -> Vec<egui::TextureHandle> {
    let Some(image) = image::ImageBuffer::<image::Rgba<u8>, &[u8]>::from_raw(
        source.width,
        source.height,
        &source.rgba,
    ) else {
        return Vec::new();
    };
    people
        .iter()
        .enumerate()
        .map(|(index, person)| {
            let bounds = person.bounds;
            let crop =
                image::imageops::crop_imm(&image, bounds.x, bounds.y, bounds.width, bounds.height);
            // At most square, so a person lying down keeps a usable chip.
            let height = CHIP_HEIGHT.min(bounds.height.max(1));
            let width = ((u64::from(bounds.width) * u64::from(height)
                / u64::from(bounds.height.max(1))) as u32)
                .clamp(1, height);
            let thumbnail = image::imageops::thumbnail(&*crop, width, height);
            let pixels = egui::ColorImage::from_rgba_unmultiplied(
                [thumbnail.width() as usize, thumbnail.height() as usize],
                thumbnail.as_raw(),
            );
            ctx.load_texture(
                format!("people-chip-{index}"),
                pixels,
                egui::TextureOptions::LINEAR,
            )
        })
        .collect()
}
