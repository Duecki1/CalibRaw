//! Texture synthesis for Remove: Big-LaMa's fill sets the structure, and
//! every pixel of the result is copied from the photo itself.
//!
//! Each pass renders its context, converts it to linear working Rec.2020 and
//! runs [`patch_fill`] guided by a render of the same context with Big-LaMa's
//! fill. The model continues edges and objects across the hole, but its
//! output is smooth and loses detail once upsampled; the synthesis rebuilds
//! that layout from matching patches of the photo, grain included. Contexts
//! larger than [`FILL_MAX_NATIVE_CONTEXT_PIXELS`] are filled at a reduced
//! working resolution and resampled to native pixels.

use super::*;
use crate::pipeline::{plan_fill_passes, FILL_MAX_NATIVE_CONTEXT_PIXELS};
use calibraw_core::patch_fill::{
    patch_fill, PatchFill, PatchFillError, PatchFillImage, PatchFillSettings, PerceptualSpace,
    PATCH_FILL_BAND,
};

/// Fills `mask` from the photo along the structure Big-LaMa generated for it.
pub(super) fn synthesize(
    request: &RemoveRequest,
    mask: RemoveMask,
    structure: &[GeneratedPass],
    events: &mpsc::Sender<RemoveEvent>,
) -> Result<Vec<RemovePatch>> {
    let passes = plan_fill_passes(request.raw.width, request.raw.height, &mask);
    anyhow::ensure!(!passes.is_empty(), "Remove mask produced no texture pass");
    let mut guide = request.existing.clone();
    guide.strokes.push(RemoveStroke {
        brush: RemoveBrushStroke::default(),
        patches: structure.iter().map(|pass| pass.patch.clone()).collect(),
        retouch: None,
        opacity: 1.0,
    });

    // Passes run in order: each one sees earlier fills as real context, while
    // the stroke pixels still to fill stay holes.
    let mut unfilled = mask;
    let mut patches = Vec::with_capacity(passes.len());
    for (index, pass) in passes.iter().enumerate() {
        ensure_not_cancelled(&request.cancellation)?;
        let mut context = request.existing.clone();
        if !patches.is_empty() {
            context.strokes.push(RemoveStroke {
                patches: patches.clone(),
                ..RemoveStroke::default()
            });
        }
        let mut progress = |fraction: f32| {
            let _ = events.send(RemoveEvent::Filling {
                area: index + 1,
                areas: passes.len(),
                fraction,
            });
        };
        let renders = PassRenders {
            context,
            guide: guide.clone(),
            model_pixel: model_pixel(structure, pass.target.bounds),
        };
        patches.extend(fill_pass(request, pass, &unfilled, renders, &mut progress)?);
        unfilled.subtract(&pass.target);
    }
    anyhow::ensure!(!patches.is_empty(), "Remove changed no pixels");
    Ok(patches)
}

/// Native pixels per Big-LaMa input pixel in the coarsest pass that covers
/// `bounds`: the size of the finest structure the guide resolves there.
fn model_pixel(structure: &[GeneratedPass], bounds: NativeRect) -> f32 {
    structure
        .iter()
        .filter(|pass| pass.crop.intersect(bounds).is_some())
        .map(|pass| pass.crop.width.max(pass.crop.height) as f32 / BIG_LAMA_INPUT_EDGE as f32)
        .fold(1.0, f32::max)
}

/// What a pass renders: its context, and the same context with Big-LaMa's
/// fill, which guides the synthesis.
struct PassRenders {
    context: RemoveEditState,
    guide: RemoveEditState,
    /// See [`model_pixel`].
    model_pixel: f32,
}

