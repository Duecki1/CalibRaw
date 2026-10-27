//! People masks: RF-DETR-Seg finds the people in a photo and outlines them
//! coarsely; Sapiens2 then segments the body parts of each person, on a crop
//! around them. For high-quality masks BiRefNet first finds the foreground
//! of the same crops, whose finer outline replaces Sapiens2's near its edge
//! (`foreground`). The models run once per photo, for everyone: the masks of
//! any person and parts are then made from that analysis without a model
//! (`PeopleAnalysis::person_mask`), and the analysis is kept beside the photo
//! (`store`) for the next time it opens. The coarse outlines only decide
//! whose a pixel is; every edge comes from Sapiens2 or BiRefNet.

use crate::config::CAPABILITIES;
use crate::jobs::{spawn_job, AiJobEvent, JobContext, JobKind};
use crate::models::masks::mask_model_retention;
use crate::models::subject::{SubjectMaskQuality, SubjectModelPrecision};
use crate::pipeline::{LoadedRaw, MaskImage, PersonPart, RemoveEditState};
use crate::registry::{AiModel, ModelInfo};
use crate::runtime::initialize_runtime;
use crate::slot::with_model_session;
use anyhow::{Context, Result};
use image::{ImageBuffer, Rgba};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fmt,
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, mpsc, Arc},
};

mod detect;
mod distance;
mod foreground;
mod parts;
mod store;
use detect::Detection;
use foreground::PersonForeground;
use parts::PersonParts;
pub use store::{content_fingerprint, ContentFingerprint};

pub const DETECTOR_MODEL: ModelInfo = AiModel::PersonDetector.spec().info();
pub const BODY_PARTS_MODEL: ModelInfo = AiModel::BodyParts.spec().info();
/// Whether this device offers people masks.
pub const AVAILABLE: bool = CAPABILITIES.people_masks;
/// Where the body-part model's license is published; its download consent
/// names and links it.
pub const BODY_PARTS_LICENSE: &str =
    "https://github.com/facebookresearch/sapiens2/blob/main/LICENSE.md";

const MAX_SOURCE_PIXELS: u64 = 17_000_000;
const MAX_TARGETS: usize = 64;
/// A portrait shows this much more than the face and hair, for the neck,
/// shoulders and some room around them.
const PORTRAIT_MARGIN: f32 = 1.4;
/// Longer edge of the silhouette of someone not segmented yet, in pixels.
const OUTLINE_SILHOUETTE_EDGE: f32 = 128.0;

const PEOPLE_JOB: JobKind = JobKind {
    thread: "calibraw-onnx-people",
    worker: "people-mask worker",
    panicked: "ONNX Runtime terminated people-mask inference",
    cancelled: "background task cancelled",
};

/// The photo as the models and the masks read it.
type RgbaView<'a> = ImageBuffer<Rgba<u8>, &'a [u8]>;

/// How much work a people mask may take. Serialized in the performance
/// settings; keep the names stable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeopleMaskQuality {
    /// Sapiens2's outline of each person.
    #[default]
    Medium,
    /// BiRefNet's finer outline of each person, from one more model run per
    /// person and the `foreground_model` download.
    High,
}

impl PeopleMaskQuality {
    pub const ALL: [Self; 2] = [Self::Medium, Self::High];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }
}

/// The BiRefNet model of high-quality people masks: the subject model with
/// the 1024 x 1024 input, one file with it.
pub const fn foreground_model(precision: SubjectModelPrecision) -> ModelInfo {
    SubjectMaskQuality::High.model(precision)
}

/// Where the BiRefNet model of high-quality people masks is, and which of
/// its files.
#[derive(Clone, Debug)]
pub struct ForegroundModel {
    pub path: PathBuf,
    pub precision: SubjectModelPrecision,
}

impl ForegroundModel {
    fn ai_model(&self) -> AiModel {
        SubjectMaskQuality::High.ai_model(self.precision)
    }
}

/// Whether the models of people masks are installed; BiRefNet's only with
/// `foreground`, for high-quality masks.
pub fn people_models_are_verified(
    detector: &Path,
    body_parts: &Path,
    foreground: Option<&ForegroundModel>,
) -> bool {
    AiModel::PersonDetector.spec().is_installed(detector)
        && AiModel::BodyParts.spec().is_installed(body_parts)
        && foreground.is_none_or(|model| model.ai_model().spec().is_installed(&model.path))
}

