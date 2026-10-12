//! Exemplar-based hole filling (content-aware fill).
//!
//! Fills a hole with pixels copied from the rest of the image, so that every
//! patch in and around the hole closely resembles some patch outside it. The
//! scheme is the multi-scale expectation-maximisation of Wexler, Shechtman and
//! Irani, "Space-Time Completion of Video" (2007), with the PatchMatch
//! nearest-neighbour search of Barnes et al. (2009), and the texture features
//! and best-patch final reconstruction of Newson, Almansa, Gousseau and Pérez,
//! "Non-Local Patch-Based Image Inpainting" (IPOL 2017).
//!
//! A [`PatchFillGuide`], such as a generative model's fill, can set the
//! hole's structure instead: the coarse levels where the scheme otherwise
//! decides it are skipped, and the guide's layout is rebuilt from patches of
//! the image, so posts and edges crossing the hole continue while its texture
//! comes from the photo.
//!
//! Matching and colour adaptation run on a perceptual encoding of the image
//! ([`PerceptualSpace`]). The output copies the caller's own pixel values from
//! the chosen sources, so texture and sensor grain are reproduced exactly
//! rather than re-synthesized, and linear or HDR values pass through unchanged.
//!
//! Coordinates are local pixel indices of the image passed in; pixel `(x, y)`
//! is element `y * width + x` of every buffer. All work is deterministic for a
//! given input and [`PatchFillSettings::seed`], independent of thread count.

mod distance;
mod field;
mod membrane;
mod priority;
mod pyramid;

use field::{Field, Vote};
use pyramid::Level;
use rayon::prelude::*;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

/// Patches are `2 * PATCH_RADIUS + 1` pixels square at every pyramid level.
const PATCH_RADIUS: usize = 3;

/// Pixels outside the hole, by Chebyshev distance, that a fill synthesizes
/// as well. Callers fade the fill out across this band; with colour adaptation
/// its outermost ring matches the surroundings' colour and light.
pub const PATCH_FILL_BAND: usize = PATCH_RADIUS;

/// Expectation-maximisation iterations per pyramid level. Coarse levels are
/// cheap and settle the structure; the finest level only refines texture.
const COARSE_ITERATIONS: usize = 10;
const FINEST_ITERATIONS: usize = 3;
/// PatchMatch sweeps per EM iteration, alternating scan direction.
const SEARCH_SWEEPS: usize = 2;
/// An EM iteration that changes the hole by less than this mean perceptual
/// amount per channel ends its level early.
const CONVERGENCE_TOLERANCE: f32 = 1.5e-3;
/// Boundary differences are averaged over band pixels this close (Chebyshev)
/// so that grain does not imprint on the colour correction.
const ADAPTATION_AVERAGE_RADIUS: isize = 4;
/// A guided fill starts at least this many halvings below the image, so the
/// finer levels re-synthesize the guide's texture from the image.
const MIN_GUIDED_LEVEL: usize = 1;
/// PatchMatch sweeps that match the guide before the first vote of a guided
/// fill, so that vote already copies patches that agree with it.
const GUIDED_START_SWEEPS: usize = 6;

/// Converts the caller's pixel values to and from the perceptual space in
/// which patches are compared and colours adapted.
///
/// The encoding should be roughly perceptually uniform, with ordinary image
/// content in about `[0, 1]`; `decode` must invert `encode`.
pub trait PerceptualSpace: Sync {
    fn encode(&self, value: [f32; 3]) -> [f32; 3];
    fn decode(&self, perceptual: [f32; 3]) -> [f32; 3];
}

/// For images that are already perceptually encoded, such as sRGB in `[0, 1]`.
#[derive(Clone, Copy, Debug, Default)]
pub struct IdentitySpace;

impl PerceptualSpace for IdentitySpace {
    fn encode(&self, value: [f32; 3]) -> [f32; 3] {
        value
    }

    fn decode(&self, perceptual: [f32; 3]) -> [f32; 3] {
        perceptual
    }
}

/// An image with a hole to fill.
#[derive(Clone, Copy, Debug)]
pub struct PatchFillImage<'a> {
    width: usize,
    height: usize,
    pixels: &'a [[f32; 3]],
    hole: &'a [bool],
    source: &'a [bool],
    guide: Option<PatchFillGuide<'a>>,
}

/// A rough fill of the hole that sets the structure of the result, such as a
/// generative model's output.
#[derive(Clone, Copy, Debug)]
pub struct PatchFillGuide<'a> {
    /// Same layout and pixel space as the image; only hole pixels are read.
    pixels: &'a [[f32; 3]],
    /// Edge, in image pixels, of the smallest detail the guide resolves.
    detail: f32,
}

