//! The analysis kept beside a photo (`<RAW>.calibraw-people`), so opening
//! it again neither finds nor segments anyone. A zip of a JSON manifest, the
//! detection outlines, each segmented person's part planes and the
//! foreground of those BiRefNet ran on: boxes, outlines and probabilities,
//! never pixels of the photo. A file of another
//! version, or of other content than the photo has now, is ignored, and the
//! next analysis replaces it.

use super::detect::{Candidate, Detection};
use super::foreground::PersonForeground;
use super::parts::{PersonParts, INPUT_HEIGHT, INPUT_PIXELS, INPUT_WIDTH, PARTS};
use super::PeopleAnalysis;
use crate::pipeline::{CfaKind, LoadedRaw, RemoveEditState};
use anyhow::{Context, Result};
use calibraw_core::file_ops::write_atomically;
use ring::digest::{Context as Sha256Context, SHA256};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Seek, Write},
    path::Path,
    sync::Arc,
};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

const FORMAT: &str = "CalibRaw people analysis";
/// Bump for every change to the layout or to what an analysis holds; files
/// of another version are made again.
const VERSION: u32 = 3;
const MANIFEST: &str = "manifest.json";
/// Every candidate's outline logits in candidate order, `f32` little-endian.
const OUTLINES: &str = "outlines.f32";
const MAX_MANIFEST_BYTES: u64 = 1 << 20;
/// RF-DETR-Seg has 200 queries.
const MAX_CANDIDATES: usize = 256;
/// With `MAX_CANDIDATES`, bounds the outlines to 64 MiB. RF-DETR-Seg's grid
/// edge is 108.
const MAX_OUTLINE_EDGE: usize = 256;

/// One person's part planes: `PARTS` bytes per pixel, by row.
fn parts_entry(person: usize) -> String {
    format!("person-{person}.parts")
}

/// One person's foreground: a byte per pixel of the part planes, by row.
fn foreground_entry(person: usize) -> String {
    format!("person-{person}.foreground")
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    /// The `ContentFingerprint` of what was analysed, in hex.
    content: String,
    /// Width over height of the photo analysed.
    aspect: f32,
    outline_edge: usize,
    /// How many `candidates`, from the first, are offered as people.
    people: usize,
    candidates: Vec<StoredCandidate>,
    /// By person, the crop their part planes cover once they are segmented.
    crops: Vec<Option<[f32; 4]>>,
    /// By person, the crop their foreground covers once BiRefNet found it;
    /// the same as their part planes' when both are known.
    foreground_crops: Vec<Option<[f32; 4]>>,
    /// Width and height of every part plane, and parts per pixel.
    planes: [usize; 3],
}

#[derive(Serialize, Deserialize)]
struct StoredCandidate {
    score: f32,
    bounds: [f32; 4],
}

const PLANES: [usize; 3] = [INPUT_WIDTH as usize, INPUT_HEIGHT as usize, PARTS];

/// Identifies the content a people analysis is of ([`content_fingerprint`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentFingerprint([u8; 32]);

/// What people sources are rendered from: the RAW as loaded, lens
/// correction included, and the Remove strokes over it. A stroke counts by
/// its brush, retouch, opacity and patch bounds; its patches only repeat
/// what the brush made.
pub fn content_fingerprint(
    raw: &LoadedRaw,
    remove: &RemoveEditState,
) -> Result<ContentFingerprint> {
    let mut digest = DigestWriter(Sha256Context::new(&SHA256));
    digest.0.update(b"CalibRaw people content v1\0");
    digest.0.update(&raw.width.to_le_bytes());
    digest.0.update(&raw.height.to_le_bytes());
    digest.0.update(match raw.cfa_kind {
        CfaKind::Bayer => b"bayer\0",
        CfaKind::XTrans => b"xtrans",
    });
    digest.0.update(bytemuck::cast_slice(&raw.raw_pixels));
    for stroke in &remove.strokes {
        digest.0.update(b"\0stroke\0");
        serde_json::to_writer(&mut digest, &stroke.brush)?;
        serde_json::to_writer(&mut digest, &stroke.retouch)?;
        digest.0.update(&stroke.opacity.to_le_bytes());
        for patch in &stroke.patches {
            serde_json::to_writer(&mut digest, &patch.bounds)?;
        }
    }
    let bytes = digest.0.finish();
    Ok(ContentFingerprint(
        bytes.as_ref().try_into().context("SHA-256 is 32 bytes")?,
    ))
}

