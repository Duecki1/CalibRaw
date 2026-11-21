//! Panorama stitching of frames shot turning one camera about its centre.
//!
//! The pipeline is Brown & Lowe's ("Automatic Panoramic Image Stitching using Invariant
//! Features", IJCV 2007) as OpenCV's stitching module implements it, with details from
//! Hugin and Enblend; each module names its sources. In order:
//!
//! 1. [`PanoramaBuilder::add`] reduces each developed frame to an undistorted luminance
//!    image, finds SIFT keypoints on it (`features`) and keeps a colour thumbnail
//!    (`preview`).
//! 2. [`PanoramaBuilder::solve`] matches keypoints between frames and verifies the pairs
//!    (`matching`), solves every camera's orientation and the focal length (`camera`),
//!    picks the projection and the panorama's extent (`projection`), and on a coarse
//!    grid balances exposure (`exposure`), places the seams (`seams`) and crops to the
//!    largest covered rectangle (`crop`).
//! 3. [`PanoramaLayout::composite`] renders the cropped panorama region by region from
//!    the full-resolution frames: one resampling per pixel through the lens correction,
//!    the camera and the projection, and multi-band blending across the seams (`blend`).
//!
//! Frames are linear camera RGB in sensor coordinates; the result is linear camera RGB
//! in the reference frame's exposure.

mod blend;
mod camera;
mod crop;
mod exposure;
mod features;
mod kdtree;
mod matching;
mod math;
mod maxflow;
mod overview;
mod preview;
mod projection;
mod seams;

pub use projection::{Projection, ProjectionChoice};

use super::alignment::resample;
use super::raw_loader::{MAX_RAW_EDGE, MAX_RAW_PIXELS};
use super::{LensGeometryMap, PerspectiveCorrection};
use anyhow::{ensure, Context, Result};
use camera::{Camera, Pinhole};
use matching::FrameFeatures;
use preview::{fill_invalid, Reduced, SourceFrame};
use projection::{frame_extent, Extent, Grid};
use rayon::prelude::*;
use seams::Seams;
use std::sync::Arc;

/// The seam grid's resolution relative to the output's: Enblend's default
/// `--coarse-mask` factor. Finer than OpenCV's fixed 0.1 MP, so the cut sees the few
/// pixels by which parallax shifts near objects between frames.
const SEAM_REDUCTION: f64 = 8.0;
/// Multi-band blend strength in percent of the panorama's size (OpenCV's default).
const BLEND_STRENGTH: f64 = 5.0;
/// Detail levels at most. Exposure is balanced before blending, so wider bands would
/// only grow the halo every region renders.
const MAX_LEVELS: usize = 7;
/// A frame's blend weight outside its seam, so a pixel some frame covers is never left
/// without weight where the seam grid assigns it to a frame that ends just short of it.
const OFF_SEAM_WEIGHT: f32 = 1e-4;
/// Bytes of working memory per region pixel: the blend pyramids, one frame's pyramid
/// with its temporaries and its sampling positions.
const BYTES_PER_REGION_PIXEL: u64 = 112;
/// Region core sizes tried, largest first.
const TILE_SIZES: [usize; 4] = [2048, 1024, 512, 256];
/// Frames are box-filtered before resampling once the panorama shows them at less than
/// `1 / DOWNSCALE_PREFILTER` of their resolution.
const DOWNSCALE_PREFILTER: f64 = 1.5;

/// One developed frame handed to [`PanoramaBuilder::add`].
pub struct PanoramaFrame<'a> {
    /// Names the frame in errors (its file name).
    pub label: &'a str,
    pub width: u32,
    pub height: u32,
    /// Black-subtracted, white-normalized camera RGB without white balance, in sensor
    /// coordinates, row-major.
    pub rgb: &'a [f32],
    /// Per pixel, the brightest sensor sample of its demosaic footprint (white = 1).
    pub peak: &'a [f32],
    /// The frame's distortion correction (undistorted → sensor positions), when the lens
    /// is corrected.
    pub lens: Option<Arc<LensGeometryMap>>,
    /// Exposure relative to the reference frame.
    pub exposure: f32,
    /// The 35 mm equivalent focal length from metadata, when known.
    pub focal_length_35mm: Option<f32>,
}

/// The frames' full-resolution pixels, read back while compositing.
pub trait FrameStore: Sync {
    /// Writes the camera RGB of `rect` of frame `frame` (sensor coordinates) into `out`,
    /// row-major, replacing its contents.
    fn read(&self, frame: usize, rect: PixelRect, out: &mut Vec<f32>) -> Result<()>;
}

