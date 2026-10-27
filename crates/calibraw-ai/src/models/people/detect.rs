//! Who is where in a photo, from RF-DETR-Seg: a box, a score and a coarse
//! outline per person. The outlines only decide whose a pixel is; Sapiens2's
//! body parts supply every edge.

use super::{DetectedPerson, RgbaView};
use crate::models::masks::mask_model_retention;
use crate::models::{normalized_rgb_input, sigmoid_probability};
use crate::registry::AiModel;
use crate::slot::with_model_session;
use anyhow::{Context, Result};
use image::imageops::FilterType;
use ort::session::SessionOutputs;
use ort::value::Tensor;
use std::path::Path;

const INPUT_EDGE: u32 = AiModel::PersonDetector.spec().input.edge();
/// The person logit's slot: RF-DETR's COCO checkpoints use COCO category IDs.
const PERSON_CLASS: usize = 1;
/// Candidates at least this likely are offered as people.
const PERSON_SCORE: f32 = 0.5;
/// Less likely candidates are not offered but still claim the pixels inside
/// their outline, so someone the detector was unsure of does not become part
/// of the person beside them.
const CANDIDATE_SCORE: f32 = 0.25;
/// A candidate whose box overlaps a likelier one's by more than this
/// intersection over union is the same person found twice.
const DUPLICATE_OVERLAP: f32 = 0.7;
/// At most this many people are offered, the likeliest ones; a people mask
/// can select them all.
pub(super) const MAX_PEOPLE: usize = crate::pipeline::MAX_MASK_PEOPLE;

/// The people and other candidates found in one photo.
pub(super) struct Detection {
    /// The offered people left to right, then the other candidates.
    candidates: Vec<Candidate>,
    /// Offered people: the first entries of `candidates`.
    people: usize,
    /// Edge of every outline grid.
    outline_edge: usize,
}

pub(super) struct Candidate {
    pub(super) score: f32,
    /// Left, top, right and bottom as fractions of the photo.
    pub(super) bounds: [f32; 4],
    /// Outline logits over the whole photo, `outline_edge` squared, by row.
    /// Positive inside the person.
    pub(super) outline: Box<[f32]>,
}

impl Candidate {
    /// The centre of the outline, or of the box where the outline is empty,
    /// as fractions of the photo.
    fn anchor(&self, edge: usize) -> [f32; 2] {
        let (mut sum_x, mut sum_y, mut count) = (0.0f64, 0.0f64, 0u32);
        for (index, &logit) in self.outline.iter().enumerate() {
            if logit >= 0.0 {
                sum_x += (index % edge) as f64 + 0.5;
                sum_y += (index / edge) as f64 + 0.5;
                count += 1;
            }
        }
        if count == 0 {
            let [left, top, right, bottom] = self.bounds;
            return [(left + right) * 0.5, (top + bottom) * 0.5];
        }
        let scale = f64::from(count) * edge as f64;
        [(sum_x / scale) as f32, (sum_y / scale) as f32]
    }

    fn outline_area(&self) -> usize {
        self.outline.iter().filter(|&&logit| logit >= 0.0).count()
    }
}

impl Detection {
    pub(super) fn people(&self) -> Vec<DetectedPerson> {
        self.candidates[..self.people]
            .iter()
            .map(|candidate| DetectedPerson {
                bounds: candidate.bounds,
                anchor: candidate.anchor(self.outline_edge),
            })
            .collect()
    }

    /// The offered person whose outline is largest.
    pub(super) fn most_prominent(&self) -> Option<usize> {
        (0..self.people).max_by_key(|&person| self.candidates[person].outline_area())
    }

    /// The offered people, largest outline first.
    pub(super) fn by_prominence(&self) -> Vec<usize> {
        let mut people: Vec<usize> = (0..self.people).collect();
        people.sort_by_key(|&person| std::cmp::Reverse(self.candidates[person].outline_area()));
        people
    }

