//! One person's body parts from Sapiens2, run on a crop around them, and the
//! masks made from them.

use super::detect::Detection;
use super::{PersonBounds, RgbaView};
use crate::models::{mask_refine, normalized_rgb_input};
use crate::pipeline::PersonPart;
use crate::placement::FallbackSession;
use crate::registry::{AiModel, InputShape};
use anyhow::{Context, Result};
use image::{imageops::FilterType, GrayImage};
use ort::value::Tensor;
use rayon::prelude::*;
use std::collections::BTreeSet;

const INPUT_SIZE: (u32, u32) = match AiModel::BodyParts.spec().input {
    InputShape::Stretched { width, height } => (width, height),
    InputShape::Square { .. } | InputShape::Tiled { .. } | InputShape::Prompted { .. } => {
        panic!("Sapiens2 takes a 3:4 input")
    }
};
pub(super) const INPUT_WIDTH: u32 = INPUT_SIZE.0;
pub(super) const INPUT_HEIGHT: u32 = INPUT_SIZE.1;
pub(super) const INPUT_PIXELS: usize = INPUT_WIDTH as usize * INPUT_HEIGHT as usize;
/// The Sapiens2 classes (docs/SEG.md in its repository), 0 being background.
const CLASSES: usize = 29;
/// The part each class belongs to besides `EntirePerson`, which holds every
/// class but the background. The tongue (28) belongs to none other.
const CLASS_PARTS: [Option<PersonPart>; CLASSES] = {
    use PersonPart::{BodySkin, Clothes, Eyeglasses, FaceNeck, Hair, Lips, Teeth};
    [
        None,             // Background
        Some(Clothes),    // Apparel
        Some(Eyeglasses), // Eyeglass
        Some(FaceNeck),   // Face_Neck
        Some(Hair),       // Hair
        Some(BodySkin),   // Left_Foot
        Some(BodySkin),   // Left_Hand
        Some(BodySkin),   // Left_Lower_Arm
        Some(BodySkin),   // Left_Lower_Leg
        Some(Clothes),    // Left_Shoe
        Some(Clothes),    // Left_Sock
        Some(BodySkin),   // Left_Upper_Arm
        Some(BodySkin),   // Left_Upper_Leg
        Some(Clothes),    // Lower_Clothing
        Some(BodySkin),   // Right_Foot
        Some(BodySkin),   // Right_Hand
        Some(BodySkin),   // Right_Lower_Arm
        Some(BodySkin),   // Right_Lower_Leg
        Some(Clothes),    // Right_Shoe
        Some(Clothes),    // Right_Sock
        Some(BodySkin),   // Right_Upper_Arm
        Some(BodySkin),   // Right_Upper_Leg
        Some(BodySkin),   // Torso
        Some(Clothes),    // Upper_Clothing
        Some(Lips),       // Lower_Lip
        Some(Lips),       // Upper_Lip
        Some(Teeth),      // Lower_Teeth
        Some(Teeth),      // Upper_Teeth
        None,             // Tongue
    ]
};
pub(super) const PARTS: usize = PersonPart::ALL.len();
/// The crop extends the person's box by this fraction of its longer side on
/// every side, so hair and hands just outside the box stay in it.
const CROP_MARGIN: f32 = 0.1;
/// Pixels outside every outline go to the nearest outline within this
/// fraction of the person's longer side, which reaches the hair and fingers
/// the coarse outlines miss but not someone the detector did not find.
const OUTLINE_REACH: f32 = 0.1;

/// The probabilities of each part of one person over a crop around them.
pub(super) struct PersonParts {
    /// The part of the photo the planes cover: left, top, right and bottom
    /// as fractions of the photo, on pixel edges of the photo segmented.
    pub(super) crop: [f32; 4],
    /// `INPUT_WIDTH` x `INPUT_HEIGHT` pixels stretched over `crop`, by row:
    /// each part's probability, 0 to 255, in `PersonPart::ALL` order. Zero
    /// where the pixel belongs to someone else.
    pub(super) probabilities: Box<[[u8; PARTS]]>,
}