/// Fills `pass.target`; every unfilled stroke pixel in the context is a hole,
/// so the object being removed is never matched or copied.
fn fill_pass(
    request: &RemoveRequest,
    pass: &RemovePass,
    unfilled: &RemoveMask,
    renders: PassRenders,
    progress: &mut dyn FnMut(f32),
) -> Result<Option<RemovePatch>> {
    let crop = pass.crop;
    let context_pixels = u64::from(crop.width) * u64::from(crop.height);
    // Native contexts render whole; larger ones at a reduced working edge.
    let reduced_edge = (context_pixels > FILL_MAX_NATIVE_CONTEXT_PIXELS).then(|| {
        let scale = (FILL_MAX_NATIVE_CONTEXT_PIXELS as f64 / context_pixels as f64).sqrt();
        (f64::from(crop.width.max(crop.height)) * scale)
            .floor()
            .max(1.0) as u32
    });
    let render = |remove: RemoveEditState, what: &str| -> Result<(u32, u32, Vec<f32>)> {
        let job = DevelopedCropJob {
            device: request.device.clone(),
            queue: request.queue.clone(),
            raw: Arc::clone(&request.raw),
            geometry: request.geometry,
            exposure: request.exposure,
            masks: request.masks.clone(),
            remove,
            crop,
            program_prewarm: request.program_prewarm.clone(),
        };
        let (width, height, scene) = match reduced_edge {
            None => {
                let scene = render_remove_scene_crop(job)
                    .with_context(|| format!("render Remove {what}"))?;
                (crop.width, crop.height, scene)
            }
            Some(edge) => {
                let scene = render_remove_scene_crop_resized(job, edge)
                    .with_context(|| format!("render reduced Remove {what}"))?;
                (scene.width, scene.height, scene.pixels)
            }
        };
        anyhow::ensure!(
            scene.len() == width as usize * height as usize * 3,
            "Remove {what} {width}x{height} has an invalid RGB length"
        );
        Ok((width, height, scene))
    };
    let (width, height, scene) = render(renders.context, "texture context")?;
    ensure_not_cancelled(&request.cancellation)?;
    let (_, _, guide_scene) = render(renders.guide, "structure guide")?;
    ensure_not_cancelled(&request.cancellation)?;

    let geometry = WorkingGeometry {
        crop,
        width,
        height,
    };
    let hole = geometry.hole(unfilled);
    let source = hole.iter().map(|hole| !hole).collect::<Vec<_>>();
    let to_working = |scene: &[f32]| {
        scene
            .chunks_exact(3)
            .map(|pixel| {
                pipeline_scene_to_working_rec2020(&request.raw, [pixel[0], pixel[1], pixel[2]])
            })
            .collect::<Vec<_>>()
    };
    let pixels = to_working(&scene);
    let guide = to_working(&guide_scene);
    drop(guide_scene);
    // Metered on part of the context only; the object must not shift it.
    let metered = scene
        .chunks_exact(3)
        .zip(&hole)
        .filter(|(_, hole)| !**hole)
        .step_by(METERING_STRIDE)
        .flat_map(|(pixel, _)| [pixel[0], pixel[1], pixel[2]])
        .collect::<Vec<_>>();
    drop(scene);
    let space = WorkingSpace {
        gain: remove_model_view_gain(&request.raw, &metered),
    };

    let [scale_x, scale_y] = geometry.scale();
    let detail = renders.model_pixel * scale_x.max(scale_y);
    let image = PatchFillImage::new(width as usize, height as usize, &pixels, &hole, &source)
        .and_then(|image| image.with_guide(&guide, detail))
        .map_err(anyhow::Error::new)?;
    let settings = PatchFillSettings {
        color_adaptation: true,
        seed: u64::from(crop.x) << 32 | u64::from(crop.y),
    };
    let fill =
        patch_fill(&image, &space, settings, &request.cancellation, progress).map_err(|error| {
            match error {
                PatchFillError::Cancelled => anyhow::anyhow!("Remove operation cancelled"),
                other => anyhow::Error::new(other),
            }
        })?;
    drop(guide);

    let raw = request.raw.as_ref();
    let exposure = &request.exposure;
    if geometry.is_native() {
        native_patch(raw, exposure, crop, &pass.target, &fill)
    } else {
        resampled_patch(raw, exposure, &geometry, &pass.target, &fill, &pixels)
    }
}