impl<'a> PatchFillImage<'a> {
    /// `hole` marks the pixels to fill; `source` the pixels that may be copied
    /// from. Hole pixels are never copied, whatever `source` says.
    pub fn new(
        width: usize,
        height: usize,
        pixels: &'a [[f32; 3]],
        hole: &'a [bool],
        source: &'a [bool],
    ) -> Result<Self, PatchFillError> {
        let count = width
            .checked_mul(height)
            .filter(|count| *count > 0 && *count <= u32::MAX as usize)
            .ok_or(PatchFillError::InvalidInput(
                "image size is empty or too large",
            ))?;
        if pixels.len() != count || hole.len() != count || source.len() != count {
            return Err(PatchFillError::InvalidInput(
                "pixel or mask length does not match the image size",
            ));
        }
        if !pixels.iter().flatten().all(|value| value.is_finite()) {
            return Err(PatchFillError::InvalidInput(
                "image contains non-finite values",
            ));
        }
        Ok(Self {
            width,
            height,
            pixels,
            hole,
            source,
            guide: None,
        })
    }

    /// Fills the hole along the structure of `guide`, which holds a rough
    /// fill of every hole pixel in the image's pixel space and layout. Its
    /// texture is not kept: from the pyramid level whose pixels are `detail`
    /// image pixels across (where the guide is still sharp) the fill copies
    /// patches of the image that match it.
    pub fn with_guide(
        mut self,
        guide: &'a [[f32; 3]],
        detail: f32,
    ) -> Result<Self, PatchFillError> {
        if guide.len() != self.pixels.len() {
            return Err(PatchFillError::InvalidInput(
                "guide length does not match the image size",
            ));
        }
        if !detail.is_finite() || detail <= 0.0 {
            return Err(PatchFillError::InvalidInput("guide detail is not positive"));
        }
        let finite = guide
            .iter()
            .zip(self.hole)
            .all(|(value, hole)| !hole || value.iter().all(|channel| channel.is_finite()));
        if !finite {
            return Err(PatchFillError::InvalidInput(
                "guide contains non-finite values",
            ));
        }
        self.guide = Some(PatchFillGuide {
            pixels: guide,
            detail,
        });
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatchFillSettings {
    /// Shifts the fill's low-frequency colour and light to meet the hole's
    /// surroundings (a membrane interpolation of the boundary difference, as
    /// in Poisson image editing), while keeping the copied texture.
    pub color_adaptation: bool,
    /// Seeds the randomized search; equal inputs and seeds give equal fills.
    pub seed: u64,
}

impl Default for PatchFillSettings {
    fn default() -> Self {
        Self {
            color_adaptation: true,
            seed: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatchFillError {
    InvalidInput(&'static str),
    /// Too little of the image may be copied from to fill the hole.
    NoSource,
    Cancelled,
}

impl fmt::Display for PatchFillError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(reason) => write!(formatter, "invalid fill input: {reason}"),
            Self::NoSource => formatter
                .write_str("not enough surrounding image to fill from; paint a smaller area"),
            Self::Cancelled => formatter.write_str("fill cancelled"),
        }
    }
}

impl std::error::Error for PatchFillError {}

/// One synthesized pixel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilledPixel {
    pub x: usize,
    pub y: usize,
    /// Chebyshev distance to the hole: 0 inside it, up to [`PATCH_FILL_BAND`].
    pub distance: usize,
    /// In the caller's pixel space.
    pub value: [f32; 3],
}

/// The synthesized pixels of a fill: every hole pixel and every pixel within
/// [`PATCH_FILL_BAND`] of the hole, in row-major order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PatchFill {
    pixels: Vec<FilledPixel>,
}

impl PatchFill {
    pub fn pixels(&self) -> &[FilledPixel] {
        &self.pixels
    }