struct DigestWriter(Sha256Context);

impl Write for DigestWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl PeopleAnalysis {
    /// Keeps this analysis of `content` at `path`, replacing what is there.
    pub fn save(&self, path: &Path, content: ContentFingerprint) -> Result<()> {
        let (candidates, people, outline_edge) = self.detection.parts();
        let manifest = serde_json::to_vec(&Manifest {
            format: FORMAT.to_owned(),
            version: VERSION,
            content: hex::encode(content.0),
            aspect: self.aspect,
            outline_edge,
            people,
            candidates: candidates
                .iter()
                .map(|candidate| StoredCandidate {
                    score: candidate.score,
                    bounds: candidate.bounds,
                })
                .collect(),
            crops: self
                .parts
                .iter()
                .map(|parts| parts.as_ref().map(|parts| parts.crop))
                .collect(),
            foreground_crops: self
                .foregrounds
                .iter()
                .map(|found| found.as_ref().map(|found| found.crop))
                .collect(),
            planes: PLANES,
        })
        .context("encode the people analysis manifest")?;
        write_atomically(path, |file| -> Result<()> {
            let mut archive = ZipWriter::new(file);
            let compressed = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .compression_level(Some(3));
            archive.start_file(MANIFEST, compressed)?;
            archive.write_all(&manifest)?;
            archive.start_file(OUTLINES, compressed)?;
            for candidate in candidates {
                let bytes: Vec<u8> = candidate
                    .outline
                    .iter()
                    .flat_map(|logit| logit.to_le_bytes())
                    .collect();
                archive.write_all(&bytes)?;
            }
            for (person, parts) in self.parts.iter().enumerate() {
                if let Some(parts) = parts {
                    archive.start_file(parts_entry(person), compressed)?;
                    archive.write_all(bytemuck::cast_slice(&parts.probabilities))?;
                }
            }
            for (person, found) in self.foregrounds.iter().enumerate() {
                if let Some(found) = found {
                    archive.start_file(foreground_entry(person), compressed)?;
                    archive.write_all(&found.probabilities)?;
                }
            }
            archive.finish()?;
            Ok(())
        })
        .with_context(|| format!("write the people analysis {}", path.display()))
    }