/// The photo people are found in, and their masks refined against: RGBA,
/// 8 bits per channel, by row.
#[derive(Clone)]
pub struct PeopleSource {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

impl PeopleSource {
    fn image(&self) -> Result<RgbaView<'_>> {
        let pixels = u64::from(self.width) * u64::from(self.height);
        anyhow::ensure!(
            pixels > 0 && pixels <= MAX_SOURCE_PIXELS,
            "people-mask input {}x{} exceeds the {MAX_SOURCE_PIXELS}-pixel limit",
            self.width,
            self.height
        );
        ImageBuffer::from_raw(self.width, self.height, &self.rgba[..])
            .context("invalid preview image for people masks")
    }
}

/// A rectangle of the photo, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PersonBounds {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl PersonBounds {
    /// The pixels of a `width` x `height` photo covered by `bounds` (left,
    /// top, right, bottom as fractions), at least one.
    pub fn from_fractions(bounds: [f32; 4], width: u32, height: u32) -> Self {
        let [left, top, right, bottom] = bounds;
        let x = ((left * width as f32).floor() as u32).min(width.saturating_sub(1));
        let y = ((top * height as f32).floor() as u32).min(height.saturating_sub(1));
        Self {
            x,
            y,
            width: ((right * width as f32).ceil() as u32).clamp(x + 1, width) - x,
            height: ((bottom * height as f32).ceil() as u32).clamp(y + 1, height) - y,
        }
    }
}

/// A person found in the photo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetectedPerson {
    /// Their box: left, top, right and bottom as fractions of the photo.
    pub bounds: [f32; 4],
    /// The centre of their outline, as fractions of the photo: what a people
    /// mask stores to find them again.
    pub anchor: [f32; 2],
}

/// The people found in one photo, and the body parts and foreground of
/// those segmented so far. Positions are fractions of the photo, so the
/// analysis serves every rendering of the same content, whatever its size.
#[derive(Clone)]
pub struct PeopleAnalysis {
    detection: Arc<Detection>,
    people: Arc<[DetectedPerson]>,
    /// Width over height of the photo analysed.
    aspect: f32,
    /// By person, those segmented so far.
    parts: Vec<Option<Arc<PersonParts>>>,
    /// By person, those whose foreground BiRefNet found so far. A person
    /// with both has them over the same crop.
    foregrounds: Vec<Option<Arc<PersonForeground>>>,
}

impl fmt::Debug for PeopleAnalysis {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PeopleAnalysis")
            .field("aspect", &self.aspect)
            .field("people", &self.people)
            .finish_non_exhaustive()
    }
}

impl PeopleAnalysis {
    fn new(detection: Detection, aspect: f32) -> Self {
        let people: Arc<[DetectedPerson]> = detection.people().into();
        Self {
            aspect,
            parts: vec![None; people.len()],
            foregrounds: vec![None; people.len()],
            detection: Arc::new(detection),
            people,
        }
    }

    /// Left to right.
    pub fn people(&self) -> &[DetectedPerson] {
        &self.people
    }

    /// The people a people mask anchored at `anchors` selects, in order and
    /// each once. An anchor selects the person whose outline contains it,
    /// else the one whose anchor is nearest; a mask that has not chosen
    /// anyone yet selects the most prominent person. Empty only when no one
    /// was found.
    pub fn resolve(&self, anchors: &[[f32; 2]]) -> Vec<usize> {
        let mut people: Vec<usize> = if anchors.is_empty() {
            self.detection.most_prominent().into_iter().collect()
        } else {
            anchors
                .iter()
                .filter_map(|&anchor| self.detection.person_at(anchor, self.aspect))
                .collect()
        };
        people.sort_unstable();
        people.dedup();
        people
    }

    /// Whether what `person`'s masks of `quality` are made from is known, so
    /// they need no model.
    pub fn is_segmented(&self, person: usize, quality: PeopleMaskQuality) -> bool {
        let parts = self.parts.get(person).is_some_and(Option::is_some);
        let foreground = self.foregrounds.get(person).is_some_and(Option::is_some);
        parts && (quality == PeopleMaskQuality::Medium || foreground)
    }