/// Every this many known context pixels is metered for the view gain.
const METERING_STRIDE: usize = 16;

/// Linear working Rec.2020, encoded as sRGB after a gain that places the
/// context's upper quartile like a normal exposure: shadows and highlights
/// then weigh alike in patch distances. Heal blends in the same encoding.
struct WorkingSpace {
    gain: f32,
}

impl PerceptualSpace for WorkingSpace {
    fn encode(&self, value: [f32; 3]) -> [f32; 3] {
        value.map(|channel| srgb_encode_signed(channel * self.gain))
    }

    fn decode(&self, perceptual: [f32; 3]) -> [f32; 3] {
        perceptual.map(|channel| srgb_decode_signed(channel) / self.gain)
    }
}

/// The working raster of a pass: its native crop, possibly reduced.
struct WorkingGeometry {
    crop: NativeRect,
    width: u32,
    height: u32,
}

impl WorkingGeometry {
    fn is_native(&self) -> bool {
        self.width == self.crop.width && self.height == self.crop.height
    }

    /// Working pixels per native pixel along each axis.
    fn scale(&self) -> [f32; 2] {
        [
            self.width as f32 / self.crop.width.max(1) as f32,
            self.height as f32 / self.crop.height.max(1) as f32,
        ]
    }

    /// Working pixels covering any unfilled native pixel, so no fragment of
    /// the object survives a reduced resolution.
    fn hole(&self, unfilled: &RemoveMask) -> Vec<bool> {
        let (width, height) = (self.width as usize, self.height as usize);
        let mut hole = vec![false; width * height];
        let Some(overlap) = unfilled.bounds.intersect(self.crop) else {
            return hole;
        };
        let [scale_x, scale_y] = self.scale();
        let span = |start: u32, scale: f32, limit: usize| {
            let first = (start as f32 * scale).floor() as usize;
            let last = (((start + 1) as f32 * scale).ceil() as usize).saturating_sub(1);
            first.min(limit - 1)..=last.min(limit - 1)
        };
        for y in overlap.y..overlap.bottom() {
            for x in overlap.x..overlap.right() {
                if !unfilled.contains_global(x, y) {
                    continue;
                }
                for working_y in span(y - self.crop.y, scale_y, height) {
                    for working_x in span(x - self.crop.x, scale_x, width) {
                        hole[working_y * width + working_x] = true;
                    }
                }
            }
        }
        hole
    }

    /// Continuous working coordinates of a native pixel centre.
    fn native_to_working(&self, x: u32, y: u32) -> [f32; 2] {
        let [scale_x, scale_y] = self.scale();
        [
            ((x - self.crop.x) as f32 + 0.5) * scale_x - 0.5,
            ((y - self.crop.y) as f32 + 0.5) * scale_y - 0.5,
        ]
    }
}

