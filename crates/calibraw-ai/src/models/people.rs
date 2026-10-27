//! People masks: RF-DETR-Seg finds the people in a photo and outlines them
//! coarsely; Sapiens2 then segments the body parts of each person, on a crop
//! around them. Both run once per photo, for everyone: the masks of any
//! person and parts are then made from that analysis without a model
//! (`PeopleAnalysis::person_mask`), and the analysis is kept beside the photo
//! (`store`) for the next time it opens. The outlines only decide whose a
//! pixel is; every edge comes from Sapiens2.

use crate::config::CAPABILITIES;
use crate::jobs::{spawn_job, AiJobEvent, JobContext, JobKind};
use crate::models::masks::mask_model_retention;
use crate::pipeline::{LoadedRaw, MaskImage, PersonPart, RemoveEditState};
use crate::registry::{AiModel, ModelInfo};
use crate::runtime::initialize_runtime;
use crate::slot::with_model_session;
use anyhow::{Context, Result};
use image::{ImageBuffer, Rgba};
use std::{
    collections::BTreeSet,
    fmt,
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, mpsc, Arc},
};

mod detect;
mod parts;
mod store;
use detect::Detection;
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

pub fn people_models_are_verified(detector: &Path, body_parts: &Path) -> bool {
    AiModel::PersonDetector.spec().is_installed(detector)
        && AiModel::BodyParts.spec().is_installed(body_parts)
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

/// The people found in one photo, and the body parts of those segmented so
/// far. Positions are fractions of the photo, so the analysis serves every
/// rendering of the same content, whatever its size.
#[derive(Clone)]
pub struct PeopleAnalysis {
    detection: Arc<Detection>,
    people: Arc<[DetectedPerson]>,
    /// Width over height of the photo analysed.
    aspect: f32,
    /// By person, those segmented so far.
    parts: Vec<Option<Arc<PersonParts>>>,
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

    /// Whether `person`'s body parts are known, so their masks need no model.
    pub fn is_segmented(&self, person: usize) -> bool {
        self.parts.get(person).is_some_and(Option::is_some)
    }

    /// Adds a person a job segmented for this analysis. False, changing
    /// nothing, for one of another analysis.
    pub fn insert(&mut self, segmented: SegmentedPerson) -> bool {
        if !Arc::ptr_eq(&self.detection, &segmented.detection) {
            return false;
        }
        let Some(slot) = self.parts.get_mut(segmented.person) else {
            return false;
        };
        *slot = Some(segmented.parts);
        true
    }

    /// The mask of `parts` of `people` over `source`, its edges refined
    /// against it; runs no model. `None` for no one, or until everyone is
    /// segmented.
    pub fn person_mask(
        &self,
        source: &PeopleSource,
        people: &[usize],
        parts: &BTreeSet<PersonPart>,
    ) -> Result<Option<PersonMask>> {
        self.mask_of(&source.image()?, people, parts)
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
    ) -> Result<Option<PersonMask>> {
        let Some(segmented) = self.segmented(people) else {
            return Ok(None);
        };
        let mut masks = segmented.iter().map(|person| person.mask(image, parts));
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

/// One person's body parts, as a job reports them while it runs.
#[derive(Clone)]
pub struct SegmentedPerson {
    detection: Arc<Detection>,
    person: usize,
    parts: Arc<PersonParts>,
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
    pub allow_download: bool,
    pub request: PeopleMaskRequest,
    pub cancellation: Arc<AtomicBool>,
}

pub fn spawn_people_mask(worker: PeopleMaskWorkerRequest) -> mpsc::Receiver<PeopleMaskEvent> {
    let PeopleMaskWorkerRequest {
        detector_path,
        body_parts_path,
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
        job.ensure_not_cancelled()?;
        infer_people_masks(job, &detector_path, &body_parts_path, request)
    })
}

/// Finds the people unless `request` brings them, segments the targets'
/// people and sends their masks, then segments everyone else the job found.
/// A cancelled job keeps and reports those segmented before it stopped.
fn infer_people_masks(
    job: &JobContext<PeopleAnalysis, PeopleMaskProgress>,
    detector_path: &Path,
    body_parts_path: &Path,
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

    let people: Vec<Vec<usize>> = targets
        .iter()
        .map(|target| analysis.resolve(&target.anchors))
        .collect();
    let mut order: Vec<usize> = people.iter().flatten().copied().collect();
    order.sort_unstable();
    order.dedup();
    order.retain(|&person| !analysis.is_segmented(person));
    let targets_waiting = order.len();
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
                    .mask_of(&image, people, &target.parts)?
                    .context("a selected person has no body parts")
                    .map(Some)
            })
            .collect::<Result<_>>()?;
        job.progress(PeopleMaskProgress::Masks(masks));
        Ok(())
    };
    if targets_waiting == 0 {
        send_masks(&analysis)?;
    }
    let mut segmented_any = false;
    if !order.is_empty() {
        let spec = AiModel::BodyParts.spec();
        with_model_session(
            AiModel::BodyParts,
            body_parts_path,
            spec.session,
            mask_model_retention(true),
            |session| {
                for (done, &person) in order.iter().enumerate() {
                    if job.ensure_not_cancelled().is_err() {
                        break;
                    }
                    job.progress(PeopleMaskProgress::SegmentingBodyParts {
                        done,
                        total: order.len(),
                    });
                    let parts = parts::segment(session, &image, &analysis.detection, person)?;
                    let segmented = SegmentedPerson {
                        detection: Arc::clone(&analysis.detection),
                        person,
                        parts: Arc::new(parts),
                    };
                    analysis.insert(segmented.clone());
                    segmented_any = true;
                    job.progress(PeopleMaskProgress::Segmented(segmented));
                    if done + 1 == targets_waiting {
                        send_masks(&analysis)?;
                    }
                }
                Ok(())
            },
        )?;
    }

    if let Some(file) = keep.filter(|_| detects || segmented_any) {
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
