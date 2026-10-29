//! People masks. The first people job on a photo finds everyone in the mask
//! source and segments them all. `PeopleState::analysis` keeps it while
//! the photo's content is unchanged, and the job keeps it beside the RAW for
//! the next time the photo opens. A mask of any person and parts is then made
//! from the analysis on a worker thread (`PeopleCompose`), without a model or
//! a progress dialog; only a person not segmented yet goes through the job.

use super::*;
use crate::app::{PeopleChoices, PeopleEdit, PersonThumbnail};
use crate::i18n::{tr, tr_fmt};
use crate::pipeline::PersonPart;
use calibraw_ai::models::people::{
    spawn_people_mask, PeopleAnalysis, PeopleAnalysisFile, PeopleMaskProgress, PeopleMaskRequest,
    PeopleMaskWorkerRequest, PeopleModels, PeopleSegmentation, PeopleSource, PeopleTarget,
    PersonBounds, PersonMask,
};
use calibraw_ai::AiFeature;
use image::{Rgba, RgbaImage};
use std::collections::BTreeSet;

const PEOPLE_JOB: ForegroundOperationKind = ForegroundOperationKind::Ai(AiFeature::People);
/// Edge of a person's square chip thumbnail, in pixels.
const CHIP_EDGE: u32 = 96;
/// Opacity of a person's canvas highlight where they certainly are, of 255.
const HIGHLIGHT_ALPHA: u16 = 115;

/// A people component's people and parts.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PersonChoice {
    /// As `MaskGeometry::Person` stores them.
    pub(crate) anchors: Vec<[f32; 2]>,
    pub(crate) parts: BTreeSet<PersonPart>,
}

impl PersonChoice {
    fn of(geometry: &MaskGeometry) -> Option<Self> {
        match geometry {
            MaskGeometry::Person { anchors, parts, .. } => Some(Self {
                anchors: anchors.clone(),
                parts: parts.clone(),
            }),
            _ => None,
        }
    }

    /// The choice after `edit`, or `None` when it changes nothing. The last
    /// person and the last part stay, and the entire person replaces the
    /// other parts, which replace it in turn. People are toggled as
    /// `analysis` found them, so a toggle needs it.
    fn edited(&self, edit: PeopleEdit, analysis: Option<&PeopleAnalysis>) -> Option<Self> {
        let mut next = self.clone();
        match edit {
            PeopleEdit::TogglePerson(person) => {
                let analysis = analysis?;
                let mut people = analysis.resolve(&self.anchors);
                if let Some(position) = people.iter().position(|&chosen| chosen == person) {
                    if people.len() == 1 {
                        return None;
                    }
                    people.remove(position);
                } else if person < analysis.people().len() {
                    people.push(person);
                    people.sort_unstable();
                } else {
                    return None;
                }
                next.anchors = people
                    .iter()
                    .filter_map(|&chosen| analysis.people().get(chosen))
                    .map(|found| found.anchor)
                    .collect();
            }
            PeopleEdit::TogglePart(PersonPart::EntirePerson) => {
                next.parts = BTreeSet::from([PersonPart::EntirePerson]);
            }
            PeopleEdit::TogglePart(part) => {
                next.parts.remove(&PersonPart::EntirePerson);
                if !next.parts.remove(&part) {
                    next.parts.insert(part);
                } else if next.parts.is_empty() {
                    return None;
                }
            }
        }
        (next != *self).then_some(next)
    }
}

/// One person's canvas highlight, kept while they stay highlighted.
pub(crate) struct PersonHighlight {
    person: usize,
    color: egui::Color32,
    pub(crate) texture: egui::TextureHandle,
    /// The part of the photo the texture spans, as fractions.
    pub(crate) region: [f32; 4],
}

/// A people mask being made from the analysis on a worker thread.
pub(crate) struct PeopleCompose {
    /// The component's mask and component index.
    location: (usize, usize),
    /// The component's choice when this started. A component changed since,
    /// by an undo for example, keeps its change.
    base: PersonChoice,
    /// The choice the mask is of, which the component takes with the mask.
    target: PersonChoice,
    receiver: mpsc::Receiver<Result<Option<PersonMask>, String>>,
}