/// A rectangle of pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// What the solver keeps of a frame.
struct Frame {
    label: String,
    width: usize,
    height: usize,
    /// Initial focal length in full-resolution pixels, when the metadata gives it.
    focal: Option<f64>,
    exposure: f32,
    lens: Option<Arc<LensGeometryMap>>,
    thumbnail: Reduced,
}

/// Collects the frames of a panorama.
#[derive(Default)]
pub struct PanoramaBuilder {
    frames: Vec<Frame>,
    /// Each frame's keypoints, in the order of `frames`.
    features: Vec<FrameFeatures>,
}

impl PanoramaBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Finds the frame's keypoints and keeps its thumbnail; `frame`'s pixels are not kept.
    pub fn add(&mut self, frame: PanoramaFrame) -> Result<()> {
        let (width, height) = (frame.width as usize, frame.height as usize);
        ensure!(
            frame.rgb.len() == width * height * 3 && frame.peak.len() == width * height,
            "Panorama frame dimensions do not match its buffers"
        );
        ensure!(
            frame.exposure.is_finite() && frame.exposure > 0.0,
            "Invalid panorama frame exposure"
        );
        let source = SourceFrame {
            width,
            height,
            rgb: frame.rgb,
            peak: frame.peak,
            lens: frame.lens.as_deref(),
            exposure: frame.exposure,
        };
        let registration = source.registration();
        // Relative to the frame's bright level, as an 8-bit photo's white is, which
        // SIFT's contrast threshold assumes; descriptors do not depend on the scale.
        let mut gray = features::Gray {
            width: registration.width,
            height: registration.height,
            data: registration
                .rgb
                .iter()
                .map(|&rgb| {
                    (exposure::luminance(rgb) / registration.level)
                        .max(0.0)
                        .cbrt()
                })
                .collect(),
        };
        fill_invalid(
            gray.width,
            gray.height,
            &mut gray.data,
            &registration.measured,
        );
        let (keypoints, descriptors) = features::detect(&gray, &registration.measured);
        let points = keypoints
            .iter()
            .map(|k| {
                [
                    registration.full_position(f64::from(k.x)),
                    registration.full_position(f64::from(k.y)),
                ]
            })
            .collect();
        let measured = registration.measured.iter().filter(|&&m| m).count();
        crate::diagnostics::record(format!(
            "Panorama: {} keypoints in {} ({:.0} % of it measured)",
            keypoints.len(),
            frame.label,
            100.0 * measured as f64 / registration.measured.len().max(1) as f64
        ));
        let focal = frame
            .focal_length_35mm
            .filter(|f| f.is_finite() && PerspectiveCorrection::FOCAL_LENGTH_RANGE.contains(f))
            .map(|focal_35mm| focal_from_35mm(focal_35mm, width, height));
        let thumbnail = source.thumbnail();
        self.features.push(FrameFeatures {
            points,
            descriptors,
            pixel: registration.factor as f64,
        });
        self.frames.push(Frame {
            label: frame.label.to_owned(),
            width,
            height,
            focal,
            exposure: frame.exposure,
            lens: frame.lens,
            thumbnail,
        });
        Ok(())
    }

    /// Registers the frames and lays out the panorama in `projection`.
    pub fn solve(self, projection: ProjectionChoice) -> Result<PanoramaLayout> {
        let Self { frames, features } = self;
        ensure!(frames.len() >= 2, "Select at least two RAW frames");
        let labels: Vec<String> = frames.iter().map(|f| f.label.clone()).collect();
        let putative = matching::match_frames(&features);
        let (pinholes, lens, pairs) = verify_pairs(&frames, &features, &putative);
        crate::diagnostics::record(format!(
            "Panorama: {} of {} frame pairs verified",
            pairs.len(),
            putative.len()
        ));
        let cameras = camera::solve(&pinholes, lens, &pairs, &labels)?;
        let projection = projection.resolve(&cameras);
        let extents = cameras
            .iter()
            .zip(&labels)
            .map(|(camera, label)| frame_extent(projection, camera, label))
            .collect::<Result<Vec<Extent>>>()?;
        let total = extents
            .iter()
            .copied()
            .reduce(Extent::union)
            .context("Panorama has no frames")?;
        let area = total.size()[0] * total.size()[1];
        let thumbnail_scale = cameras
            .iter()
            .zip(&frames)
            .map(|(c, f)| c.pinhole.focal / f.thumbnail.factor as f64)
            .fold(f64::INFINITY, f64::min);
        // The output's scale before any reduction to MAX_RAW_PIXELS, which bounds the grid.
        let full_scale = median(cameras.iter().map(|c| c.pinhole.focal).collect());
        let seam_scale = (full_scale / SEAM_REDUCTION)
            .min((MAX_RAW_PIXELS as f64 / area).sqrt() / SEAM_REDUCTION)
            .min(thumbnail_scale);
        let seam_grid = Grid::covering(projection, total, seam_scale);
        let overviews: Vec<overview::Overview> = cameras
            .iter()
            .zip(&frames)
            .zip(&extents)
            .map(|((camera, frame), &extent)| {
                overview::warp(&seam_grid, camera, &frame.thumbnail, extent)
            })
            .collect();
        let gains = exposure::balance(&overviews);
        let crop = crop_extent(&seam_grid, &overviews)?;
        let output = output_grid(projection, crop, &cameras);
        crate::diagnostics::record(format!(
            "Panorama: {projection:?}, {} × {} px at {:.3} of the source resolution",
            output.width,
            output.height,
            output.scale / median(cameras.iter().map(|c| c.pinhole.focal).collect())
        ));
        let levels = blend_levels(output.width, output.height);
        let seams = seams::find(
            seam_grid,
            &overviews,
            &gains,
            (1u64 << levels) as f64,
            output.scale,
        );
        Ok(PanoramaLayout {
            frames: frames
                .into_iter()
                .zip(cameras)
                .zip(extents)
                .zip(gains)
                .map(|(((frame, camera), extent), gain)| FrameLayout {
                    width: frame.width,
                    height: frame.height,
                    camera,
                    lens: frame.lens,
                    // Values are divided by this to reach the reference exposure.
                    normalization: frame.exposure / gain as f32,
                    extent,
                })
                .collect(),
            output,
            seams,
            levels,
        })
    }
}