    /// Adds what a job found of a person for this analysis. False, changing
    /// nothing, for one of another analysis.
    pub fn insert(&mut self, segmented: SegmentedPerson) -> bool {
        if !Arc::ptr_eq(&self.detection, &segmented.detection)
            || segmented.person >= self.people.len()
        {
            return false;
        }
        match segmented.found {
            PersonFound::Parts(parts) => self.parts[segmented.person] = Some(parts),
            PersonFound::Foreground(foreground) => {
                self.foregrounds[segmented.person] = Some(foreground);
            }
        }
        true
    }

    /// The crop `person`'s models run on, as fractions of the photo: the one
    /// a model already ran on, else the first for their box on a `width` x
    /// `height` photo.
    fn crop(&self, person: usize, width: u32, height: u32) -> [f32; 4] {
        self.parts[person]
            .as_ref()
            .map(|parts| parts.crop)
            .or_else(|| self.foregrounds[person].as_ref().map(|found| found.crop))
            .unwrap_or_else(|| parts::person_crop(self.detection.bounds(person), width, height))
    }

    /// The mask of `parts` of `people` over `source` at `quality`, its edges
    /// refined against it; runs no model. `None` for no one, or until
    /// everyone is segmented for `quality`.
    pub fn person_mask(
        &self,
        source: &PeopleSource,
        people: &[usize],
        parts: &BTreeSet<PersonPart>,
        quality: PeopleMaskQuality,
    ) -> Result<Option<PersonMask>> {
        self.mask_of(&source.image()?, people, parts, quality)
    }

    /// The parts any of `people` shows; `None` until they are all
    /// segmented.
    pub fn parts_shown(&self, people: &[usize]) -> Option<BTreeSet<PersonPart>> {
        let segmented = self.segmented(people)?;
        Some(
            PersonPart::ALL
                .into_iter()
                .filter(|&part| segmented.iter().any(|person| person.shows(part)))
                .collect(),
        )
    }

    /// The body parts of each of `people`; `None` unless they are all
    /// segmented.
    fn segmented(&self, people: &[usize]) -> Option<Vec<&PersonParts>> {
        people
            .iter()
            .map(|&person| self.parts.get(person)?.as_deref())
            .collect()
    }

