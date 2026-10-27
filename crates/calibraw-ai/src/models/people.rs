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

    /// The person a people mask anchored at `anchor` selects: the one whose
    /// outline contains it, else the one whose anchor is nearest. `None`
    /// only when no one was found.
    pub fn person_at(&self, anchor: [f32; 2]) -> Option<usize> {
        self.detection.person_at(anchor, self.aspect)
    }

    /// `person_at`, or the most prominent person for a mask that has not
    /// chosen one yet.
    pub fn resolve(&self, anchor: Option<[f32; 2]>) -> Option<usize> {
        match anchor {
            Some(anchor) => self.person_at(anchor),
            None => self.detection.most_prominent(),
        }
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

    /// The mask of `parts` of `person` over `source`, its edges refined
    /// against it; runs no model. `None` until the person is segmented.
    pub fn person_mask(
        &self,
        source: &PeopleSource,
        person: usize,
        parts: &BTreeSet<PersonPart>,
    ) -> Result<Option<PersonMask>> {
        self.mask_of(&source.image()?, person, parts)
    }

    fn mask_of(
        &self,
        image: &RgbaView<'_>,
        person: usize,
        parts: &BTreeSet<PersonPart>,
    ) -> Result<Option<PersonMask>> {
        let Some(segmented) = self.parts.get(person).and_then(Option::as_ref) else {
            return Ok(None);
        };
        Ok(Some(PersonMask {
            anchor: self.people[person].anchor,
            width: image.width(),
            height: image.height(),
            mask: segmented.mask(image, parts)?,
        }))
    }
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
    /// A point on the person, as fractions of the photo; `None` selects the
    /// most prominent person.
    pub anchor: Option<[f32; 2]>,
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
    /// The anchor of the person found, which the target's mask stores.
    pub anchor: [f32; 2],
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

    let people: Vec<Option<usize>> = targets
        .iter()
        .map(|target| analysis.resolve(target.anchor))
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
            .map(|(target, person)| match person {
                Some(person) => analysis
                    .mask_of(&image, *person, &target.parts)?
                    .context("a selected person has no body parts")
                    .map(Some),
                None => Ok(None),
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