/// Focal length in pixels of a frame whose 35 mm equivalent focal length is `focal_35mm`.
fn focal_from_35mm(focal_35mm: f32, width: usize, height: usize) -> f64 {
    PerspectiveCorrection::focal_length_pixels(focal_35mm, [width as f64, height as f64])
}

/// The frames' starting cameras, what is known of their lens, and the verified pairs.
/// With every frame's focal length known from metadata, pairs are verified with the
/// rotation model; otherwise with homographies, from which the focal length is estimated
/// (or, failing that, taken as the generic one perspective correction assumes).
fn verify_pairs(
    frames: &[Frame],
    features: &[FrameFeatures],
    putative: &[matching::Putative],
) -> (Vec<Pinhole>, camera::Lens, Vec<matching::PairMatches>) {
    let corrected = frames.iter().any(|f| f.lens.is_some());
    let cameras = |focals: &[f64]| -> Vec<Pinhole> {
        frames
            .iter()
            .zip(focals)
            .map(|(f, &focal)| Pinhole::new(f.width, f.height, focal))
            .collect()
    };
    let known: Option<Vec<f64>> = frames.iter().map(|f| f.focal).collect();
    if let Some(focals) = known {
        let first = focals[0];
        let shared = focals.iter().all(|f| (f / first - 1.0).abs() < 0.005);
        let focals = if shared {
            vec![first; frames.len()]
        } else {
            focals
        };
        let pinholes = cameras(&focals);
        let pairs = matching::verify_rotations(features, &pinholes, putative);
        let lens = camera::Lens {
            shared_focal: shared,
            focal_from_metadata: true,
            corrected,
        };
        return (pinholes, lens, pairs);
    }
    let sizes: Vec<(usize, usize)> = frames.iter().map(|f| (f.width, f.height)).collect();
    let verified = matching::verify_homographies(features, &sizes, putative);
    let focal = match matching::estimate_focal(&verified, frames.len()) {
        Some(focal) => {
            crate::diagnostics::record(format!(
                "Panorama: focal length estimated from the matches: {focal:.1} px"
            ));
            focal
        }
        None => {
            crate::diagnostics::record(
                "Panorama: focal length unknown and not estimable; assuming a 28 mm equivalent",
            );
            focal_from_35mm(
                PerspectiveCorrection::GENERIC_FOCAL_LENGTH_35MM,
                frames[0].width,
                frames[0].height,
            )
        }
    };
    let pinholes = cameras(&vec![focal; frames.len()]);
    let pairs = matching::rotations_from_inliers(verified, &pinholes);
    let lens = camera::Lens {
        shared_focal: true,
        focal_from_metadata: false,
        corrected,
    };
    (pinholes, lens, pairs)
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values.get(values.len() / 2).copied().unwrap_or(1.0)
}