    /// The offered person at `anchor` (fractions of a photo `aspect` times
    /// as wide as it is high): the likeliest one whose outline contains it,
    /// else the one whose own anchor is nearest.
    pub(super) fn person_at(&self, anchor: [f32; 2], aspect: f32) -> Option<usize> {
        let [u, v] = anchor;
        let containing = (0..self.people)
            .map(|person| (person, self.outline_probability(person, u, v)))
            .filter(|&(_, probability)| probability >= 0.5)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(person, _)| person);
        containing.or_else(|| {
            // In photo heights, so both axes share a unit.
            let distance = |person: usize| {
                let [x, y] = self.candidates[person].anchor(self.outline_edge);
                let dx = f64::from(x - u) * f64::from(aspect);
                let dy = f64::from(y - v);
                dx * dx + dy * dy
            };
            (0..self.people).min_by(|&a, &b| distance(a).total_cmp(&distance(b)))
        })
    }

    pub(super) fn bounds(&self, person: usize) -> [f32; 4] {
        self.candidates[person].bounds
    }

    /// Every candidate, the offered people first; how many of them are
    /// offered; and the edge of their outline grids. What `from_parts` takes.
    pub(super) fn parts(&self) -> (&[Candidate], usize, usize) {
        (&self.candidates, self.people, self.outline_edge)
    }

    /// A detection from what `parts` returned, checked as an untrusted file
    /// is: in that order, the offered people still left to right.
    pub(super) fn from_parts(
        candidates: Vec<Candidate>,
        people: usize,
        outline_edge: usize,
    ) -> Result<Self> {
        anyhow::ensure!(
            people <= candidates.len().min(MAX_PEOPLE),
            "{people} people of {} candidates",
            candidates.len()
        );
        anyhow::ensure!(outline_edge > 0, "empty outline grid");
        for candidate in &candidates {
            let [left, top, right, bottom] = candidate.bounds;
            anyhow::ensure!(
                candidate.score.is_finite()
                    && candidate
                        .bounds
                        .iter()
                        .all(|value| (0.0..=1.0).contains(value))
                    && left <= right
                    && top <= bottom,
                "invalid candidate box"
            );
            anyhow::ensure!(
                candidate.outline.len() == outline_edge * outline_edge
                    && candidate.outline.iter().all(|logit| logit.is_finite()),
                "invalid candidate outline"
            );
        }
        Ok(Self {
            candidates,
            people,
            outline_edge,
        })
    }

    /// Every candidate whose box overlaps `region` (fractions: left, top,
    /// right, bottom), the offered people included.
    pub(super) fn candidates_overlapping(&self, region: [f32; 4]) -> Vec<usize> {
        (0..self.candidates.len())
            .filter(|&index| {
                let [left, top, right, bottom] = self.candidates[index].bounds;
                left < region[2] && right > region[0] && top < region[3] && bottom > region[1]
            })
            .collect()
    }

    /// How likely the point (`u`, `v`), in fractions of the photo, lies in
    /// candidate `index`'s outline. Bilinear between grid cell centres.
    pub(super) fn outline_probability(&self, index: usize, u: f32, v: f32) -> f32 {
        let edge = self.outline_edge;
        let outline = &self.candidates[index].outline;
        let last = (edge - 1) as f32;
        let x = (u * edge as f32 - 0.5).clamp(0.0, last);
        let y = (v * edge as f32 - 0.5).clamp(0.0, last);
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(edge - 1), (y0 + 1).min(edge - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let top = outline[y0 * edge + x0] * (1.0 - fx) + outline[y0 * edge + x1] * fx;
        let bottom = outline[y1 * edge + x0] * (1.0 - fx) + outline[y1 * edge + x1] * fx;
        sigmoid_probability(top * (1.0 - fy) + bottom * fy)
    }
}

/// Runs RF-DETR-Seg on the whole photo, stretched to its square input.
pub(super) fn detect(model_path: &Path, image: &RgbaView<'_>) -> Result<Detection> {
    let resized = image::imageops::resize(image, INPUT_EDGE, INPUT_EDGE, FilterType::Triangle);
    let edge = INPUT_EDGE as usize;
    let input = Tensor::from_array(([1usize, 3, edge, edge], normalized_rgb_input(&resized)?))
        .context("create RF-DETR-Seg input tensor")?;
    let (outline_edge, candidates) = with_model_session(
        AiModel::PersonDetector,
        model_path,
        AiModel::PersonDetector.spec().session,
        mask_model_retention(true),
        |session| {
            session.run_with_fallback("RF-DETR-Seg inference", |ort_session, _| {
                let outputs = ort_session
                    .run(ort::inputs![&input])
                    .context("run RF-DETR-Seg inference")?;
                read_candidates(&outputs)
            })
        },
    )?;
    Ok(Detection::new(candidates, outline_edge))
}