impl PersonParts {
    /// The photo's mask of `parts`, its edges refined against the photo.
    pub(super) fn mask(
        &self,
        image: &RgbaView<'_>,
        parts: &BTreeSet<PersonPart>,
    ) -> Result<Vec<u8>> {
        let planes: Vec<usize> = if parts.contains(&PersonPart::EntirePerson) {
            vec![part_index(PersonPart::EntirePerson)]
        } else {
            parts.iter().map(|&part| part_index(part)).collect()
        };
        let selected = self
            .probabilities
            .par_iter()
            .map(|pixel| {
                let sum: u16 = planes.iter().map(|&plane| u16::from(pixel[plane])).sum();
                sum.min(255) as u8
            })
            .collect();
        let selected = GrayImage::from_raw(INPUT_WIDTH, INPUT_HEIGHT, selected)
            .context("invalid body-part plane")?;
        let PersonBounds {
            x,
            y,
            width,
            height,
        } = crop_pixels(self.crop, image.width(), image.height());
        let mut alpha =
            image::imageops::resize(&selected, width, height, FilterType::Triangle).into_raw();
        let photo_width = image.width() as usize;
        let mut guide = Vec::with_capacity(width as usize * height as usize * 4);
        for row in y as usize..(y + height) as usize {
            let start = (row * photo_width + x as usize) * 4;
            guide.extend_from_slice(&image.as_raw()[start..start + width as usize * 4]);
        }
        // About two model pixels: enough to move a boundary onto the photo's
        // edge, too little to bleed into a neighbouring part.
        let radius = (width.max(height) / 512).clamp(2, 8);
        mask_refine::guided_filter_color(&guide, &mut alpha, width, height, radius, 1e-4)?;

        let mut mask = vec![0u8; photo_width * image.height() as usize];
        for (row, source) in alpha.chunks_exact(width as usize).enumerate() {
            let start = (y as usize + row) * photo_width + x as usize;
            mask[start..start + width as usize].copy_from_slice(source);
        }
        Ok(mask)
    }
}

const fn part_index(part: PersonPart) -> usize {
    match part {
        PersonPart::EntirePerson => 0,
        PersonPart::FaceNeck => 1,
        PersonPart::BodySkin => 2,
        PersonPart::Hair => 3,
        PersonPart::Clothes => 4,
        PersonPart::Lips => 5,
        PersonPart::Teeth => 6,
        PersonPart::Eyeglasses => 7,
    }
}

/// `crop` (fractions of the photo) in pixels of a `width` x `height` photo.
/// Exactly the pixels it was made from on a photo of the size segmented,
/// the nearest ones on another.
fn crop_pixels(crop: [f32; 4], width: u32, height: u32) -> PersonBounds {
    let edge = |fraction: f32, limit: u32| {
        ((f64::from(fraction) * f64::from(limit)).round() as u32).min(limit)
    };
    let [left, top, right, bottom] = crop;
    let x = edge(left, width).min(width.saturating_sub(1));
    let y = edge(top, height).min(height.saturating_sub(1));
    PersonBounds {
        x,
        y,
        width: edge(right, width).max(x + 1) - x,
        height: edge(bottom, height).max(y + 1) - y,
    }
}