impl PeopleCompose {
    fn is_for(&self, location: (usize, usize)) -> bool {
        self.location == location
    }
}

/// The people in the open photo, what is shown of them, and the people
/// masks being made from them.
#[derive(Default)]
pub(crate) struct PeopleState {
    /// People components waiting for their job to start.
    pub(in crate::app) pending: Vec<(usize, usize)>,
    /// The people found in the photo and the body parts of those segmented,
    /// while the content they were found in is unchanged.
    pub(crate) analysis: Option<PeopleAnalysis>,
    /// A thumbnail of each person in `analysis`, once made from the mask
    /// source.
    chips: Option<Vec<egui::TextureHandle>>,
    /// People masks being made from `analysis`.
    pub(in crate::app) composes: Vec<PeopleCompose>,
    /// The person whose chip the pointer is over in the mask panel, for the
    /// canvas to highlight. Set while the panel shows; the canvas takes it.
    pub(crate) hovered: Option<usize>,
    highlight: Option<PersonHighlight>,
}

impl PeopleState {
    /// Forgets the people found, when the content they were found in
    /// changes or another photo opens. Replacing the mask source with a new
    /// rendering of the same content keeps them.
    pub(in crate::app) fn forget(&mut self) {
        self.analysis = None;
        self.forget_views();
        self.composes.clear();
    }

    /// Drops the thumbnails and highlight made from the people found, when
    /// those change.
    fn forget_views(&mut self) {
        self.chips = None;
        self.highlight = None;
        self.hovered = None;
    }

    fn cancel_compose(&mut self, location: (usize, usize)) {
        self.composes.retain(|compose| !compose.is_for(location));
    }
}

impl CalibRawApp {
    /// Masks made from now on are of `quality`; existing ones keep theirs.
    pub(crate) fn set_people_mask_quality(
        &mut self,
        quality: calibraw_ai::models::people::PeopleMaskQuality,
    ) {
        if self.ai.people_mask_quality == quality {
            return;
        }
        self.ai.people_mask_quality = quality;
        self.persist_performance_settings();
    }

    /// Makes the masks of these people components: from the analysis for
    /// those whose person is segmented, through the people job, which finds
    /// and segments people, for the others.
    pub(crate) fn request_people_masks(
        &mut self,
        components: Vec<(usize, usize)>,
        frame: &eframe::Frame,
    ) {
        self.ui.error_dialogs.dismiss_kind(ErrorKind::AiMask);
        let mut needs_job = Vec::new();
        let mut parts_unavailable = false;
        for location in components {
            let Some(choice) = self.person_choice(location) else {
                continue;
            };
            if self.segmented_people(&choice.anchors).is_some() {
                self.start_people_compose(location, choice.clone(), choice, frame);
            } else if self.people_segmentation() == PeopleSegmentation::Outline
                && choice.parts != BTreeSet::from([PersonPart::EntirePerson])
            {
                // Outlining the people would not make their parts.
                parts_unavailable = true;
            } else {
                needs_job.push(location);
            }
        }
        if parts_unavailable {
            self.report_ai_mask_error(tr_fmt(
                "The people mask could not be made: {PartsUnavailable}. It keeps its last result.",
                &[(
                    "PartsUnavailable",
                    &calibraw_ai::models::people::PartsUnavailable,
                )],
            ));
        }
        if needs_job.is_empty() || !self.people_job_may_start() {
            return;
        }
        if let Err(error) = self.capture_mask_source(frame) {
            self.report_ai_mask_error(error);
            return;
        }
        self.ai.people.pending = needs_job;
        self.request_content_job(AiFeature::People);
    }

    /// Whether a people job may start now; reports why not otherwise.
    fn people_job_may_start(&mut self) -> bool {
        if self.foreground_operation_active() {
            self.ui.notice =
                Some(tr("Finish or cancel the current editing operation first.").to_owned());
            return false;
        }
        true
    }