/// The largest rectangle every full-resolution pixel of which some frame covers, on the
/// panorama plane.
fn crop_extent(grid: &Grid, overviews: &[overview::Overview]) -> Result<Extent> {
    let (w, h) = (grid.width, grid.height);
    let covered: Vec<bool> = (0..w * h)
        .into_par_iter()
        .map(|i| overviews.iter().any(|o| o.covers(i % w, i / w)))
        .collect();
    let inside = crop::erode(w, h, &covered);
    let rect = crop::largest_rectangle(w, h, &inside)
        .context("The frames do not overlap enough to form a panorama")?;
    Ok(Extent {
        min: [
            grid.origin[0] + rect[0] as f64 / grid.scale,
            grid.origin[1] + rect[1] as f64 / grid.scale,
        ],
        max: [
            grid.origin[0] + rect[2] as f64 / grid.scale,
            grid.origin[1] + rect[3] as f64 / grid.scale,
        ],
    })
}

/// The output grid over `crop`: the frames' resolution at the centre (OpenCV's
/// `warped_image_scale`, the median focal length), reduced uniformly when the panorama
/// would exceed what CalibRaw opens.
fn output_grid(projection: projection::Projection, crop: Extent, cameras: &[Camera]) -> Grid {
    let mut scale = median(cameras.iter().map(|c| c.pinhole.focal).collect());
    let size = crop.size();
    let (width, height) = (size[0] * scale, size[1] * scale);
    let edge = f64::from(MAX_RAW_EDGE);
    let limit = (edge / width)
        .min(edge / height)
        .min((MAX_RAW_PIXELS as f64 / (width * height)).sqrt())
        .min(1.0);
    scale *= limit;
    Grid {
        projection,
        origin: crop.min,
        scale,
        width: ((size[0] * scale).floor() as usize).clamp(1, MAX_RAW_EDGE as usize),
        height: ((size[1] * scale).floor() as usize).clamp(1, MAX_RAW_EDGE as usize),
    }
}

/// OpenCV's band count for a blend strength of `BLEND_STRENGTH` percent:
/// `ceil(log2(√area · strength / 100)) − 1`, here at least 1 and at most `MAX_LEVELS`.
fn blend_levels(width: usize, height: usize) -> usize {
    let blend_width = ((width * height) as f64).sqrt() * BLEND_STRENGTH / 100.0;
    if blend_width < 2.0 {
        return 1;
    }
    ((blend_width.log2().ceil() as isize - 1).max(1) as usize).min(MAX_LEVELS)
}

/// A frame's place in the panorama.
struct FrameLayout {
    width: usize,
    height: usize,
    camera: Camera,
    lens: Option<Arc<LensGeometryMap>>,
    /// Exposure relative to the reference divided by the balancing gain.
    normalization: f32,
    extent: Extent,
}

/// A solved panorama, ready to composite.
pub struct PanoramaLayout {
    frames: Vec<FrameLayout>,
    output: Grid,
    seams: Seams,
    levels: usize,
}

impl PanoramaLayout {
    pub fn width(&self) -> u32 {
        self.output.width as u32
    }

    pub fn height(&self) -> u32 {
        self.output.height as u32
    }

    /// Whether the panorama had to be reduced below the frames' resolution to stay
    /// within what CalibRaw opens.
    pub fn reduced(&self) -> bool {
        let focal = median(self.frames.iter().map(|f| f.camera.pinhole.focal).collect());
        self.output.scale < focal * 0.999
    }

    fn halo(&self) -> usize {
        // OpenCV grows a multi-band region by `3 · 2^bands` for the same reason.
        3 << self.levels
    }

    fn tile_size(&self, memory_bytes: u64) -> usize {
        let unit = 1usize << self.levels;
        let rows_needed = self.output.height.div_ceil(unit) * unit;
        let fits = |tile: usize| {
            let region = (tile + 2 * self.halo()) as u64;
            region * region * BYTES_PER_REGION_PIXEL + (self.output.width * tile * 12) as u64
                <= memory_bytes
        };
        TILE_SIZES
            .iter()
            .map(|&t| t.max(unit).min(rows_needed.max(unit)))
            .find(|&t| fits(t))
            .unwrap_or_else(|| TILE_SIZES[TILE_SIZES.len() - 1].max(unit))
    }