/// Opacity across the band around the target: opaque inside it, fading to
/// nothing just beyond `band` pixels from it.
fn feather_alpha(distance: f32, band: f32) -> u8 {
    ((1.0 - distance / (band + 1.0)).clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Chebyshev distance from native `(x, y)` to the nearest target pixel, if
/// within `reach`.
fn distance_to_target(target: &RemoveMask, x: u32, y: u32, reach: u32) -> Option<u32> {
    (1..=reach).find(|&distance| {
        let (left, right) = (x.saturating_sub(distance), x.saturating_add(distance));
        let (top, bottom) = (y.saturating_sub(distance), y.saturating_add(distance));
        (left..=right).any(|ring_x| {
            target.contains_global(ring_x, top) || target.contains_global(ring_x, bottom)
        }) || (top..=bottom).any(|ring_y| {
            target.contains_global(left, ring_y) || target.contains_global(right, ring_y)
        })
    })
}

/// One native-resolution pixel of a patch under construction.
struct PatchPixel {
    x: u32,
    y: u32,
    alpha: u8,
    working: [f32; 3],
}

/// A patch of the target's fill and the feathered band around it.
fn native_patch(
    raw: &LoadedRaw,
    exposure: &ExposureParams,
    crop: NativeRect,
    target: &RemoveMask,
    fill: &PatchFill,
) -> Result<Option<RemovePatch>> {
    let band = PATCH_FILL_BAND as u32;
    let pixels = fill
        .pixels()
        .iter()
        .filter_map(|pixel| {
            let (x, y) = (crop.x + pixel.x as u32, crop.y + pixel.y as u32);
            let alpha = if target.contains_global(x, y) {
                255
            } else if pixel.distance == 0 {
                // A hole pixel of another pass.
                return None;
            } else {
                let distance = distance_to_target(target, x, y, band)?;
                feather_alpha(distance as f32, band as f32)
            };
            Some(PatchPixel {
                x,
                y,
                alpha,
                working: pixel.value,
            })
        })
        .collect::<Vec<_>>();
    build_patch(raw, exposure, &pixels)
}

/// A patch filled at a reduced resolution, resampled to native pixels over
/// the target and a band scaled like the working pixels.
fn resampled_patch(
    raw: &LoadedRaw,
    exposure: &ExposureParams,
    geometry: &WorkingGeometry,
    target: &RemoveMask,
    fill: &PatchFill,
    original: &[[f32; 3]],
) -> Result<Option<RemovePatch>> {
    let working = WorkingFill::new(geometry, fill, original)?;
    let [scale_x, scale_y] = geometry.scale();
    let band = (PATCH_FILL_BAND as f32 / scale_x.min(scale_y))
        .floor()
        .max(1.0) as u32;
    let crop = geometry.crop;
    let region = NativeRect {
        x: target.bounds.x.saturating_sub(band),
        y: target.bounds.y.saturating_sub(band),
        width: target.bounds.width + 2 * band,
        height: target.bounds.height + 2 * band,
    }
    .intersect(crop)
    .unwrap_or(target.bounds);
    let mut pixels = Vec::new();
    for y in region.y..region.bottom() {
        for x in region.x..region.right() {
            let [working_x, working_y] = geometry.native_to_working(x, y);
            if !working.synthesized_near(working_x, working_y) {
                continue;
            }
            let alpha = if target.contains_global(x, y) {
                255
            } else {
                let Some(distance) = distance_to_target(target, x, y, band) else {
                    continue;
                };
                feather_alpha(distance as f32, band as f32)
            };
            pixels.push(PatchPixel {
                x,
                y,
                alpha,
                working: working.sample(working_x, working_y),
            });
        }
    }
    build_patch(raw, exposure, &pixels)
}

/// The working-resolution fill over its bounding box, ready to resample.
struct WorkingFill {
    left: u32,
    top: u32,
    image: Rgb32FImage,
    synthesized: Vec<bool>,
}

impl WorkingFill {
    /// Bicubic resampling reads two pixels beyond the fill, which come from
    /// the original context.
    const MARGIN: u32 = 2;

    fn new(geometry: &WorkingGeometry, fill: &PatchFill, original: &[[f32; 3]]) -> Result<Self> {
        let (mut left, mut top, mut right, mut bottom) = (u32::MAX, u32::MAX, 0, 0);
        for pixel in fill.pixels() {
            left = left.min(pixel.x as u32);
            top = top.min(pixel.y as u32);
            right = right.max(pixel.x as u32 + 1);
            bottom = bottom.max(pixel.y as u32 + 1);
        }
        anyhow::ensure!(right > left && bottom > top, "Fill synthesized no pixels");
        let left = left.saturating_sub(Self::MARGIN);
        let top = top.saturating_sub(Self::MARGIN);
        let right = (right + Self::MARGIN).min(geometry.width);
        let bottom = (bottom + Self::MARGIN).min(geometry.height);
        let (width, height) = (right - left, bottom - top);
        let mut image = Rgb32FImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let index = (top + y) as usize * geometry.width as usize + (left + x) as usize;
                image.put_pixel(x, y, image::Rgb(original[index]));
            }
        }
        let mut synthesized = vec![false; width as usize * height as usize];
        for pixel in fill.pixels() {
            let (x, y) = (pixel.x as u32 - left, pixel.y as u32 - top);
            image.put_pixel(x, y, image::Rgb(pixel.value));
            synthesized[(y * width + x) as usize] = true;
        }
        Ok(Self {
            left,
            top,
            image,
            synthesized,
        })
    }

    /// Whether the working pixel nearest to these coordinates was synthesized.
    fn synthesized_near(&self, x: f32, y: f32) -> bool {
        let x = x.round() as i64 - i64::from(self.left);
        let y = y.round() as i64 - i64::from(self.top);
        (0..i64::from(self.image.width())).contains(&x)
            && (0..i64::from(self.image.height())).contains(&y)
            && self.synthesized[(y * i64::from(self.image.width()) + x) as usize]
    }

    fn sample(&self, x: f32, y: f32) -> [f32; 3] {
        lama::sample_catmull_rom(&self.image, x - self.left as f32, y - self.top as f32)
    }
}