    pub fn is_empty(&self) -> bool {
        self.pixels.is_empty()
    }
}

/// Fills the hole of `image`. `progress` receives the completed fraction in
/// `[0, 1]` from the calling thread; `cancellation` is polled between steps.
pub fn patch_fill(
    image: &PatchFillImage<'_>,
    space: &impl PerceptualSpace,
    settings: PatchFillSettings,
    cancellation: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<PatchFill, PatchFillError> {
    let (width, height) = (image.width, image.height);
    if !image.hole.contains(&true) {
        return Ok(PatchFill::default());
    }
    let perceptual = image
        .pixels
        .par_iter()
        .map(|value| space.encode(*value))
        .collect::<Vec<_>>();
    let allowed = image
        .source
        .iter()
        .zip(image.hole)
        .map(|(source, hole)| *source && !hole)
        .collect::<Vec<_>>();
    let mut levels = pyramid::build(&perceptual, image.hole, allowed, width, height)?;
    // A guided fill starts where the guide is sharp and takes its structure
    // from there; coarser levels would only decide that structure again.
    let mut guide = image.guide.map(|guide| {
        let start = guided_start_level(guide.detail, levels.len());
        levels.truncate(start + 1);
        pyramid::guide_at_level(guide.pixels, space, width, height, start)
    });
    let schedule = Schedule::new(&levels);

    let mut done = 0.0f64;
    let mut coarser: Option<(Level, Field)> = None;
    let mut finest = None;
    // Coarsest first: the pyramid is ordered finest first.
    while let Some(mut level) = levels.pop() {
        let index = levels.len();
        let level_seed = settings.seed ^ ((index as u64) << 32);
        let mut field = match coarser.take() {
            None if guide.is_some() => {
                let start = guide.take().unwrap_or_default();
                for &index in &level.hole_pixels {
                    level.guide[index as usize] = start[index as usize];
                }
                level.refresh_features();
                let mut field = Field::random(&level, level_seed);
                for sweep in 0..GUIDED_START_SWEEPS {
                    field.search(&level, sweep as u64, level_seed ^ GUIDED_SEED);
                }
                field
            }
            None => {
                // The membrane start only shapes the texture features of known
                // pixels beside the hole; priority filling then replaces it.
                membrane::fill(&mut level.guide, level.width, level.height, &level.hole);
                level.refresh_features();
                priority::initialise(&mut level, level_seed);
                Field::random(&level, level_seed)
            }
            Some((coarse, coarse_field)) => {
                // Copying through the upsampled field gives the finer level
                // real texture at the coarse level's structure (Wexler et al.).
                level.upsample_hole_from(&coarse);
                level.refresh_features();
                let mut field = Field::upsampled(&level, &coarse, &coarse_field, level_seed);
                field.vote(&mut level, Vote::Uniform);
                field.refresh_distances(&level);
                field
            }
        };
        let iterations = Schedule::iterations(index);
        for iteration in 0..iterations {
            check(cancellation)?;
            for sweep in 0..SEARCH_SWEEPS {
                field.search(
                    &level,
                    (iteration * SEARCH_SWEEPS + sweep) as u64,
                    level_seed,
                );
            }
            check(cancellation)?;
            // Averaged votes settle structure but are smoother than any real
            // patch, so finest-level matching against them would favour the
            // least grainy sources. Once the structure is set there, best-patch
            // votes keep the estimate as textured as the photo (Newson et al.).
            let vote = if index == 0 && iteration > 0 {
                Vote::Best
            } else {
                Vote::Mean
            };
            let change = field.vote(&mut level, vote);
            done += schedule.weight(index);
            progress(schedule.fraction(done));
            if change < CONVERGENCE_TOLERANCE {
                done += schedule.weight(index) * (iterations - iteration - 1) as f64;
                break;
            }
        }
        if index == 0 {
            // Match against the final estimate before choosing what to copy.
            check(cancellation)?;
            field.search(&level, (iterations * SEARCH_SWEEPS) as u64, level_seed);
            finest = Some((level, field));
        } else {
            coarser = Some((level, field));
        }
    }
    let (level, field) = finest.ok_or(PatchFillError::NoSource)?;
    check(cancellation)?;

    let band = distance::chebyshev(image.hole, width, height, PATCH_FILL_BAND as u32 + 1);
    let mut pixels = field.reconstruct(&level, image.pixels, &band);
    drop(field);
    drop(level);
    if settings.color_adaptation {
        check(cancellation)?;
        adapt_colors(&mut pixels, &perceptual, space, width);
    }
    progress(1.0);
    Ok(PatchFill { pixels })
}

/// Separates the guided start's search from the EM sweeps that follow.
const GUIDED_SEED: u64 = 0x6A1D_ED00_57A2_7000;

/// The finest level, at least [`MIN_GUIDED_LEVEL`], whose pixels span
/// `detail` image pixels, within the `levels` the pyramid has.
fn guided_start_level(detail: f32, levels: usize) -> usize {
    let coarsest = levels.saturating_sub(1);
    let mut level = MIN_GUIDED_LEVEL.min(coarsest);
    while level < coarsest && ((1u64 << level) as f32) < detail {
        level += 1;
    }
    level
}

fn check(cancellation: &AtomicBool) -> Result<(), PatchFillError> {
    if cancellation.load(Ordering::Acquire) {
        Err(PatchFillError::Cancelled)
    } else {
        Ok(())
    }
}

/// Progress weights: each EM iteration costs about its level's hole area.
struct Schedule {
    hole_pixels: Vec<f64>,
    total: f64,
}

impl Schedule {
    fn new(levels: &[Level]) -> Self {
        let hole_pixels = levels
            .iter()
            .map(|level| level.hole.iter().filter(|hole| **hole).count().max(1) as f64)
            .collect::<Vec<_>>();
        let total = hole_pixels
            .iter()
            .enumerate()
            .map(|(index, pixels)| pixels * Self::iterations(index) as f64)
            .sum::<f64>();
        Self { hole_pixels, total }
    }

    fn iterations(level: usize) -> usize {
        if level == 0 {
            FINEST_ITERATIONS
        } else {
            COARSE_ITERATIONS
        }
    }

    fn weight(&self, level: usize) -> f64 {
        self.hole_pixels[level]
    }

    fn fraction(&self, done: f64) -> f32 {
        (done / self.total.max(1.0)).clamp(0.0, 1.0) as f32
    }
}

/// Shifts the fill's low frequencies to meet the surroundings: the perceptual
/// difference between the original and the fill on the band's outer ring is
/// interpolated smoothly across the band and hole, then added to the fill.
fn adapt_colors(
    pixels: &mut [FilledPixel],
    original: &[[f32; 3]],
    space: &impl PerceptualSpace,
    width: usize,
) {
    let ring = PATCH_FILL_BAND;
    let Some((left, top, right, bottom)) = pixels.iter().fold(None, |bounds, pixel| {
        let (l, t, r, b) = bounds.unwrap_or((pixel.x, pixel.y, pixel.x, pixel.y));
        Some((
            l.min(pixel.x),
            t.min(pixel.y),
            r.max(pixel.x),
            b.max(pixel.y),
        ))
    }) else {
        return;
    };
    let (grid_width, grid_height) = (right - left + 1, bottom - top + 1);
    let grid_index = |x: usize, y: usize| (y - top) * grid_width + (x - left);
    // Per grid pixel: the index into `pixels`, if synthesized.
    let mut slot = vec![u32::MAX; grid_width * grid_height];
    let mut encoded = Vec::with_capacity(pixels.len());
    for (index, pixel) in pixels.iter().enumerate() {
        slot[grid_index(pixel.x, pixel.y)] = index as u32;
        encoded.push(space.encode(pixel.value));
    }

    // Only the outer ring is known. Pixels beyond it are solved too, as a
    // smooth extension that keeps coarse multigrid levels undiluted; the ring
    // separates them from the band and hole.
    let mut correction = vec![[0.0f32; 3]; grid_width * grid_height];
    let mut unknown = vec![true; grid_width * grid_height];
    for pixel in pixels.iter() {
        if pixel.distance < ring {
            continue;
        }
        let cell = grid_index(pixel.x, pixel.y);
        unknown[cell] = false;
        // Outer ring: the original-minus-fill difference averaged over the
        // nearby band, where both are known.
        let mut sum = [0.0f32; 3];
        let mut count = 0usize;
        for dy in -ADAPTATION_AVERAGE_RADIUS..=ADAPTATION_AVERAGE_RADIUS {
            for dx in -ADAPTATION_AVERAGE_RADIUS..=ADAPTATION_AVERAGE_RADIUS {
                let (x, y) = (pixel.x as isize + dx, pixel.y as isize + dy);
                if x < left as isize
                    || y < top as isize
                    || x > right as isize
                    || y > bottom as isize
                {
                    continue;
                }
                let neighbor = slot[grid_index(x as usize, y as usize)];
                if neighbor == u32::MAX || pixels[neighbor as usize].distance == 0 {
                    continue;
                }
                let image_index = y as usize * width + x as usize;
                for channel in 0..3 {
                    sum[channel] +=
                        original[image_index][channel] - encoded[neighbor as usize][channel];
                }
                count += 1;
            }
        }
        correction[cell] = sum.map(|value| value / count.max(1) as f32);
    }
    membrane::fill(&mut correction, grid_width, grid_height, &unknown);
    pixels
        .par_iter_mut()
        .zip(encoded.par_iter())
        .for_each(|(pixel, encoded)| {
            let shift = correction[grid_index(pixel.x, pixel.y)];
            pixel.value = space.decode(std::array::from_fn(|channel| {
                encoded[channel] + shift[channel]
            }));
        });
}

#[cfg(test)]
mod tests;