    /// Renders the panorama top to bottom in strips as tall as the regions that fit
    /// `memory_bytes` of working memory, and hands each to `sink` as linear camera RGB
    /// (reference exposure, row-major, `width` pixels per row). `progress` reports and
    /// returns false to cancel.
    pub fn composite(
        &self,
        store: &dyn FrameStore,
        memory_bytes: u64,
        mut sink: impl FnMut(&[f32]) -> Result<()>,
        mut progress: impl FnMut(String) -> bool,
    ) -> Result<()> {
        let tile = self.tile_size(memory_bytes);
        let halo = self.halo();
        let region = tile + 2 * halo;
        let (width, height) = (self.output.width, self.output.height);
        let columns = width.div_ceil(tile);
        let rows = height.div_ceil(tile);
        let mut strip = Vec::new();
        for row in 0..rows {
            let strip_height = tile.min(height - row * tile);
            strip.clear();
            strip.resize(width * strip_height * 3, 0.0f32);
            for column in 0..columns {
                ensure!(
                    progress(format!(
                        "Blending part {}/{}",
                        row * columns + column + 1,
                        rows * columns
                    )),
                    "Merge cancelled"
                );
                let origin = [
                    (column * tile) as isize - halo as isize,
                    (row * tile) as isize - halo as isize,
                ];
                let blended = self.render_region(store, origin, region)?;
                let core_width = tile.min(width - column * tile);
                for y in 0..strip_height {
                    let source = (y + halo) * region + halo;
                    let destination = (y * width + column * tile) * 3;
                    for (out, value) in strip[destination..destination + core_width * 3]
                        .chunks_exact_mut(3)
                        .zip(&blended[source..source + core_width])
                    {
                        out.copy_from_slice(value);
                    }
                }
            }
            sink(&strip)?;
        }
        Ok(())
    }

    /// Blends the `size × size` output region whose top-left pixel is `origin` (which may
    /// lie outside the output: frames extend beyond the crop).
    fn render_region(
        &self,
        store: &dyn FrameStore,
        origin: [isize; 2],
        size: usize,
    ) -> Result<Vec<[f32; 3]>> {
        let mut blend = blend::RegionBlend::new(size, self.levels);
        let bounds = Extent {
            min: self
                .output
                .plane(origin[0] as f64 - 0.5, origin[1] as f64 - 0.5),
            max: self.output.plane(
                (origin[0] + size as isize) as f64 - 0.5,
                (origin[1] + size as isize) as f64 - 0.5,
            ),
        };
        for (index, frame) in self.frames.iter().enumerate() {
            let overlaps = frame.extent.min[0] <= bounds.max[0]
                && frame.extent.max[0] >= bounds.min[0]
                && frame.extent.min[1] <= bounds.max[1]
                && frame.extent.max[1] >= bounds.min[1];
            if !overlaps {
                continue;
            }
            if let Some(warped) = self.warp_frame(store, index, origin, size)? {
                blend.add(warped.values, warped.coverage, warped.weight);
            }
        }
        Ok(blend.finish())
    }