    /// The person at `point` (fractions of the photo): the likeliest whose
    /// body parts, or outline while not segmented, cover it. `None` over no
    /// one.
    pub fn person_under(&self, point: [f32; 2]) -> Option<usize> {
        (0..self.people.len())
            .map(|person| {
                let probability = match &self.parts[person] {
                    Some(parts) => parts.coverage_at(point),
                    None => self
                        .detection
                        .outline_probability(person, point[0], point[1]),
                };
                (person, probability)
            })
            .filter(|&(_, probability)| probability >= 0.5)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(person, _)| person)
    }

    /// A square of the photo around `person`'s head, for their thumbnail:
    /// left, top, right and bottom as fractions. Around their face and hair
    /// once segmented, else the top of their box.
    pub fn portrait(&self, person: usize) -> Option<[f32; 4]> {
        let [left, top, right, bottom] = self.people.get(person)?.bounds;
        let aspect = self.aspect;
        // Sides in photo heights, so the square is square in pixels.
        let (centre, side) = match self.parts[person].as_ref().and_then(|parts| parts.head()) {
            Some([head_left, head_top, head_right, head_bottom]) => (
                [
                    (head_left + head_right) * 0.5,
                    (head_top + head_bottom) * 0.5,
                ],
                ((head_right - head_left) * aspect).max(head_bottom - head_top) * PORTRAIT_MARGIN,
            ),
            None => {
                let side = ((right - left) * aspect).min(bottom - top);
                ([(left + right) * 0.5, top + side * 0.5], side)
            }
        };
        let side = side.clamp(f32::EPSILON, aspect.min(1.0));
        let (width, height) = (side / aspect, side);
        let x = (centre[0] - width * 0.5).clamp(0.0, 1.0 - width);
        let y = (centre[1] - height * 0.5).clamp(0.0, 1.0 - height);
        Some([x, y, x + width, y + height])
    }

    /// Where `person` is, to show them over the photo: from their body
    /// parts once segmented, else their outline.
    pub fn silhouette(&self, person: usize) -> Option<PersonSilhouette> {
        let bounds = self.people.get(person)?.bounds;
        if let Some(parts) = &self.parts[person] {
            return Some(PersonSilhouette {
                region: parts.crop,
                width: parts::INPUT_WIDTH,
                height: parts::INPUT_HEIGHT,
                coverage: parts.coverage(),
            });
        }
        let [left, top, right, bottom] = bounds;
        let (span_x, span_y) = ((right - left) * self.aspect, bottom - top);
        let scale = OUTLINE_SILHOUETTE_EDGE / span_x.max(span_y).max(f32::EPSILON);
        let width = ((span_x * scale).round() as u32).max(1);
        let height = ((span_y * scale).round() as u32).max(1);
        let coverage = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .map(|(x, y)| {
                let u = left + (right - left) * (x as f32 + 0.5) / width as f32;
                let v = top + (bottom - top) * (y as f32 + 0.5) / height as f32;
                (self.detection.outline_probability(person, u, v) * 255.0).round() as u8
            })
            .collect();
        Some(PersonSilhouette {
            region: bounds,
            width,
            height,
            coverage,
        })
    }

    /// Where people overlap, the mask keeps the likelier of their masks.
    fn mask_of(
        &self,
        image: &RgbaView<'_>,
        people: &[usize],
        parts: &BTreeSet<PersonPart>,
        quality: PeopleMaskQuality,
    ) -> Result<Option<PersonMask>> {
        if !people
            .iter()
            .all(|&person| self.is_segmented(person, quality))
        {
            return Ok(None);
        }
        let mut masks = people
            .iter()
            .map(|&person| self.mask_of_person(image, person, parts, quality));
        let Some(first) = masks.next() else {
            return Ok(None);
        };
        let mut mask = first?;
        for other in masks {
            for (union, value) in mask.iter_mut().zip(other?) {
                *union = (*union).max(value);
            }
        }
        Ok(Some(PersonMask {
            anchors: people
                .iter()
                .map(|&person| self.people[person].anchor)
                .collect(),
            width: image.width(),
            height: image.height(),
            mask,
        }))
    }

    /// The mask of `parts` of one person segmented for `quality`.
    fn mask_of_person(
        &self,
        image: &RgbaView<'_>,
        person: usize,
        parts: &BTreeSet<PersonPart>,
        quality: PeopleMaskQuality,
    ) -> Result<Vec<u8>> {
        let segmented = self.parts[person]
            .as_deref()
            .context("a selected person has no body parts")?;
        if quality == PeopleMaskQuality::Medium {
            return segmented.mask(image, parts);
        }
        let found = self.foregrounds[person]
            .as_deref()
            .context("a selected person has no foreground")?;
        match foreground::refine(segmented, found) {
            Some(planes) => parts::crop_mask(segmented.crop, &planes, image, parts),
            None => segmented.mask(image, parts),
        }
    }
}

/// Where one person is in the photo.
pub struct PersonSilhouette {
    /// The part of the photo `coverage` spans: left, top, right and bottom
    /// as fractions.
    pub region: [f32; 4],
    pub width: u32,
    pub height: u32,
    /// How likely each pixel is theirs, 0 to 255, by row.
    pub coverage: Vec<u8>,
}

/// One person's body parts or foreground, as a job reports them while it
/// runs.
#[derive(Clone)]
pub struct SegmentedPerson {
    detection: Arc<Detection>,
    person: usize,
    found: PersonFound,
}

#[derive(Clone)]
enum PersonFound {
    Parts(Arc<PersonParts>),
    Foreground(Arc<PersonForeground>),
}

impl fmt::Debug for SegmentedPerson {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SegmentedPerson")
            .field("person", &self.person)
            .finish_non_exhaustive()
    }
}

/// One people mask to make.
#[derive(Clone, Debug, PartialEq)]
pub struct PeopleTarget {
    /// A point on each person, as fractions of the photo; empty selects the
    /// most prominent person.
    pub anchors: Vec<[f32; 2]>,
    pub parts: BTreeSet<PersonPart>,
}

/// Where a job keeps its analysis, and the photo content it is of: the RAW
/// as loaded, lens correction included, and the Remove edits rendered over
/// it, which the source is rendered from.
pub struct PeopleAnalysisFile {
    pub path: PathBuf,
    pub raw: Arc<LoadedRaw>,
    pub remove: Arc<RemoveEditState>,
}

pub struct PeopleMaskRequest {
    pub source: PeopleSource,
    /// The analysis of an earlier job on the same content. Without one, the
    /// job finds everyone and segments them all.
    pub analysis: Option<PeopleAnalysis>,
    pub targets: Vec<PeopleTarget>,
    pub keep: Option<PeopleAnalysisFile>,
}