/// Runs Sapiens2 on a crop around `person` and keeps the parts of the pixels
/// that are theirs.
pub(super) fn segment(
    session: &mut FallbackSession,
    image: &RgbaView<'_>,
    detection: &Detection,
    person: usize,
) -> Result<PersonParts> {
    let (photo_width, photo_height) = image.dimensions();
    let crop = model_crop(detection.bounds(person), photo_width, photo_height);
    let cropped = image::imageops::crop_imm(image, crop.x, crop.y, crop.width, crop.height);
    let resized =
        image::imageops::resize(&*cropped, INPUT_WIDTH, INPUT_HEIGHT, FilterType::Triangle);
    let input = Tensor::from_array((
        [1usize, 3, INPUT_HEIGHT as usize, INPUT_WIDTH as usize],
        normalized_rgb_input(&resized)?,
    ))
    .context("create Sapiens2 input tensor")?;
    let owned = ownership(detection, person, crop, photo_width, photo_height);
    let probabilities = session.run_with_fallback("Sapiens2 inference", |ort_session, _| {
        let outputs = ort_session
            .run(ort::inputs![&input])
            .context("run Sapiens2 inference")?;
        let (shape, logits) = outputs
            .get("logits")
            .context("Sapiens2 returned no logits")?
            .try_extract_tensor::<f32>()
            .context("read Sapiens2 logits")?;
        anyhow::ensure!(
            **shape
                == [
                    1,
                    CLASSES as i64,
                    i64::from(INPUT_HEIGHT),
                    i64::from(INPUT_WIDTH)
                ]
                && logits.len() == CLASSES * INPUT_PIXELS,
            "unexpected Sapiens2 output shape {shape:?}"
        );
        part_probabilities(logits, &owned)
    })?;
    let (width, height) = (f64::from(photo_width), f64::from(photo_height));
    let fraction = |pixels: u32, limit: f64| (f64::from(pixels) / limit) as f32;
    Ok(PersonParts {
        crop: [
            fraction(crop.x, width),
            fraction(crop.y, height),
            fraction(crop.x + crop.width, width),
            fraction(crop.y + crop.height, height),
        ],
        probabilities,
    })
}

