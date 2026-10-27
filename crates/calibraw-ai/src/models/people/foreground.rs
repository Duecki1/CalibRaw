//! High-quality people masks: BiRefNet's foreground over each person's crop,
//! whose fine outline replaces Sapiens2's soft one near its edge, while
//! Sapiens2 still decides which part each pixel is.

use super::detect::Detection;
use super::distance::{nearest_seeds, NO_SEED};
use super::parts::{
    crop_pixels, ownership, part_index, PersonParts, INPUT_HEIGHT, INPUT_PIXELS, INPUT_WIDTH, PARTS,
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
    let (mut both, mut either) = (0usize, 0usize);
    for (&person, &foreground) in person.iter().zip(foreground.probabilities.iter()) {
        let (person, foreground) = (person >= 128, foreground >= 128);
        both += usize::from(person && foreground);
        either += usize::from(person || foreground);
    }
    if either == 0 || (both as f32) < MIN_AGREEMENT * either as f32 {
        return None;
    }

    let (width, height) = (INPUT_WIDTH as usize, INPUT_HEIGHT as usize);
    // From inside the outline to the nearest pixel outside it, and back.
    let (to_outside, _) = nearest_seeds(width, height, |index| person[index] < 128);
    let (to_inside, _) = nearest_seeds(width, height, |index| person[index] >= 128);
    let (_, likely) = nearest_seeds(width, height, |index| person[index] >= SHARE_FLOOR);
    let (to_found, _) = nearest_seeds(width, height, |index| {
        foreground.probabilities[index] >= 128
    });

    let refined = (0..INPUT_PIXELS)
        .into_par_iter()
        .map(|index| {
            // Positive inside the outline.
            let signed = if person[index] >= 128 {
                to_outside[index]
            } else {
                -to_inside[index]
            };
            let keep = smoothstep(BAND * 0.5, BAND, signed);
            let allow = 1.0 - smoothstep(BAND * 0.5, BAND, -signed);
            let coverage =
                (f32::from(foreground.probabilities[index]) / 255.0).clamp(keep, allow.max(keep));
            // How far BiRefNet's outline counts here; Sapiens2's planes the rest.
            let trust = 1.0 - smoothstep(BAND * 0.5, BAND, to_found[index]);
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

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