    /// Changes a people component's people or parts. The component takes
    /// the change together with its new mask, as one edit. A person not
    /// segmented yet is chosen at once, and the people job segments them.
    pub(crate) fn edit_people_mask(
        &mut self,
        mask_index: usize,
        component_index: usize,
        edit: PeopleEdit,
        frame: &eframe::Frame,
    ) {
        let location = (mask_index, component_index);
        let (Some(current), Some(shown)) = (
            self.person_choice(location),
            self.shown_person_choice(mask_index, component_index),
        ) else {
            return;
        };
        let Some(next) = shown.edited(edit, self.ai.people.analysis.as_ref()) else {
            return;
        };
        if next == current {
            // Back to the component's own choice: its mask still fits.
            self.ai.people.cancel_compose(location);
            return;
        }
        if self.segmented_people(&next.anchors).is_some() {
            self.start_people_compose(location, current, next, frame);
            return;
        }
        if !self.people_job_may_start() {
            return;
        }
        self.ai.people.cancel_compose(location);
        if let Some(MaskGeometry::Person { anchors, parts, .. }) =
            self.component_geometry_mut(location)
        {
            *anchors = next.anchors;
            *parts = next.parts;
        }
        self.mark_mask_geometry_dirty(mask_index);
        self.request_people_masks(vec![location], frame);
    }

    /// Adds the person under `point` (fractions of the photo) to the people
    /// component's people, or removes them, for a click on the canvas.
    pub(crate) fn pick_person(
        &mut self,
        mask_index: usize,
        component_index: usize,
        point: [f32; 2],
        frame: &eframe::Frame,
    ) {
        let Some(person) = self
            .ai
            .people
            .analysis
            .as_ref()
            .and_then(|analysis| analysis.person_under(point))
        else {
            return;
        };
        self.edit_people_mask(
            mask_index,
            component_index,
            PeopleEdit::TogglePerson(person),
            frame,
        );
    }