#[derive(Debug)]
pub struct PersonMask {
    /// The anchors of the people found, which the target's mask stores.
    pub anchors: Vec<[f32; 2]>,
    pub width: u32,
    pub height: u32,
    pub mask: Vec<u8>,
}

impl PersonMask {
    pub fn into_probability_mask(self) -> Option<MaskImage> {
        MaskImage::new(self.width, self.height, self.mask)
    }
}

/// What a people job reports while it runs. Its results arrive here too, as
/// soon as they exist; the job ends with the analysis.
#[derive(Debug)]
pub enum PeopleMaskProgress {
    FindingPeople,
    /// Everyone the job found, none segmented yet.
    Found(PeopleAnalysis),
    /// BiRefNet found the foreground of `done` of `total` people.
    OutliningPeople {
        done: usize,
        total: usize,
    },
    /// `done` of `total` people are segmented.
    SegmentingBodyParts {
        done: usize,
        total: usize,
    },
    Segmented(SegmentedPerson),
    /// The targets' masks, in order; `None` where no one was found. Sent once
    /// the targets' people are segmented, before anyone else is.
    Masks(Vec<Option<PersonMask>>),
    /// The analysis could not be kept beside the photo, for this reason.
    NotKept(String),
}

pub type PeopleMaskEvent = AiJobEvent<PeopleAnalysis, PeopleMaskProgress>;

pub struct PeopleMaskWorkerRequest {
    pub detector_path: PathBuf,
    pub body_parts_path: PathBuf,
    /// BiRefNet, for high-quality masks; `None` makes medium-quality ones.
    pub foreground: Option<ForegroundModel>,
    pub allow_download: bool,
    pub request: PeopleMaskRequest,
    pub cancellation: Arc<AtomicBool>,
}

pub fn spawn_people_mask(worker: PeopleMaskWorkerRequest) -> mpsc::Receiver<PeopleMaskEvent> {
    let PeopleMaskWorkerRequest {
        detector_path,
        body_parts_path,
        foreground,
        allow_download,
        request,
        cancellation,
    } = worker;
    spawn_job(&PEOPLE_JOB, cancellation, move |job| {
        job.install(
            AiModel::PersonDetector.spec(),
            &detector_path,
            allow_download,
        )?;
        job.install(AiModel::BodyParts.spec(), &body_parts_path, allow_download)?;
        if let Some(model) = &foreground {
            job.install(model.ai_model().spec(), &model.path, allow_download)?;
        }
        job.ensure_not_cancelled()?;
        infer_people_masks(
            job,
            &detector_path,
            &body_parts_path,
            foreground.as_ref(),
            request,
        )
    })
}