    /// Frame `index` resampled onto the output region at `origin`, encoded for blending,
    /// with its coverage and blend weight; `None` where it does not reach the region.
    fn warp_frame(
        &self,
        store: &dyn FrameStore,
        index: usize,
        origin: [isize; 2],
        size: usize,
    ) -> Result<Option<WarpedRegion>> {
        let frame = &self.frames[index];
        let camera = &frame.camera;
        let (fw, fh) = (frame.width as u32, frame.height as u32);
        // Sensor position of every region pixel the frame sees.
        let positions: Vec<Option<[f32; 2]>> = (0..size * size)
            .into_par_iter()
            .map(|i| {
                let x = (origin[0] + (i % size) as isize) as f64;
                let y = (origin[1] + (i / size) as isize) as f64;
                let p = self
                    .output
                    .ray(x, y)
                    .and_then(|ray| camera.pixel(ray))
                    .filter(|&p| camera.pinhole.contains(p))?;
                Some(match &frame.lens {
                    Some(map) => map.source_position_for_raster(p[0] as f32, p[1] as f32, fw, fh),
                    None => [p[0] as f32, p[1] as f32],
                })
            })
            .collect();
        let Some(bounds) = positions
            .par_iter()
            .flatten()
            .fold(
                || None,
                |b: Option<[f32; 4]>, p| {
                    Some(b.map_or([p[0], p[1], p[0], p[1]], |b| {
                        [
                            b[0].min(p[0]),
                            b[1].min(p[1]),
                            b[2].max(p[0]),
                            b[3].max(p[1]),
                        ]
                    }))
                },
            )
            .reduce(
                || None,
                |a, b| match (a, b) {
                    (Some(a), Some(b)) => Some([
                        a[0].min(b[0]),
                        a[1].min(b[1]),
                        a[2].max(b[2]),
                        a[3].max(b[3]),
                    ]),
                    (a, None) => a,
                    (None, b) => b,
                },
            )
        else {
            return Ok(None);
        };
        // Box-filter frames the panorama shows reduced, so they do not alias.
        let ratio = camera.pinhole.focal / self.output.scale;
        let factor = if ratio >= DOWNSCALE_PREFILTER {
            ratio.round() as u32
        } else {
            1
        };
        // Catmull–Rom reaches two samples beyond the position.
        let margin = 2 * factor + 1;
        let clamp = |v: f32, size: u32| (v.max(0.0) as u32).min(size - 1);
        let x0 = clamp(bounds[0].floor(), fw).saturating_sub(margin) / factor * factor;
        let y0 = clamp(bounds[1].floor(), fh).saturating_sub(margin) / factor * factor;
        let x1 = (clamp(bounds[2].ceil(), fw) + margin + 1).min(fw);
        let y1 = (clamp(bounds[3].ceil(), fh) + margin + 1).min(fh);
        let rect = PixelRect {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
        };
        let mut pixels = Vec::new();
        store.read(index, rect, &mut pixels)?;
        let (source, source_width, source_height) = box_reduce(
            pixels,
            rect.width as usize,
            rect.height as usize,
            factor as usize,
        );
        let half = 0.5 * (factor as f32 - 1.0);
        let warped = resample(
            (source_width, source_height),
            (size, size),
            &source,
            None,
            |x, y| match positions[y as usize * size + x as usize] {
                Some(p) => (
                    (p[0] - x0 as f32 - half) / factor as f32,
                    (p[1] - y0 as f32 - half) / factor as f32,
                ),
                None => (f32::NAN, f32::NAN),
            },
        );
        let normalization = frame.normalization;
        let seam_grid = &self.seams.grid;
        let label = index as u16;
        let values = warped
            .rgb
            .into_par_iter()
            .zip(&positions)
            .map(|(rgb, position)| match position {
                Some(_) => blend::encode(rgb.map(|v| v / normalization)),
                None => [0.0; 3],
            })
            .collect();
        let coverage = positions
            .par_iter()
            .map(|position| f32::from(u8::from(position.is_some())))
            .collect();
        let weight = positions
            .par_iter()
            .enumerate()
            .map(|(i, position)| {
                if position.is_none() {
                    return 0.0;
                }
                let x = (origin[0] + (i % size) as isize) as f64;
                let y = (origin[1] + (i / size) as isize) as f64;
                let seam = seam_grid.position(self.output.plane(x, y));
                let owned = self.seams.owner(seam[0], seam[1]) == label;
                f32::from(u8::from(owned)) + OFF_SEAM_WEIGHT
            })
            .collect();
        Ok(Some(WarpedRegion {
            values,
            coverage,
            weight,
        }))
    }
}

/// A frame resampled onto an output region.
struct WarpedRegion {
    values: Vec<[f32; 3]>,
    coverage: Vec<f32>,
    weight: Vec<f32>,
}

/// Averages `factor × factor` blocks of a `width × height` RGB image (edge blocks average
/// what they hold). Returns the image and its size.
fn box_reduce(
    rgb: Vec<f32>,
    width: usize,
    height: usize,
    factor: usize,
) -> (Vec<f32>, usize, usize) {
    if factor <= 1 {
        return (rgb, width, height);
    }
    let (w, h) = (width.div_ceil(factor), height.div_ceil(factor));
    let mut out = vec![0.0f32; w * h * 3];
    out.par_chunks_mut(w * 3).enumerate().for_each(|(by, row)| {
        let mut counts = vec![0u32; w];
        for y in by * factor..((by + 1) * factor).min(height) {
            for x in 0..width {
                let bx = x / factor;
                for c in 0..3 {
                    row[bx * 3 + c] += rgb[(y * width + x) * 3 + c];
                }
                counts[bx] += 1;
            }
        }
        for (pixel, count) in row.chunks_exact_mut(3).zip(counts) {
            pixel.iter_mut().for_each(|v| *v /= count.max(1) as f32);
        }
    });
    (out, w, h)
}
