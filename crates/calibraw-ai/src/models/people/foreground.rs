//! BiRefNet's foreground over each person's crop. For high-quality masks its
//! fine outline replaces Sapiens2's soft one near its edge, while Sapiens2
//! still decides which part each pixel is. Without Sapiens2 it replaces
//! RF-DETR-Seg's coarse outline the same way, for the entire person.

use super::detect::Detection;
use super::distance::{nearest_seeds, NO_SEED};
use super::parts::{
    crop_pixels, ownership, part_index, plane_index, plane_u, plane_v, PersonParts, INPUT_HEIGHT,
    INPUT_PIXELS, INPUT_WIDTH, PARTS,
};
use super::RgbaView;
use crate::models::subject::infer_birefnet;
use crate::pipeline::PersonPart;
use crate::placement::FallbackSession;
use anyhow::{Context, Result};
use image::RgbaImage;
use rayon::prelude::*;

/// Within this many plane pixels of Sapiens2's outline (where its entire
/// person is half likely), BiRefNet draws the edge. Pixels further inside
/// keep the person whatever BiRefNet says, and BiRefNet adds nothing further
/// outside: it finds every salient thing in the crop, not this person. About
/// 1.6% of the crop's height, wide enough for hair Sapiens2 rounds off.
/// BiRefNet draws it only within as far of its own foreground: where it found
/// nothing nearby, such as a foot in shadow, it missed a part rather than
/// found its edge, and Sapiens2's outline stays.
const BAND: f32 = 16.0;
/// When BiRefNet's foreground and Sapiens2's person overlap by less than this
/// intersection over union, BiRefNet found something else, and the person
/// keeps Sapiens2's planes.
const MIN_AGREEMENT: f32 = 0.6;
/// Pixels at least this likely the person, of 255, lend the shares of their
/// parts to the pixels BiRefNet adds beyond Sapiens2's outline.
const SHARE_FLOOR: u8 = 64;
/// RF-DETR-Seg's outline grid spans the whole photo, so its outline of
/// someone small is coarse and may miss their edge by about a cell. Without
/// Sapiens2, BiRefNet draws the edge within this many cells of that outline,
/// and at least `BAND` plane pixels.
const OUTLINE_BAND_CELLS: f32 = 2.0;

/// BiRefNet's foreground over a crop around one person.
pub(super) struct PersonForeground {
    /// The part of the photo the plane covers: left, top, right and bottom as
    /// fractions of the photo. The same crop as the person's part planes.
    pub(super) crop: [f32; 4],
    /// `INPUT_WIDTH` x `INPUT_HEIGHT` pixels stretched over `crop`, by row:
    /// how likely each is foreground, 0 to 255. Zero where the pixel belongs
    /// to someone else.
    pub(super) probabilities: Box<[u8]>,
}

impl PersonForeground {
    /// How likely `point` (fractions of the photo) is foreground of theirs,
    /// 0 to 1.
    pub(super) fn coverage_at(&self, point: [f32; 2]) -> f32 {
        plane_index(self.crop, point)
            .map_or(0.0, |index| f32::from(self.probabilities[index]) / 255.0)
    }
}

/// Runs BiRefNet, with its square input `edge`, on `crop` (fractions of the
/// photo) around `person`, and keeps the foreground of the pixels that are
/// theirs.
pub(super) fn segment(
    session: &mut FallbackSession,
    edge: u32,
    image: &RgbaView<'_>,
    detection: &Detection,
    person: usize,
    crop_fractions: [f32; 4],
) -> Result<PersonForeground> {
    let (photo_width, photo_height) = image.dimensions();
    let crop = crop_pixels(crop_fractions, photo_width, photo_height);
    let row_bytes = crop.width as usize * 4;
    let mut rgba = Vec::with_capacity(row_bytes * crop.height as usize);
    for row in crop.y..crop.y + crop.height {
        let start = (row as usize * photo_width as usize + crop.x as usize) * 4;
        rgba.extend_from_slice(&image.as_raw()[start..start + row_bytes]);
    }
    let cropped =
        RgbaImage::from_raw(crop.width, crop.height, rgba).context("invalid foreground crop")?;
    let mut probabilities = infer_birefnet(session, &cropped, edge, (INPUT_WIDTH, INPUT_HEIGHT))?;
    let owned = ownership(detection, person, crop, photo_width, photo_height);
    for (probability, owned) in probabilities.iter_mut().zip(owned) {
        if !owned {
            *probability = 0;
        }
    }
    Ok(PersonForeground {
        crop: crop_fractions,
        probabilities: probabilities.into_boxed_slice(),
    })
}