/// Per pixel, the softmax of the class logits summed into parts; zero where
/// `owned` is false. `logits` is class-major, `CLASSES` planes.
fn part_probabilities(logits: &[f32], owned: &[bool]) -> Result<Box<[[u8; PARTS]]>> {
    let probabilities = (0..INPUT_PIXELS)
        .into_par_iter()
        .map(|pixel| {
            if !owned[pixel] {
                return Ok([0; PARTS]);
            }
            let logit = |class: usize| logits[class * INPUT_PIXELS + pixel];
            let mut largest = f32::NEG_INFINITY;
            for class in 0..CLASSES {
                anyhow::ensure!(
                    logit(class).is_finite(),
                    "Sapiens2 returned non-finite logits"
                );
                largest = largest.max(logit(class));
            }
            let mut parts = [0.0f32; PARTS];
            let mut total = 0.0f32;
            for (class, part) in CLASS_PARTS.iter().enumerate() {
                let weight = (logit(class) - largest).exp();
                total += weight;
                if let Some(part) = part {
                    parts[part_index(*part)] += weight;
                }
                if class != 0 {
                    parts[part_index(PersonPart::EntirePerson)] += weight;
                }
            }
            Ok(parts.map(|weight: f32| ((weight / total).clamp(0.0, 1.0) * 255.0 + 0.5) as u8))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(probabilities.into_boxed_slice())
}

/// The crop Sapiens2 sees for a person: their box (fractions of the photo)
/// with a margin, widened or heightened to the input's aspect ratio where
/// the photo allows, so the person is stretched as little as possible.
fn model_crop(bounds: [f32; 4], photo_width: u32, photo_height: u32) -> PersonBounds {
    let (width, height) = (photo_width as f32, photo_height as f32);
    let [left, top, right, bottom] = bounds;
    let (mut x0, mut y0) = (left * width, top * height);
    let (mut x1, mut y1) = (right * width, bottom * height);
    let margin = CROP_MARGIN * (x1 - x0).max(y1 - y0);
    (x0, y0, x1, y1) = (x0 - margin, y0 - margin, x1 + margin, y1 + margin);
    let aspect = INPUT_WIDTH as f32 / INPUT_HEIGHT as f32;
    let extra_width = (y1 - y0) * aspect - (x1 - x0);
    if extra_width > 0.0 {
        (x0, x1) = (x0 - extra_width * 0.5, x1 + extra_width * 0.5);
    } else {
        let extra_height = (x1 - x0) / aspect - (y1 - y0);
        (y0, y1) = (y0 - extra_height * 0.5, y1 + extra_height * 0.5);
    }
    let (x0, x1) = fit_span(x0, x1, width);
    let (y0, y1) = fit_span(y0, y1, height);
    let x = (x0.floor() as u32).min(photo_width - 1);
    let y = (y0.floor() as u32).min(photo_height - 1);
    PersonBounds {
        x,
        y,
        width: (x1.ceil() as u32).clamp(x + 1, photo_width) - x,
        height: (y1.ceil() as u32).clamp(y + 1, photo_height) - y,
    }
}

/// Moves the span [`start`, `end`] inside [0, `limit`], or clips it where it
/// is longer.
fn fit_span(start: f32, end: f32, limit: f32) -> (f32, f32) {
    let length = end - start;
    if length >= limit {
        (0.0, limit)
    } else if start < 0.0 {
        (0.0, length)
    } else if end > limit {
        (limit - length, limit)
    } else {
        (start, end)
    }
}

/// Which pixels of the model input are `person`'s. A pixel inside one or
/// more outlines belongs to the likeliest; one outside every outline belongs
/// to the nearest outline within `OUTLINE_REACH`, and to no one beyond it.
/// Every candidate competes, also those not offered as people. Distances are
/// in input pixels, chamfer-approximated.
fn ownership(
    detection: &Detection,
    person: usize,
    crop: PersonBounds,
    photo_width: u32,
    photo_height: u32,
) -> Vec<bool> {
    const NO_ONE: usize = usize::MAX;
    let (width, height) = (INPUT_WIDTH as usize, INPUT_HEIGHT as usize);
    let to_u = |x: usize| {
        (crop.x as f32 + (x as f32 + 0.5) / width as f32 * crop.width as f32) / photo_width as f32
    };
    let to_v = |y: usize| {
        (crop.y as f32 + (y as f32 + 0.5) / height as f32 * crop.height as f32)
            / photo_height as f32
    };
    let crop_fractions = [
        crop.x as f32 / photo_width as f32,
        crop.y as f32 / photo_height as f32,
        (crop.x + crop.width) as f32 / photo_width as f32,
        (crop.y + crop.height) as f32 / photo_height as f32,
    ];
    let nearby = detection.candidates_overlapping(crop_fractions);

    // Owners inside the outlines; `NO_ONE` elsewhere.
    let mut owner = vec![NO_ONE; INPUT_PIXELS];
    owner
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            let v = to_v(y);
            for (x, owner) in row.iter_mut().enumerate() {
                let u = to_u(x);
                let likeliest = nearby
                    .iter()
                    .map(|&candidate| (candidate, detection.outline_probability(candidate, u, v)))
                    .max_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((candidate, probability)) = likeliest {
                    if probability >= 0.5 {
                        *owner = candidate;
                    }
                }
            }
        });

    // Two chamfer passes carry each outline's owner outward with its distance.
    let mut distance: Vec<f32> = owner
        .iter()
        .map(|&owner| if owner == NO_ONE { f32::INFINITY } else { 0.0 })
        .collect();
    let diagonal = std::f32::consts::SQRT_2;
    let forward = [
        (-1isize, 0isize, 1.0f32),
        (-1, -1, diagonal),
        (0, -1, 1.0),
        (1, -1, diagonal),
    ];
    let backward = forward.map(|(dx, dy, cost)| (-dx, -dy, cost));
    let mut relax = |x: usize, y: usize, steps: &[(isize, isize, f32)]| {
        let index = y * width + x;
        for &(dx, dy, cost) in steps {
            let (nx, ny) = (x as isize + dx, y as isize + dy);
            if nx < 0 || ny < 0 || nx >= width as isize || ny >= height as isize {
                continue;
            }
            let neighbour = ny as usize * width + nx as usize;
            if distance[neighbour] + cost < distance[index] {
                distance[index] = distance[neighbour] + cost;
                owner[index] = owner[neighbour];
            }
        }
    };
    for y in 0..height {
        for x in 0..width {
            relax(x, y, &forward);
        }
    }
    for y in (0..height).rev() {
        for x in (0..width).rev() {
            relax(x, y, &backward);
        }
    }

    let [left, top, right, bottom] = detection.bounds(person);
    let box_width = (right - left) * photo_width as f32 / crop.width as f32 * width as f32;
    let box_height = (bottom - top) * photo_height as f32 / crop.height as f32 * height as f32;
    let reach = OUTLINE_REACH * box_width.max(box_height);
    owner
        .iter()
        .zip(&distance)
        .map(|(&owner, &distance)| owner == person && distance <= reach)
        .collect()
}