    /// The analysis of `content` kept at `path`. `None` when there is none,
    /// or it is of another version or other content.
    pub fn load(path: &Path, content: ContentFingerprint) -> Result<Option<Self>> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).with_context(|| format!("open {}", path.display())),
        };
        let mut archive = ZipArchive::new(file)
            .with_context(|| format!("read the people analysis {}", path.display()))?;
        let manifest = {
            let mut entry = archive
                .by_name(MANIFEST)
                .context("the people analysis has no manifest")?;
            let mut bytes = Vec::new();
            (&mut entry)
                .take(MAX_MANIFEST_BYTES + 1)
                .read_to_end(&mut bytes)?;
            anyhow::ensure!(
                bytes.len() as u64 <= MAX_MANIFEST_BYTES,
                "the people analysis manifest is too large"
            );
            serde_json::from_slice::<Manifest>(&bytes)
                .context("read the people analysis manifest")?
        };
        anyhow::ensure!(manifest.format == FORMAT, "not a people analysis");
        if manifest.version != VERSION || manifest.content != hex::encode(content.0) {
            return Ok(None);
        }
        anyhow::ensure!(
            manifest.planes == PLANES,
            "the people analysis has {:?} part planes",
            manifest.planes
        );
        anyhow::ensure!(
            manifest.aspect.is_finite() && manifest.aspect > 0.0,
            "invalid photo aspect"
        );
        anyhow::ensure!(
            manifest.candidates.len() <= MAX_CANDIDATES
                && (1..=MAX_OUTLINE_EDGE).contains(&manifest.outline_edge)
                && manifest.crops.len() == manifest.people
                && manifest.foreground_crops.len() == manifest.people,
            "the people analysis manifest is inconsistent"
        );

        let outline_len = manifest.outline_edge * manifest.outline_edge;
        let mut outline_bytes = vec![0u8; manifest.candidates.len() * outline_len * 4];
        read_entry(&mut archive, OUTLINES, &mut outline_bytes)?;
        let candidates = manifest
            .candidates
            .iter()
            .zip(outline_bytes.chunks_exact(outline_len * 4))
            .map(|(candidate, outline)| Candidate {
                score: candidate.score,
                bounds: candidate.bounds,
                outline: outline
                    .chunks_exact(4)
                    .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
                    .collect(),
            })
            .collect();
        let detection = Detection::from_parts(candidates, manifest.people, manifest.outline_edge)?;

        let parts = manifest
            .crops
            .iter()
            .enumerate()
            .map(|(person, crop)| {
                let Some(crop) = *crop else {
                    return Ok(None);
                };
                ensure_crop(crop, person)?;
                let mut probabilities = vec![[0u8; PARTS]; INPUT_PIXELS].into_boxed_slice();
                read_entry(
                    &mut archive,
                    &parts_entry(person),
                    bytemuck::cast_slice_mut(&mut probabilities),
                )?;
                Ok(Some(Arc::new(PersonParts::new(crop, probabilities))))
            })
            .collect::<Result<Vec<_>>>()?;
        let foregrounds = manifest
            .foreground_crops
            .iter()
            .enumerate()
            .map(|(person, crop)| {
                let Some(crop) = *crop else {
                    return Ok(None);
                };
                ensure_crop(crop, person)?;
                anyhow::ensure!(
                    manifest.crops[person].is_none_or(|parts_crop| parts_crop == crop),
                    "the foreground of person {person} is not over their part planes"
                );
                let mut probabilities = vec![0u8; INPUT_PIXELS].into_boxed_slice();
                read_entry(&mut archive, &foreground_entry(person), &mut probabilities)?;
                Ok(Some(Arc::new(PersonForeground {
                    crop,
                    probabilities,
                })))
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Some(Self {
            detection: Arc::new(detection),
            aspect: manifest.aspect,
            parts,
            foregrounds,
        }))
    }
}

/// Checks a person's crop (left, top, right, bottom) read from a file.
fn ensure_crop(crop: [f32; 4], person: usize) -> Result<()> {
    let [left, top, right, bottom] = crop;
    anyhow::ensure!(
        crop.iter().all(|value| (0.0..=1.0).contains(value)) && left < right && top < bottom,
        "invalid crop of person {person}"
    );
    Ok(())
}

/// Fills `buffer` with the entry `name`, which must be exactly as long.
fn read_entry(
    archive: &mut ZipArchive<impl Read + Seek>,
    name: &str,
    buffer: &mut [u8],
) -> Result<()> {
    let mut entry = archive
        .by_name(name)
        .with_context(|| format!("the people analysis has no {name}"))?;
    anyhow::ensure!(
        entry.size() == buffer.len() as u64,
        "the people analysis {name} has an unexpected size"
    );
    entry
        .read_exact(buffer)
        .with_context(|| format!("read the people analysis {name}"))?;
    let mut trailing = [0u8; 1];
    anyhow::ensure!(
        entry.read(&mut trailing)? == 0,
        "the people analysis {name} has trailing data"
    );
    Ok(())
}