/// `parts`' planes with BiRefNet's outline: the entire person is BiRefNet's
/// foreground within `BAND` of Sapiens2's outline and of BiRefNet's own
/// foreground, and each part keeps its share of the person. A pixel BiRefNet
/// adds beyond the outline takes the shares of the nearest pixel Sapiens2
/// found likely. `None` when BiRefNet found something other than the person.
pub(super) fn refine(
    parts: &PersonParts,
    foreground: &PersonForeground,
) -> Option<Box<[[u8; PARTS]]>> {
    let entire = part_index(PersonPart::EntirePerson);
    let person: Vec<u8> = parts
        .probabilities
        .iter()
        .map(|pixel| pixel[entire])
        .collect();
    if !agrees(&person, &foreground.probabilities) {
        return None;
    }

    let (width, height) = (INPUT_WIDTH as usize, INPUT_HEIGHT as usize);
    let band = Band::new(&person, &foreground.probabilities, BAND);
    let (_, likely) = nearest_seeds(width, height, |index| person[index] >= SHARE_FLOOR);

    let refined = (0..INPUT_PIXELS)
        .into_par_iter()
        .map(|index| {
            let (coverage, trust) =
                band.coverage(index, person[index], foreground.probabilities[index]);
            let own = &parts.probabilities[index];
            let mut pixel = [0u8; PARTS];
            let source = match likely[index] {
                NO_SEED => &[0; PARTS],
                source => &parts.probabilities[source as usize],
            };
            let total = f32::from(source[entire]).max(1.0);
            for (part, refined) in pixel.iter_mut().enumerate() {
                let share = if part == entire {
                    1.0
                } else {
                    (f32::from(source[part]) / total).min(1.0)
                };
                let outlined = share * coverage * 255.0;
                *refined = (trust * outlined + (1.0 - trust) * f32::from(own[part]) + 0.5) as u8;
            }
            pixel
        })
        .collect::<Vec<_>>();
    Some(refined.into_boxed_slice())
}

/// The entire person from their foreground alone, for a device without
/// Sapiens2: as `refine` makes it, with RF-DETR-Seg's outline of them in
/// place of Sapiens2's, within `OUTLINE_BAND_CELLS` of it. Their outline
/// itself where BiRefNet found something other than them. A plane over the
/// foreground's crop of a photo `width` x `height` pixels.
pub(super) fn outline(
    found: &PersonForeground,
    detection: &Detection,
    person: usize,
    (photo_width, photo_height): (u32, u32),
) -> Vec<u8> {
    let crop = crop_pixels(found.crop, photo_width, photo_height);
    let owned = ownership(detection, person, crop, photo_width, photo_height);
    let width = INPUT_WIDTH as usize;
    let outline: Vec<u8> = (0..INPUT_PIXELS)
        .into_par_iter()
        .map(|index| {
            if !owned[index] {
                return 0;
            }
            let u = plane_u(crop, photo_width, index % width);
            let v = plane_v(crop, photo_height, index / width);
            (detection.outline_probability(person, u, v) * 255.0 + 0.5) as u8
        })
        .collect();
    if !agrees(&outline, &found.probabilities) {
        return outline;
    }

    // One grid cell in plane pixels, along the axis it spans most of.
    let edge = detection.outline_edge() as f32;
    let cell_width = photo_width as f32 / edge / crop.width as f32 * INPUT_WIDTH as f32;
    let cell_height = photo_height as f32 / edge / crop.height as f32 * INPUT_HEIGHT as f32;
    let width = (OUTLINE_BAND_CELLS * cell_width.max(cell_height)).max(BAND);
    let band = Band::new(&outline, &found.probabilities, width);
    (0..INPUT_PIXELS)
        .into_par_iter()
        .map(|index| {
            let (coverage, trust) =
                band.coverage(index, outline[index], found.probabilities[index]);
            let own = f32::from(outline[index]);
            (trust * coverage * 255.0 + (1.0 - trust) * own + 0.5) as u8
        })
        .collect()
}

/// Whether BiRefNet's `foreground` and a person's plane, both 0 to 255,
/// overlap by at least `MIN_AGREEMENT`: otherwise BiRefNet found something
/// else.
fn agrees(person: &[u8], foreground: &[u8]) -> bool {
    let (mut both, mut either) = (0usize, 0usize);
    for (&person, &foreground) in person.iter().zip(foreground) {
        let (person, foreground) = (person >= 128, foreground >= 128);
        both += usize::from(person && foreground);
        either += usize::from(person || foreground);
    }
    either != 0 && (both as f32) >= MIN_AGREEMENT * either as f32
}

/// Where BiRefNet draws a person's edge: within `width` plane pixels of the
/// outline of their plane, and of its own foreground.
struct Band {
    width: f32,
    /// From inside the outline to the nearest pixel outside it, and back.
    to_outside: Vec<f32>,
    to_inside: Vec<f32>,
    to_found: Vec<f32>,
}

impl Band {
    fn new(person: &[u8], foreground: &[u8], width: f32) -> Self {
        let (columns, rows) = (INPUT_WIDTH as usize, INPUT_HEIGHT as usize);
        let (to_outside, _) = nearest_seeds(columns, rows, |index| person[index] < 128);
        let (to_inside, _) = nearest_seeds(columns, rows, |index| person[index] >= 128);
        let (to_found, _) = nearest_seeds(columns, rows, |index| foreground[index] >= 128);
        Self {
            width,
            to_outside,
            to_inside,
            to_found,
        }
    }

    /// At plane pixel `index`, where the person's plane is `person` and
    /// BiRefNet's `foreground`: the person's coverage by BiRefNet's outline,
    /// 0 to 1, and how far that outline counts there rather than the plane's.
    fn coverage(&self, index: usize, person: u8, foreground: u8) -> (f32, f32) {
        let width = self.width;
        // Positive inside the outline.
        let signed = if person >= 128 {
            self.to_outside[index]
        } else {
            -self.to_inside[index]
        };
        let keep = smoothstep(width * 0.5, width, signed);
        let allow = 1.0 - smoothstep(width * 0.5, width, -signed);
        let coverage = (f32::from(foreground) / 255.0).clamp(keep, allow.max(keep));
        let trust = 1.0 - smoothstep(width * 0.5, width, self.to_found[index]);
        (coverage, trust)
    }
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