/// Finds the people unless `request` brings them, then segments the
/// targets' people and everyone else the job found, the targets first, and
/// sends the targets' masks as soon as their people are segmented. With
/// `foreground`, BiRefNet finds their foregrounds before Sapiens2 segments
/// them: one model is loaded at a time, so each runs for everyone at once.
/// A cancelled job keeps and reports what it found before it stopped.
fn infer_people_masks(
    job: &JobContext<PeopleAnalysis, PeopleMaskProgress>,
    detector_path: &Path,
    body_parts_path: &Path,
    foreground: Option<&ForegroundModel>,
    request: PeopleMaskRequest,
) -> Result<PeopleAnalysis> {
    let PeopleMaskRequest {
        source,
        analysis,
        targets,
        keep,
    } = request;
    anyhow::ensure!(
        targets.len() <= MAX_TARGETS,
        "a people-mask job takes at most {MAX_TARGETS} masks"
    );
    let image = source.image()?;
    initialize_runtime()?;

    let detects = analysis.is_none();
    let mut analysis = match analysis {
        Some(analysis) => analysis,
        None => {
            job.progress(PeopleMaskProgress::FindingPeople);
            let detection = detect::detect(detector_path, &image)?;
            let analysis =
                PeopleAnalysis::new(detection, source.width as f32 / source.height as f32);
            job.progress(PeopleMaskProgress::Found(analysis.clone()));
            analysis
        }
    };
    job.ensure_not_cancelled()?;

    let quality = if foreground.is_some() {
        PeopleMaskQuality::High
    } else {
        PeopleMaskQuality::Medium
    };
    let people: Vec<Vec<usize>> = targets
        .iter()
        .map(|target| analysis.resolve(&target.anchors))
        .collect();
    let mut order: Vec<usize> = people.iter().flatten().copied().collect();
    order.sort_unstable();
    order.dedup();
    order.retain(|&person| !analysis.is_segmented(person, quality));
    let mut masks_sent = order.is_empty();
    if detects {
        for person in analysis.detection.by_prominence() {
            if !order.contains(&person) {
                order.push(person);
            }
        }
    }

    let send_masks = |analysis: &PeopleAnalysis| -> Result<()> {
        let masks = targets
            .iter()
            .zip(&people)
            .map(|(target, people)| {
                if people.is_empty() {
                    return Ok(None);
                }
                analysis
                    .mask_of(&image, people, &target.parts, quality)?
                    .context("a selected person is not segmented")
                    .map(Some)
            })
            .collect::<Result<_>>()?;
        job.progress(PeopleMaskProgress::Masks(masks));
        Ok(())
    };
    let targets_segmented = |analysis: &PeopleAnalysis| {
        people
            .iter()
            .flatten()
            .all(|&person| analysis.is_segmented(person, quality))
    };
    if masks_sent {
        send_masks(&analysis)?;
    }
    // Reports what was found of a person, and sends the masks once the
    // targets' people are segmented.
    let mut found_any = false;
    let mut found = |analysis: &mut PeopleAnalysis, person: usize, found: PersonFound| {
        let segmented = SegmentedPerson {
            detection: Arc::clone(&analysis.detection),
            person,
            found,
        };
        analysis.insert(segmented.clone());
        found_any = true;
        job.progress(PeopleMaskProgress::Segmented(segmented));
        if !masks_sent && targets_segmented(analysis) {
            masks_sent = true;
            send_masks(analysis)?;
        }
        anyhow::Ok(())
    };
    let (width, height) = image.dimensions();

    if let Some(model) = foreground {
        let outline: Vec<usize> = order
            .iter()
            .copied()
            .filter(|&person| analysis.foregrounds[person].is_none())
            .collect();
        if !outline.is_empty() {
            let ai_model = model.ai_model();
            let spec = ai_model.spec();
            let edge = spec.input.edge();
            with_model_session(
                ai_model,
                &model.path,
                spec.session,
                mask_model_retention(true),
                |session| {
                    for (done, &person) in outline.iter().enumerate() {
                        if job.ensure_not_cancelled().is_err() {
                            break;
                        }
                        job.progress(PeopleMaskProgress::OutliningPeople {
                            done,
                            total: outline.len(),
                        });
                        let crop = analysis.crop(person, width, height);
                        let person_foreground = foreground::segment(
                            session,
                            edge,
                            &image,
                            &analysis.detection,
                            person,
                            crop,
                        )?;
                        found(
                            &mut analysis,
                            person,
                            PersonFound::Foreground(Arc::new(person_foreground)),
                        )?;
                    }
                    Ok(())
                },
            )?;
        }
    }

    let segment: Vec<usize> = order
        .iter()
        .copied()
        .filter(|&person| analysis.parts[person].is_none())
        .collect();
    if !segment.is_empty() && job.ensure_not_cancelled().is_ok() {
        let spec = AiModel::BodyParts.spec();
        with_model_session(
            AiModel::BodyParts,
            body_parts_path,
            spec.session,
            mask_model_retention(true),
            |session| {
                for (done, &person) in segment.iter().enumerate() {
                    if job.ensure_not_cancelled().is_err() {
                        break;
                    }
                    job.progress(PeopleMaskProgress::SegmentingBodyParts {
                        done,
                        total: segment.len(),
                    });
                    let crop = analysis.crop(person, width, height);
                    let parts = parts::segment(session, &image, &analysis.detection, person, crop)?;
                    found(&mut analysis, person, PersonFound::Parts(Arc::new(parts)))?;
                }
                Ok(())
            },
        )?;
    }

    if let Some(file) = keep.filter(|_| detects || found_any) {
        let saved = content_fingerprint(&file.raw, &file.remove)
            .and_then(|content| analysis.save(&file.path, content));
        if let Err(error) = saved {
            let message = format!("{error:#}");
            calibraw_core::diagnostics::record(format!(
                "People analysis write failed for {}: {message}",
                file.path.display()
            ));
            job.progress(PeopleMaskProgress::NotKept(message));
        }
    }
    job.ensure_not_cancelled()?;
    Ok(analysis)
}