    /// `person`'s highlight over the canvas in `color`.
    pub(crate) fn person_highlight(
        &mut self,
        ctx: &egui::Context,
        person: usize,
        color: egui::Color32,
    ) -> Option<&PersonHighlight> {
        let current = self
            .ai
            .people
            .highlight
            .as_ref()
            .is_some_and(|highlight| highlight.person == person && highlight.color == color);
        if !current {
            let silhouette = self.ai.people.analysis.as_ref()?.silhouette(person)?;
            let rgba: Vec<u8> = silhouette
                .coverage
                .iter()
                .flat_map(|&coverage| {
                    let alpha = u16::from(coverage) * HIGHLIGHT_ALPHA / 255;
                    [color.r(), color.g(), color.b(), alpha as u8]
                })
                .collect();
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [silhouette.width as usize, silhouette.height as usize],
                &rgba,
            );
            self.ai.people.highlight = Some(PersonHighlight {
                person,
                color,
                texture: ctx.load_texture("person-highlight", image, egui::TextureOptions::LINEAR),
                region: silhouette.region,
            });
        }
        self.ai.people.highlight.as_ref()
    }

    /// What a people component shows: the choice its mask is being made
    /// for, else its own.
    pub(crate) fn shown_person_choice(
        &self,
        mask_index: usize,
        component_index: usize,
    ) -> Option<PersonChoice> {
        let location = (mask_index, component_index);
        self.ai
            .people
            .composes
            .iter()
            .find(|compose| compose.is_for(location))
            .map(|compose| compose.target.clone())
            .or_else(|| self.person_choice(location))
    }

    fn person_choice(&self, location: (usize, usize)) -> Option<PersonChoice> {
        PersonChoice::of(self.component_geometry(location)?)
    }

    fn component_geometry(
        &self,
        (mask_index, component_index): (usize, usize),
    ) -> Option<&MaskGeometry> {
        let mask = self.masks.stack.masks.get(mask_index)?;
        Some(&mask.components.get(component_index)?.geometry)
    }

    fn component_geometry_mut(
        &mut self,
        (mask_index, component_index): (usize, usize),
    ) -> Option<&mut MaskGeometry> {
        let mask = self.masks.stack.masks.get_mut(mask_index)?;
        Some(&mut mask.components.get_mut(component_index)?.geometry)
    }

    /// The people `anchors` select, while the analysis has all their masks
    /// need at the people-mask quality.
    fn segmented_people(&self, anchors: &[[f32; 2]]) -> Option<Vec<usize>> {
        let analysis = self.ai.people.analysis.as_ref()?;
        let segmentation = self.people_segmentation();
        let people = analysis.resolve(anchors);
        (!people.is_empty()
            && people
                .iter()
                .all(|&person| analysis.is_segmented(person, segmentation)))
        .then_some(people)
    }

    /// Starts making the mask of `target`, whose people are segmented, for
    /// the component at `location`, replacing one already being made.
    fn start_people_compose(
        &mut self,
        location: (usize, usize),
        base: PersonChoice,
        target: PersonChoice,
        frame: &eframe::Frame,
    ) {
        self.ai.people.cancel_compose(location);
        let (Some(people), Some(analysis)) = (
            self.segmented_people(&target.anchors),
            self.ai.people.analysis.clone(),
        ) else {
            return;
        };
        if let Err(error) = self.capture_mask_source(frame) {
            self.report_ai_mask_error(error);
            return;
        }
        let Some(source) = self.masks.source_cache.as_ref().map(PeopleSource::from) else {
            return;
        };
        let parts = target.parts.clone();
        let segmentation = self.people_segmentation();
        let receiver = spawn_ui_worker(&self.egui_ctx, move || {
            analysis
                .person_mask(&source, &people, &parts, segmentation)
                .map_err(|error| format!("{error:#}"))
        });
        self.ai.people.composes.push(PeopleCompose {
            location,
            base,
            target,
            receiver,
        });
    }

    /// Gives each component whose people mask is ready its mask and choice.
    pub(in crate::app) fn poll_people_composes(&mut self) {
        let mut index = 0;
        while index < self.ai.people.composes.len() {
            let result = match self.ai.people.composes[index].receiver.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => {
                    index += 1;
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    Err(tr("its worker stopped unexpectedly").to_owned())
                }
            };
            let compose = self.ai.people.composes.swap_remove(index);
            self.apply_people_compose(compose, result);
        }
    }

    fn apply_people_compose(
        &mut self,
        compose: PeopleCompose,
        result: Result<Option<PersonMask>, String>,
    ) {
        let mut mask = match result {
            Ok(Some(mask)) => mask,
            // Only an analysis without the person returns none, and the
            // compose started from one with them.
            Ok(None) => return,
            Err(error) => {
                self.report_ai_mask_error(tr_fmt(
                    "The people mask could not be made: {error}",
                    &[("error", &error)],
                ));
                return;
            }
        };
        let location = compose.location;
        if self.person_choice(location).as_ref() != Some(&compose.base) {
            return;
        }
        let people_anchors = std::mem::take(&mut mask.anchors);
        let Some(image) = mask.into_probability_mask() else {
            self.report_ai_mask_error(
                tr("People selection returned an invalid mask image.").to_owned(),
            );
            return;
        };
        if let Some(MaskGeometry::Person {
            anchors,
            parts,
            mask,
            ..
        }) = self.component_geometry_mut(location)
        {
            *anchors = people_anchors;
            *parts = compose.target.parts;
            *mask = Some(image);
        }
        self.mark_mask_geometry_dirty(location.0);
        self.blink_selected_component();
    }

    /// Every people component of the stack, for an update.
    pub(in crate::app) fn queue_all_people_masks(&mut self) {
        self.ai.people.pending = self
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

    /// Where the people analysis of the photo `target` names is kept, next
    /// to it. `None` when the photo has no writable location.
    pub(crate) fn people_analysis_path_for_target(
        &self,
        target: &crate::sidecar::SidecarTarget,
    ) -> Option<PathBuf> {
        match target {
            crate::sidecar::SidecarTarget::Desktop { raw_path } => self
                .caps()
                .has_folders()
                .then(|| crate::sidecar::people_analysis_path_for_raw(raw_path)),
            crate::sidecar::SidecarTarget::Android {
                raw_uri,
                display_name,
            } => self
                .platform
                .people_analysis_path(raw_uri, display_name)
                .map_err(|error| log::warn!("{error}"))
                .ok(),
        }
    }

    /// Where a people job keeps its analysis, and what it is an analysis of.
    fn people_analysis_file(&self) -> Option<PeopleAnalysisFile> {
        let path = self
            .persistence
            .sidecar_target
            .as_ref()
            .and_then(|target| self.people_analysis_path_for_target(target))?;
        Some(PeopleAnalysisFile {
            path,
            raw: Arc::clone(self.develop.loaded_raw.as_ref()?),
            remove: Arc::clone(&self.inpaint.edits),
        })
    }

    pub(in crate::app) fn start_people_worker(&mut self, allow_download: bool) {
        let components = std::mem::take(&mut self.ai.people.pending);
        if self.foreground_operation_active() || components.is_empty() {
            return;
        }
        let Some(source) = self.masks.source_cache.as_ref().map(PeopleSource::from) else {
            self.report_error(
                ErrorKind::AiMask,
                tr("The preview could not be prepared for people selection.").to_owned(),
            );
            return;
        };
        let (targets, captured): (Vec<_>, Vec<_>) = components
            .into_iter()
            .filter_map(|(mask_index, component_index)| {
                let target = self.masks.capture_ai_target(mask_index, component_index)?;
                let MaskGeometry::Person { anchors, parts, .. } = &target.geometry else {
                    return None;
                };
                let people_target = PeopleTarget {
                    anchors: anchors.clone(),
                    parts: parts.clone(),
                };
                Some((people_target, target))
            })
            .unzip();
        if captured.is_empty() {
            self.report_error(
                ErrorKind::AiMask,
                tr("The people mask is no longer available.").to_owned(),
            );
            return;
        }
        let analysis = self.ai.people.analysis.clone();
        let models = self.people_models();
        let segmentation = models.segmentation();
        let segments = analysis.as_ref().is_none_or(|analysis| {
            targets.iter().any(|target| {
                analysis
                    .resolve(&target.anchors)
                    .iter()
                    .any(|&person| !analysis.is_segmented(person, segmentation))
            })
        });
        let phase = if !models.are_verified() {
            tr("Preparing people models…")
        } else if analysis.is_none() {
            tr("Finding people…")
        } else if segments && matches!(models, PeopleModels::Outline { .. }) {
            tr("Outlining people…")
        } else if segments {
            tr("Segmenting body parts…")
        } else {
            tr("Updating the people mask…")
        };
        let cancellation = Arc::new(AtomicBool::new(false));
        let receiver = spawn_people_mask(PeopleMaskWorkerRequest {
            models,
            allow_download,
            request: PeopleMaskRequest {
                source,
                analysis,
                targets,
                keep: self.people_analysis_file(),
            },
            cancellation: Arc::clone(&cancellation),
        });
        self.begin_foreground_operation(ForegroundOperation {
            kind: PEOPLE_JOB,
            document_id: self.persistence.document_generation,
            cancellation,
            progress: ForegroundProgress::indeterminate(phase),
            cancelling: false,
            receiver: ForegroundOperationReceiver::People(receiver),
            context: ForegroundOperationContext::People {
                targets: captured,
                error: None,
                not_kept: None,
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
        let mut updates = Vec::new();
        let finished = poll_ai_job(
            self.platform.as_ref(),
            receiver,
            || tr("The people-mask worker stopped unexpectedly.").to_owned(),
            |update| updates.push(update),
        );
        let accepts = operation.accepts_result(self.persistence.document_generation);
        for update in updates {
            match update {
                AiJobUpdate::Download(progress) => {
                    operation.progress = model_download_progress(progress);
                }
                AiJobUpdate::Progress(progress) => {
                    self.apply_people_progress(&mut operation, progress, accepts);
                }
            }
        }
        let Some(result) = finished else {
            self.foreground_operation = Some(operation);
            return;
        };
        let cancelled = operation.is_cancelled();
        let stale = !operation.is_for_document(self.persistence.document_generation);
        let ForegroundOperationContext::People {
            error, not_kept, ..
        } = operation.context
        else {
            return;
        };

        let mut error_message = None;
        if !cancelled && !stale {
            match result {
                Ok(analysis) => {
                    self.ai.people.analysis = Some(analysis);
                    error_message = error;
                }
                Err(error) => {
                    error_message = Some(tr_fmt(
                        "People selection failed: {error}",
                        &[("error", &error)],
                    ))
                }
            }
        }
        if let Some(reason) = not_kept.filter(|_| !stale) {
            self.report_error(
                ErrorKind::PeopleNotSaved,
                tr_fmt(
                    "The people in this photo could not be saved next to it ({reason}). They \
                     will be found again the next time the photo is opened.",
                    &[("reason", &reason)],
                ),
            );
        }
        let succeeded = !cancelled && !stale && error_message.is_none();
        if self.ai.update.is_some() {
            self.advance_ai_update(succeeded, cancelled || stale, error_message);
        } else if let Some(message) = error_message {
            self.report_error(ErrorKind::AiMask, message);
        } else if stale && !cancelled {
            self.report_error(
                ErrorKind::AiMask,
                tr("People selection became stale before inference completed.").to_owned(),
            );
        }
        self.egui_ctx.request_repaint();
    }

    /// Takes in what the people job reported. Its results count only while
    /// the job still `accepts` them: a cancelled job's later ones may be of
    /// content that changed.
    fn apply_people_progress(
        &mut self,
        operation: &mut ForegroundOperation,
        progress: PeopleMaskProgress,
        accepts: bool,
    ) {
        match progress {
            PeopleMaskProgress::FindingPeople => {
                operation.progress = ForegroundProgress::indeterminate(tr("Finding people…"));
            }
            PeopleMaskProgress::OutliningPeople { done, total } => {
                operation.progress = ForegroundProgress::units(
                    done as u64,
                    total as u64,
                    None::<String>,
                    tr("Outlining people…"),
                );
            }
            PeopleMaskProgress::SegmentingBodyParts { done, total } => {
                operation.progress = ForegroundProgress::units(
                    done as u64,
                    total as u64,
                    None::<String>,
                    tr("Segmenting body parts…"),
                );
            }
            PeopleMaskProgress::Found(analysis) if accepts => {
                self.ai.people.analysis = Some(analysis);
                self.ai.people.forget_views();
            }
            PeopleMaskProgress::Segmented(person) if accepts => {
                if let Some(analysis) = self.ai.people.analysis.as_mut() {
                    analysis.insert(person);
                }
                // Their thumbnail now centres on their face, and their
                // highlight follows their body.
                self.ai.people.forget_views();
            }
            PeopleMaskProgress::Masks(masks) if accepts => {
                if let ForegroundOperationContext::People { targets, error, .. } =
                    &mut operation.context
                {
                    *error = self.apply_people_masks(targets, masks);
                }
            }
            PeopleMaskProgress::NotKept(reason) => {
                if let ForegroundOperationContext::People { not_kept, .. } = &mut operation.context
                {
                    *not_kept = Some(reason);
                }
            }
            PeopleMaskProgress::Found(_)
            | PeopleMaskProgress::Segmented(_)
            | PeopleMaskProgress::Masks(_) => {}
        }
    }

    /// Writes each mask and its people's anchors to its component. Returns
    /// the error to report; finding no one for a mask that has not chosen a
    /// person yet is not one.
    fn apply_people_masks(
        &mut self,
        targets: &[AiMaskTarget],
        masks: Vec<Option<PersonMask>>,
    ) -> Option<String> {
        let mut error = None;
        let mut found_no_one = false;
        let mut changed_layers = BTreeSet::new();
        for (target, mask) in targets.iter().zip(masks) {
            let Some(mask) = mask else {
                // A refreshed mask keeps its last result; a new one stays empty.
                if matches!(
                    &target.geometry,
                    MaskGeometry::Person { anchors, .. } if !anchors.is_empty()
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
            let mut mask = mask;
            let anchors = std::mem::take(&mut mask.anchors);
            let Some(image) = mask.into_probability_mask() else {
                error = Some(tr("People selection returned an invalid mask image.").to_owned());
                continue;
            };
            if let Some(MaskGeometry::Person {
                anchors: target_anchors,
                mask: target_mask,
                ..
            }) = self.component_geometry_mut(location)
            {
                *target_anchors = anchors;
                *target_mask = Some(image);
                changed_layers.insert(location.0);
            }
        }
        if found_no_one && self.ai.update.is_none() {
            self.report_error(
                ErrorKind::NoPeopleFound,
                tr("No people were found in this photo.").to_owned(),
            );
        }
        if !changed_layers.is_empty() {
            for mask_index in changed_layers {
                self.mark_mask_geometry_dirty(mask_index);
            }
            self.blink_selected_component();
        }
        error
    }

    /// The people and parts a people component shows, and the people found
    /// in the photo, for its properties; empty for another component. Makes
    /// the people's chips first.
    pub(crate) fn people_choices(
        &mut self,
        mask_index: usize,
        component_index: usize,
        frame: &eframe::Frame,
    ) -> PeopleChoices {
        let Some(choice) = self.shown_person_choice(mask_index, component_index) else {
            return PeopleChoices::default();
        };
        self.ensure_people_chips(frame);
        let segmentation = self.people_segmentation();
        let Some(analysis) = self.ai.people.analysis.as_ref() else {
            return PeopleChoices {
                parts: choice.parts,
                shown: segmentation.unsegmented_parts(),
                ..PeopleChoices::default()
            };
        };
        let people = self
            .ai
            .people
            .chips
            .iter()
            .flatten()
            .take(analysis.people().len())
            .enumerate()
            .map(|(index, chip)| PersonThumbnail {
                texture: chip.id(),
                segmented: analysis.is_segmented(index, segmentation),
            })
            .collect();
        let selected = analysis.resolve(&choice.anchors);
        PeopleChoices {
            analysed: true,
            people,
            shown: analysis.parts_shown(&selected, segmentation),
            selected,
            parts: choice.parts,
        }
    }

    /// Makes the chips of the people found, from the mask source. A photo
    /// opened with its analysis has none until its people mask is shown.
    fn ensure_people_chips(&mut self, frame: &eframe::Frame) {
        let Some(analysis) = self.ai.people.analysis.as_ref() else {
            return;
        };
        if self.ai.people.chips.is_some() {
            return;
        }
        if !analysis.people().is_empty() {
            if let Err(error) = self.capture_mask_source(frame) {
                // Reported once; the panel offers to find people instead.
                self.ai.people.chips = Some(Vec::new());
                self.report_ai_mask_error(error);
                return;
            }
        }
        let chips = match (&self.masks.source_cache, &self.ai.people.analysis) {
            (Some(source), Some(analysis)) => person_chips(&self.egui_ctx, source, analysis),
            _ => Vec::new(),
        };
        self.ai.people.chips = Some(chips);
    }

    /// Keeps the first part of a people component and moves each other part
    /// to a mask group of its own for the same people, so each part takes its
    /// own adjustments, then makes their masks.
    pub(crate) fn separate_person_parts(
        &mut self,
        mask_index: usize,
        component_index: usize,
        frame: &eframe::Frame,
    ) {
        let Some(PersonChoice { anchors, parts }) =
            self.shown_person_choice(mask_index, component_index)
        else {
            return;
        };
        let Some(&MaskGeometry::Person { grow, feather, .. }) =
            self.component_geometry((mask_index, component_index))
        else {
            return;
        };
        let mut parts = parts.into_iter();
        let Some(first) = parts.next() else {
            return;
        };
        let others: Vec<PersonPart> = parts.collect();
        if others.is_empty() {
            return;
        }

        let mut candidate = self.masks.stack.clone();
        if let Some(MaskGeometry::Person {
            anchors: kept_anchors,
            parts: kept_parts,
            ..
        }) = candidate
            .masks
            .get_mut(mask_index)
            .and_then(|mask| mask.components.get_mut(component_index))
            .map(|component| &mut component.geometry)
        {
            *kept_anchors = anchors.clone();
            *kept_parts = BTreeSet::from([first]);
        }
        let mut components = vec![(mask_index, component_index)];
        for part in others {
            let Some((new_mask, new_component)) = candidate.add_mask(MaskKind::Person) else {
                self.report_error(
                    ErrorKind::EditNotApplied,
                    tr_fmt("The parts were not separated: a photo holds at most {MAX_LOCAL_MASKS} mask groups.", &[("MAX_LOCAL_MASKS", &MAX_LOCAL_MASKS)]),
                );
                return;
            };
            let group = &mut candidate.masks[new_mask];
            group.common.rename(part.label());
            group.components[new_component].geometry = MaskGeometry::Person {
                anchors: anchors.clone(),
                parts: BTreeSet::from([part]),
                mask: None,
                grow,
                feather,
            };
            components.push((new_mask, new_component));
        }
        candidate.select_component(mask_index, component_index);
        if let Err(error) = crate::sidecar::preflight_mask_change(&candidate) {
            self.report_mask_persistence_limit(tr("Separating the parts"), &error);
            return;
        }
        self.masks.stack = candidate;
        self.ai.people.cancel_compose((mask_index, component_index));
        self.mark_all_mask_layers_dirty();
        self.sync_selected_mask_tool();
        self.request_people_masks(components, frame);
    }
}

/// A square thumbnail of each person's head, cut from the mask source.
fn person_chips(
    ctx: &egui::Context,
    source: &MaskRgbImage,
    analysis: &PeopleAnalysis,
) -> Vec<egui::TextureHandle> {
    let Some(image) =
        image::ImageBuffer::<Rgba<u8>, &[u8]>::from_raw(source.width, source.height, &source.rgba)
    else {
        return Vec::new();
    };
    analysis
        .people()
        .iter()
        .enumerate()
        .map(|(index, person)| {
            let portrait = analysis.portrait(index).unwrap_or(person.bounds);
            let bounds = PersonBounds::from_fractions(portrait, source.width, source.height);
            let crop =
                image::imageops::crop_imm(&image, bounds.x, bounds.y, bounds.width, bounds.height);
            let mut thumbnail = image::imageops::thumbnail(&*crop, CHIP_EDGE, CHIP_EDGE);
            stretch_levels(&mut thumbnail);
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

/// Stretches a thumbnail's luminance from its 1st to its 99th percentile
/// over the full range and lifts dark midtones: the mask source is a flat
/// scene-referred rendering, in which faces are hard to tell apart.
fn stretch_levels(thumbnail: &mut RgbaImage) {
    let luminance = |pixel: &Rgba<u8>| {
        let [red, green, blue, _] = pixel.0.map(f32::from);
        0.2126 * red + 0.7152 * green + 0.0722 * blue
    };
    let mut histogram = [0usize; 256];
    for pixel in thumbnail.pixels() {
        histogram[luminance(pixel).round() as usize] += 1;
    }
    let total: usize = histogram.iter().sum();
    let percentile = |fraction: f32| {
        let target = (total as f32 * fraction) as usize;
        let mut below = 0;
        for (level, count) in histogram.iter().enumerate() {
            below += count;
            if below > target {
                return level as f32;
            }
        }
        255.0
    };
    let (low, high) = (percentile(0.01), percentile(0.99));
    if high - low < 8.0 {
        return;
    }
    // Brings a dark median towards 0.45; never darkens.
    let median = ((percentile(0.5) - low) / (high - low)).clamp(0.01, 0.99);
    let gamma = (0.45f32.ln() / median.ln()).clamp(0.5, 1.0);
    for pixel in thumbnail.pixels_mut() {
        for channel in &mut pixel.0[..3] {
            let value = ((f32::from(*channel) - low) / (high - low)).clamp(0.0, 1.0);
            *channel = (value.powf(gamma) * 255.0).round() as u8;
        }
    }
}