/// Packs native pixels into a canonical-scene patch over their bounds.
fn build_patch(
    raw: &LoadedRaw,
    exposure: &ExposureParams,
    pixels: &[PatchPixel],
) -> Result<Option<RemovePatch>> {
    let Some(first) = pixels.first() else {
        return Ok(None);
    };
    let (mut left, mut top, mut right, mut bottom) = (first.x, first.y, first.x, first.y);
    for pixel in pixels {
        left = left.min(pixel.x);
        top = top.min(pixel.y);
        right = right.max(pixel.x);
        bottom = bottom.max(pixel.y);
    }
    let bounds = NativeRect {
        x: left,
        y: top,
        width: right - left + 1,
        height: bottom - top + 1,
    };
    let count = bounds.width as usize * bounds.height as usize;
    let mut rgb16f = vec![0u16; count * 3];
    let mut alpha = vec![0u8; count];
    for pixel in pixels {
        let index = (pixel.y - top) as usize * bounds.width as usize + (pixel.x - left) as usize;
        alpha[index] = pixel.alpha;
        let canonical = working_rec2020_to_canonical_remove_scene(raw, exposure, pixel.working);
        for (channel, value) in canonical.into_iter().enumerate() {
            let finite = if value.is_finite() { value } else { 0.0 };
            rgb16f[index * 3 + channel] =
                half::f16::from_f32(finite.clamp(-65_504.0, 65_504.0)).to_bits();
        }
    }
    RemovePatch::new_scene(bounds, rgb16f, alpha)
        .map(Some)
        .map_err(anyhow::Error::msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: u32, y: u32, edge: u32) -> RemoveMask {
        RemoveMask {
            bounds: NativeRect {
                x,
                y,
                width: edge,
                height: edge,
            },
            pixels: vec![255; (edge * edge) as usize],
        }
    }

    #[test]
    fn working_space_round_trips_hdr_and_negative_values() {
        let space = WorkingSpace { gain: 2.5 };
        for value in [[0.0, 0.18, 1.0], [-0.01, 4.0, 120.0]] {
            let restored = space.decode(space.encode(value));
            for channel in 0..3 {
                assert!(
                    (restored[channel] - value[channel]).abs()
                        <= 1e-4 * value[channel].abs().max(1.0)
                );
            }
        }
    }

    #[test]
    fn reduced_hole_covers_every_unfilled_pixel() {
        let geometry = WorkingGeometry {
            crop: NativeRect {
                x: 1000,
                y: 2000,
                width: 900,
                height: 600,
            },
            width: 300,
            height: 200,
        };
        let speck = square(1301, 2302, 1);
        let hole = geometry.hole(&speck);
        assert_eq!(hole.iter().filter(|hole| **hole).count(), 1);
        let [x, y] = geometry.native_to_working(1301, 2302);
        assert!(hole[y.round() as usize * 300 + x.round() as usize]);
    }

    #[test]
    fn distance_to_target_measures_chebyshev_rings() {
        let target = square(10, 10, 4);
        assert_eq!(distance_to_target(&target, 14, 12, 3), Some(1));
        assert_eq!(distance_to_target(&target, 16, 16, 3), Some(3));
        assert_eq!(distance_to_target(&target, 17, 12, 3), None);
        assert_eq!(feather_alpha(0.0, 3.0), 255);
        assert!(feather_alpha(1.0, 3.0) > feather_alpha(3.0, 3.0));
    }

    #[test]
    fn native_patch_is_opaque_on_the_target_and_feathered_around_it() {
        let raw = LoadedRaw::from_scene_linear_rec2020(64, 64, vec![0.2; 64 * 64 * 3]).unwrap();
        let crop = NativeRect {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
        };
        let pixels = vec![[0.2f32; 3]; 64 * 64];
        let hole = (0..64 * 64)
            .map(|index| {
                let (x, y) = (index % 64, index / 64);
                (20..30).contains(&x) && (20..30).contains(&y)
            })
            .collect::<Vec<_>>();
        let source = hole.iter().map(|hole| !hole).collect::<Vec<_>>();
        let image = PatchFillImage::new(64, 64, &pixels, &hole, &source).unwrap();
        let fill = patch_fill(
            &image,
            &calibraw_core::patch_fill::IdentitySpace,
            PatchFillSettings::default(),
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        let patch = native_patch(
            &raw,
            &ExposureParams::default(),
            crop,
            &square(20, 20, 10),
            &fill,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            patch.bounds,
            NativeRect {
                x: 17,
                y: 17,
                width: 16,
                height: 16
            }
        );
        let alpha = |x: u32, y: u32| patch.alpha[((y - 17) * 16 + (x - 17)) as usize];
        assert_eq!(alpha(25, 25), 255);
        assert_eq!(alpha(20, 29), 255);
        assert!(alpha(19, 25) > alpha(18, 25) && alpha(18, 25) > alpha(17, 25));
        assert!(alpha(17, 25) > 0);
    }

    #[test]
    fn reduced_fill_is_resampled_over_the_native_target() {
        let raw = LoadedRaw::from_scene_linear_rec2020(1, 1, vec![0.2; 3]).unwrap();
        // A 200x160 native context filled at half resolution.
        let geometry = WorkingGeometry {
            crop: NativeRect {
                x: 400,
                y: 300,
                width: 200,
                height: 160,
            },
            width: 100,
            height: 80,
        };
        let target = square(470, 350, 40);
        let hole = geometry.hole(&target);
        let pixels = (0..100 * 80)
            .map(|index| {
                let value = 0.3 + 0.02 * ((index % 100) as f32 / 99.0);
                if hole[index] {
                    [5.0; 3]
                } else {
                    [value; 3]
                }
            })
            .collect::<Vec<_>>();
        let source = hole.iter().map(|hole| !hole).collect::<Vec<_>>();
        let image = PatchFillImage::new(100, 80, &pixels, &hole, &source).unwrap();
        let fill = patch_fill(
            &image,
            &calibraw_core::patch_fill::IdentitySpace,
            PatchFillSettings::default(),
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        let patch = resampled_patch(
            &raw,
            &ExposureParams::default(),
            &geometry,
            &target,
            &fill,
            &pixels,
        )
        .unwrap()
        .unwrap();
        // Three working pixels of band are six native ones.
        assert_eq!(
            patch.bounds,
            NativeRect {
                x: 464,
                y: 344,
                width: 52,
                height: 52,
            }
        );
        for y in 350..390u32 {
            for x in 470..510u32 {
                let index = ((y - 344) * 52 + (x - 464)) as usize;
                assert_eq!(patch.alpha[index], 255);
                let red = half::f16::from_bits(patch.rgb_scene16f[index * 3]).to_f32();
                assert!((0.29..0.33).contains(&red), "{x},{y}: {red}");
            }
        }
    }

    #[test]
    #[ignore = "requires a working wgpu device"]
    fn synthesis_replaces_an_object_with_the_surrounding_texture_on_the_gpu() {
        let (width, height) = (256u32, 192u32);
        let mut scene = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                let stripe = if (x / 6) % 2 == 0 { 0.12 } else { 0.2 };
                // A bright object the fill must not reproduce.
                let object = (110..150).contains(&x) && (80..115).contains(&y);
                let value = if object {
                    0.9
                } else {
                    stripe + 0.002 * ((x * 7 + y * 13) % 5) as f32
                };
                scene.extend([value, value * 0.9, value * 0.8]);
            }
        }
        let mut composited = scene.clone();
        let raw = Arc::new(LoadedRaw::from_scene_linear_rec2020(width, height, scene).unwrap());
        let instance = wgpu::Instance::default();
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .expect("a wgpu adapter is required");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("Fill integration test"),
            ..Default::default()
        }))
        .unwrap();
        let brush = RemoveBrushStroke {
            points: (0..9)
                .map(|step| crate::pipeline::RemoveBrushPoint {
                    x: 112.0 + step as f32 * 4.5,
                    y: 97.0,
                    radius: 22.0,
                })
                .collect(),
            dilation_radius: 0,
        };
        let mut brush = brush;
        brush.dilation_radius = adaptive_remove_dilation(&brush.points);
        let mask = rasterize_remove_brush(width, height, &brush).unwrap();
        // A flat stand-in for Big-LaMa's fill: the structure is "no object".
        let bounds = mask.bounds;
        let structure_patch = RemovePatch::new_scene(
            bounds,
            [0.16f32, 0.144, 0.128]
                .repeat((bounds.width * bounds.height) as usize)
                .into_iter()
                .map(|value| half::f16::from_f32(value).to_bits())
                .collect(),
            mask.pixels.clone(),
        )
        .unwrap();
        let request = RemoveRequest {
            device,
            queue,
            raw: Arc::clone(&raw),
            geometry: GeometryTransform::default(),
            exposure: ExposureParams::default(),
            masks: MaskStack::default(),
            existing: RemoveEditState::default(),
            brush: RemoveBrushStroke::default(),
            opacity: 1.0,
            model_path: PathBuf::new(),
            allow_download: false,
            runtime_path: None,
            runtime_sha256: None,
            program_prewarm: None,
            cancellation: Arc::new(AtomicBool::new(false)),
        };
        let structure = [GeneratedPass {
            crop: NativeRect {
                x: 0,
                y: 0,
                width,
                height,
            },
            patch: structure_patch,
        }];
        let (events, receiver) = mpsc::channel();
        let patches = synthesize(&request, mask, &structure, &events).unwrap();
        drop(events);
        assert!(receiver.iter().any(
            |event| matches!(event, RemoveEvent::Filling { fraction, .. } if fraction >= 1.0)
        ));

        let region = NativeRect {
            x: 0,
            y: 0,
            width,
            height,
        };
        crate::pipeline::composite_remove_edits_into_linear_region(
            &RemoveEditState {
                strokes: vec![RemoveStroke {
                    patches,
                    ..RemoveStroke::default()
                }],
            },
            region,
            &mut composited,
        );
        for y in 80..115u32 {
            for x in 110..150u32 {
                let red = composited[((y * width + x) * 3) as usize];
                assert!(
                    (0.1..0.23).contains(&red),
                    "object pixel {x},{y} filled with {red}"
                );
            }
        }
    }
}
