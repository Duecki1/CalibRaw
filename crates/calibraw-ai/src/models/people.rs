//! People masks: RF-DETR-Seg finds the people in a photo and outlines them
//! coarsely, once per photo; Sapiens2 then segments the body parts of each
//! person chosen, on a crop around them, once per person. The outlines only
//! decide whose a pixel is; every edge comes from Sapiens2.

use crate::config::CAPABILITIES;
use crate::jobs::{spawn_job, AiJobEvent, JobContext, JobKind};
use crate::models::masks::mask_model_retention;
use crate::pipeline::{MaskImage, PersonPart};
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
use detect::Detection;
use parts::PersonParts;

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

pub fn people_models_are_verified(detector: &Path, body_parts: &Path) -> bool {
    AiModel::PersonDetector.spec().is_installed(detector)
        && AiModel::BodyParts.spec().is_installed(body_parts)
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
    /// The pixels covered by `bounds` (left, top, right, bottom as fractions),
    /// at least one.
    fn from_fractions(bounds: [f32; 4], width: u32, height: u32) -> Self {
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
    pub bounds: PersonBounds,
    /// The centre of their outline, as fractions of the photo: what a people
    /// mask stores to find them again.
    pub anchor: [f32; 2],
}

/// The people found in one mask source, and the body parts of those
/// segmented so far. Reused while the source is unchanged, so choosing
/// another person segments only them and choosing other parts runs no model.
#[derive(Clone)]
pub struct PeopleAnalysis {
    source_width: u32,
    source_height: u32,
    detection: Arc<Detection>,
    people: Arc<[DetectedPerson]>,
    /// By person, those segmented so far.
    parts: Vec<Option<Arc<PersonParts>>>,
}

impl fmt::Debug for PeopleAnalysis {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PeopleAnalysis")
            .field("source_width", &self.source_width)
            .field("source_height", &self.source_height)
            .field("people", &self.people)
            .finish_non_exhaustive()
    }
}

impl PeopleAnalysis {
    fn new(detection: Detection, source_width: u32, source_height: u32) -> Self {
        let people: Arc<[DetectedPerson]> = detection.people(source_width, source_height).into();
        Self {
            source_width,
            source_height,
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
        self.detection
            .person_at(anchor, self.source_width, self.source_height)
    }

    fn resolve(&self, anchor: Option<[f32; 2]>) -> Option<usize> {
        match anchor {
            Some(anchor) => self.person_at(anchor),
            None => self.detection.most_prominent(),
        }
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

pub struct PeopleMaskRequest {
    pub source_width: u32,
    pub source_height: u32,
    pub source_rgba: Vec<u8>,
    /// The analysis of an earlier job on the same source.
    pub analysis: Option<PeopleAnalysis>,
    pub targets: Vec<PeopleTarget>,
}

#[derive(Debug)]
pub struct PeopleMaskResult {
    pub analysis: PeopleAnalysis,
    /// One per target, in order; `None` where no one was found.
    pub masks: Vec<Option<PersonMask>>,
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

#[derive(Debug)]
pub enum PeopleMaskProgress {
    FindingPeople,
    SegmentingBodyParts,
}

pub type PeopleMaskEvent = AiJobEvent<PeopleMaskResult, PeopleMaskProgress>;

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

fn infer_people_masks(
    job: &JobContext<PeopleMaskResult, PeopleMaskProgress>,
    detector_path: &Path,
    body_parts_path: &Path,
    request: PeopleMaskRequest,
) -> Result<PeopleMaskResult> {
    let PeopleMaskRequest {
        source_width,
        source_height,
        source_rgba,
        analysis,
        targets,
    } = request;
    let pixels = u64::from(source_width) * u64::from(source_height);
    anyhow::ensure!(
        pixels > 0 && pixels <= MAX_SOURCE_PIXELS,
        "people-mask input {source_width}x{source_height} exceeds the {MAX_SOURCE_PIXELS}-pixel limit"
    );
    anyhow::ensure!(
        targets.len() <= MAX_TARGETS,
        "a people-mask job takes at most {MAX_TARGETS} masks"
    );
    let image = ImageBuffer::<Rgba<u8>, _>::from_raw(source_width, source_height, source_rgba)
        .context("invalid preview image for people masks")?;
    initialize_runtime()?;

    let mut analysis = match analysis.filter(|analysis| {
        analysis.source_width == source_width && analysis.source_height == source_height
    }) {
        Some(analysis) => analysis,
        None => {
            job.progress(PeopleMaskProgress::FindingPeople);
            let detection = detect::detect(detector_path, &image)?;
            PeopleAnalysis::new(detection, source_width, source_height)
        }
    };
    job.ensure_not_cancelled()?;

    let people: Vec<Option<usize>> = targets
        .iter()
        .map(|target| analysis.resolve(target.anchor))
        .collect();
    let mut unsegmented: Vec<usize> = people
        .iter()
        .flatten()
        .copied()
        .filter(|&person| analysis.parts[person].is_none())
        .collect();
    unsegmented.sort_unstable();
    unsegmented.dedup();
    if !unsegmented.is_empty() {
        job.progress(PeopleMaskProgress::SegmentingBodyParts);
        let spec = AiModel::BodyParts.spec();
        with_model_session(
            AiModel::BodyParts,
            body_parts_path,
            spec.session,
            mask_model_retention(true),
            |session| {
                for person in unsegmented {
                    job.ensure_not_cancelled()?;
                    let segmented = parts::segment(session, &image, &analysis.detection, person)?;
                    analysis.parts[person] = Some(Arc::new(segmented));
                }
                Ok(())
            },
        )?;
    }

    let masks = targets
        .iter()
        .zip(people)
        .map(|(target, person)| {
            let Some(person) = person else {
                return Ok(None);
            };
            let parts = analysis.parts[person]
                .as_ref()
                .context("a selected person has no body parts")?;
            Ok(Some(PersonMask {
                anchor: analysis.people[person].anchor,
                width: source_width,
                height: source_height,
                mask: parts.mask(&image, &target.parts)?,
            }))
        })
        .collect::<Result<_>>()?;
    Ok(PeopleMaskResult { analysis, masks })
}