/// The candidates at least `CANDIDATE_SCORE` likely to be people, and the
/// edge of their outline grids.
fn read_candidates(outputs: &SessionOutputs<'_>) -> Result<(usize, Vec<Candidate>)> {
    let tensor = |name: &str| {
        outputs
            .get(name)
            .with_context(|| format!("RF-DETR-Seg returned no {name} output"))?
            .try_extract_tensor::<f32>()
            .with_context(|| format!("read the RF-DETR-Seg {name} output"))
    };
    let (box_shape, boxes) = tensor("dets")?;
    let (label_shape, labels) = tensor("labels")?;
    let (outline_shape, outlines) = tensor("masks")?;
    anyhow::ensure!(
        box_shape.len() == 3 && box_shape[0] == 1 && box_shape[2] == 4,
        "unexpected RF-DETR-Seg box shape {box_shape:?}; expected [1, Q, 4]"
    );
    let queries = usize::try_from(box_shape[1]).context("invalid RF-DETR-Seg query count")?;
    anyhow::ensure!(
        label_shape.len() == 3
            && label_shape[0] == 1
            && label_shape[1] == box_shape[1]
            && label_shape[2] > PERSON_CLASS as i64,
        "unexpected RF-DETR-Seg label shape {label_shape:?}"
    );
    anyhow::ensure!(
        outline_shape.len() == 4
            && outline_shape[0] == 1
            && outline_shape[1] == box_shape[1]
            && outline_shape[2] == outline_shape[3]
            && outline_shape[2] > 0,
        "unexpected RF-DETR-Seg mask shape {outline_shape:?}"
    );
    let classes = label_shape[2] as usize;
    let outline_edge = outline_shape[2] as usize;
    let outline_len = outline_edge * outline_edge;
    anyhow::ensure!(
        boxes.len() == queries * 4
            && labels.len() == queries * classes
            && outlines.len() == queries * outline_len,
        "RF-DETR-Seg outputs do not match their shapes"
    );
    let mut candidates = Vec::new();
    for query in 0..queries {
        let logit = labels[query * classes + PERSON_CLASS];
        anyhow::ensure!(logit.is_finite(), "RF-DETR-Seg returned non-finite scores");
        let score = sigmoid_probability(logit);
        if score < CANDIDATE_SCORE {
            continue;
        }
        let [centre_x, centre_y, width, height] = <[f32; 4]>::try_from(&boxes[query * 4..][..4])
            .context("RF-DETR-Seg box has four values")?;
        let outline = &outlines[query * outline_len..][..outline_len];
        anyhow::ensure!(
            [centre_x, centre_y, width, height]
                .iter()
                .chain(outline)
                .all(|value| value.is_finite()),
            "RF-DETR-Seg returned non-finite boxes or masks"
        );
        candidates.push(Candidate {
            score,
            bounds: [
                (centre_x - width * 0.5).clamp(0.0, 1.0),
                (centre_y - height * 0.5).clamp(0.0, 1.0),
                (centre_x + width * 0.5).clamp(0.0, 1.0),
                (centre_y + height * 0.5).clamp(0.0, 1.0),
            ],
            outline: outline.into(),
        });
    }
    Ok((outline_edge, candidates))
}

impl Detection {
    fn new(mut found: Vec<Candidate>, outline_edge: usize) -> Self {
        found.sort_by(|a, b| b.score.total_cmp(&a.score));
        let mut candidates: Vec<Candidate> = Vec::with_capacity(found.len());
        for candidate in found {
            let duplicate = candidates
                .iter()
                .any(|kept| box_overlap(kept.bounds, candidate.bounds) > DUPLICATE_OVERLAP);
            if !duplicate {
                candidates.push(candidate);
            }
        }
        let people = candidates
            .iter()
            .take_while(|candidate| candidate.score >= PERSON_SCORE)
            .count()
            .min(MAX_PEOPLE);
        candidates[..people].sort_by(|a, b| {
            let a = a.anchor(outline_edge)[0];
            let b = b.anchor(outline_edge)[0];
            a.total_cmp(&b)
        });
        Self {
            candidates,
            people,
            outline_edge,
        }
    }
}

/// Intersection over union of two boxes.
fn box_overlap(a: [f32; 4], b: [f32; 4]) -> f32 {
    let width = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let height = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let intersection = width * height;
    let area = |bounds: [f32; 4]| (bounds[2] - bounds[0]) * (bounds[3] - bounds[1]);
    let union = area(a) + area(b) - intersection;
    if union > 0.0 {
        intersection / union
    } else {
        0.0
    }
}
