//! Big-LaMa inference for one Remove pass.
//!
//! The ONNX export takes a fixed 512x512 image in [0, 1] with a binary mask,
//! masks the image itself, and returns `mask * prediction + (1 - mask) *
//! image` scaled to [0, 255]. A pass maps its native crop onto that input, so
//! a 512px crop runs without any resampling.

use super::*;

/// Native pixels feathered outside the filled target, where the model also
/// generated content, so the patch blends into the photo without a seam.
const MIN_FEATHER: f32 = 2.0;
const FEATHER_PER_MODEL_PIXEL: f32 = 1.5;
/// The view gain is metered on context pixels only; with fewer than this many
/// the whole crop is used.
const MIN_METERED_PIXELS: usize = 64;

/// Fills `pass.target` from `scene`, a render of `pass.crop`. `unfilled` holds
/// every stroke pixel not filled yet; all of them inside the crop stay masked,
/// so the model never sees the object it removes.
pub(super) fn infer_pass(
    model_path: &Path,
    pass: &RemovePass,
    unfilled: &RemoveMask,
    raw: &LoadedRaw,
    exposure: &ExposureParams,
    scene: &ResizedRemoveSceneCrop,
) -> Result<RemovePatch> {
    let geometry = PassGeometry::new(pass.crop);
    let model_mask = geometry.model_mask(unfilled);
    anyhow::ensure!(
        model_mask.iter().any(|masked| *masked),
        "Remove mask vanished at the model resolution"
    );
    let scene = scene_at_model_resolution(scene)?;
    let view_gain = metered_view_gain(raw, &scene, &model_mask);

    let edge = BIG_LAMA_INPUT_EDGE as usize;
    let plane = edge * edge;
    let mut image_values = vec![0.0f32; plane * 3];
    for (index, pixel) in scene.pixels().enumerate() {
        let srgb = remove_scene_to_model_srgb(raw, [pixel[0], pixel[1], pixel[2]], view_gain);
        for channel in 0..3 {
            image_values[channel * plane + index] = srgb[channel].clamp(0.0, 1.0);
        }
    }
    let mask_values = model_mask
        .iter()
        .map(|masked| f32::from(u8::from(*masked)))
        .collect::<Vec<_>>();
    let output = run_big_lama(model_path, image_values, mask_values)?;

    let alpha = geometry.feather_alpha(&pass.target, &model_mask);
    let bounds = alpha.bounds;
    let mut rgb16f = Vec::with_capacity(bounds.width as usize * bounds.height as usize * 3);
    for y in bounds.y..bounds.bottom() {
        for x in bounds.x..bounds.right() {
            let [model_x, model_y] = geometry.native_to_model(x, y);
            let srgb = sample_catmull_rom(&output, model_x, model_y).map(|v| v.clamp(0.0, 1.0));
            let generated = remove_model_srgb_to_canonical_scene(raw, exposure, srgb, view_gain);
            for value in generated {
                let finite = if value.is_finite() { value } else { 0.0 };
                rgb16f.push(half::f16::from_f32(finite.clamp(-65_504.0, 65_504.0)).to_bits());
            }
        }
    }
    RemovePatch::new_scene(bounds, rgb16f, alpha.pixels).map_err(anyhow::Error::msg)
}

/// Maps a native crop onto the square model input.
#[derive(Clone, Copy, Debug)]
struct PassGeometry {
    crop: NativeRect,
    /// Native pixels per model pixel along each axis.
    scale: [f32; 2],
}

impl PassGeometry {
    fn new(crop: NativeRect) -> Self {
        let edge = BIG_LAMA_INPUT_EDGE as f32;
        Self {
            crop,
            scale: [
                crop.width.max(1) as f32 / edge,
                crop.height.max(1) as f32 / edge,
            ],
        }
    }

    /// Feather width in native pixels: wider when the model ran downscaled,
    /// so the softer upsampled fill fades in over a few of its own pixels.
    fn feather(self) -> f32 {
        (FEATHER_PER_MODEL_PIXEL * self.scale[0].max(self.scale[1])).max(MIN_FEATHER)
    }

    /// Continuous model coordinates of a native pixel centre.
    fn native_to_model(self, x: u32, y: u32) -> [f32; 2] {
        [
            ((x - self.crop.x) as f32 + 0.5) / self.scale[0] - 0.5,
            ((y - self.crop.y) as f32 + 0.5) / self.scale[1] - 0.5,
        ]
    }

    fn model_index(self, x: u32, y: u32) -> usize {
        let edge = BIG_LAMA_INPUT_EDGE as usize;
        let model_x = (((x - self.crop.x) as f32 / self.scale[0]) as usize).min(edge - 1);
        let model_y = (((y - self.crop.y) as f32 / self.scale[1]) as usize).min(edge - 1);
        model_y * edge + model_x
    }

    /// Model-resolution mask: a model pixel is masked when any native pixel
    /// it covers is unfilled, so no fragment of the object stays visible.
    /// It is then grown by the feather width, which the model fills too.
    fn model_mask(self, unfilled: &RemoveMask) -> Vec<bool> {
        let edge = BIG_LAMA_INPUT_EDGE as usize;
        let mut mask = vec![false; edge * edge];
        if let Some(overlap) = unfilled.bounds.intersect(self.crop) {
            for y in overlap.y..overlap.bottom() {
                for x in overlap.x..overlap.right() {
                    if unfilled.contains_global(x, y) {
                        mask[self.model_index(x, y)] = true;
                    }
                }
            }
        }
        let radius = (self.feather() / self.scale[0].min(self.scale[1]))
            .ceil()
            .max(1.0) as usize;
        dilate_square(&mask, edge, edge, radius)
    }

    /// Alpha for the patch: opaque on the target and fading to zero over the
    /// feather width outside it, never beyond what the model generated.
    fn feather_alpha(self, target: &RemoveMask, model_mask: &[bool]) -> FeatherAlpha {
        let feather = self.feather();
        let margin = feather.ceil() as u32 + 1;
        let region = NativeRect {
            x: target.bounds.x.saturating_sub(margin),
            y: target.bounds.y.saturating_sub(margin),
            width: target.bounds.width + 2 * margin,
            height: target.bounds.height + 2 * margin,
        }
        .intersect(self.crop)
        .unwrap_or(target.bounds);
        let distance = chamfer_distance(region, target);
        let mut alpha = vec![0u8; distance.len()];
        for (index, (value, distance)) in alpha.iter_mut().zip(&distance).enumerate() {
            let x = region.x + (index % region.width as usize) as u32;
            let y = region.y + (index / region.width as usize) as u32;
            if !model_mask[self.model_index(x, y)] {
                continue;
            }
            let coverage = (1.0 - distance / (feather + 1.0)).clamp(0.0, 1.0);
            *value = (coverage * 255.0).round() as u8;
        }
        FeatherAlpha::shrunk(region, alpha)
    }
}

struct FeatherAlpha {
    bounds: NativeRect,
    pixels: Vec<u8>,
}

impl FeatherAlpha {
    fn shrunk(region: NativeRect, alpha: Vec<u8>) -> Self {
        let width = region.width as usize;
        let (mut left, mut top, mut right, mut bottom) = (usize::MAX, usize::MAX, 0, 0);
        for (index, value) in alpha.iter().enumerate() {
            if *value != 0 {
                let (x, y) = (index % width, index / width);
                left = left.min(x);
                top = top.min(y);
                right = right.max(x + 1);
                bottom = bottom.max(y + 1);
            }
        }
        if right <= left {
            return Self {
                bounds: NativeRect::default(),
                pixels: Vec::new(),
            };
        }
        let mut pixels = Vec::with_capacity((right - left) * (bottom - top));
        for y in top..bottom {
            pixels.extend_from_slice(&alpha[y * width + left..y * width + right]);
        }
        Self {
            bounds: NativeRect {
                x: region.x + left as u32,
                y: region.y + top as u32,
                width: (right - left) as u32,
                height: (bottom - top) as u32,
            },
            pixels,
        }
    }
}

/// Approximate Euclidean distance (3-4 chamfer) in native pixels from each
/// pixel of `region` to the nearest target pixel.
fn chamfer_distance(region: NativeRect, target: &RemoveMask) -> Vec<f32> {
    const ORTHOGONAL: u32 = 3;
    const DIAGONAL: u32 = 4;
    let width = region.width as usize;
    let height = region.height as usize;
    let mut distance = vec![u32::MAX / 2; width * height];
    for y in 0..height {
        for x in 0..width {
            if target.contains_global(region.x + x as u32, region.y + y as u32) {
                distance[y * width + x] = 0;
            }
        }
    }
    let relax = |distance: &mut [u32], index: usize, neighbor: usize, step: u32| {
        let candidate = distance[neighbor] + step;
        if candidate < distance[index] {
            distance[index] = candidate;
        }
    };
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            if x > 0 {
                relax(&mut distance, index, index - 1, ORTHOGONAL);
            }
            if y > 0 {
                relax(&mut distance, index, index - width, ORTHOGONAL);
                if x > 0 {
                    relax(&mut distance, index, index - width - 1, DIAGONAL);
                }
                if x + 1 < width {
                    relax(&mut distance, index, index - width + 1, DIAGONAL);
                }
            }
        }
    }
    for y in (0..height).rev() {
        for x in (0..width).rev() {
            let index = y * width + x;
            if x + 1 < width {
                relax(&mut distance, index, index + 1, ORTHOGONAL);
            }
            if y + 1 < height {
                relax(&mut distance, index, index + width, ORTHOGONAL);
                if x + 1 < width {
                    relax(&mut distance, index, index + width + 1, DIAGONAL);
                }
                if x > 0 {
                    relax(&mut distance, index, index + width - 1, DIAGONAL);
                }
            }
        }
    }
    distance
        .into_iter()
        .map(|value| value as f32 / ORTHOGONAL as f32)
        .collect()
}

/// Grows a binary mask by `radius` pixels in every direction (square
/// structuring element, applied separably).
fn dilate_square(mask: &[bool], width: usize, height: usize, radius: usize) -> Vec<bool> {
    let mut rows = vec![false; mask.len()];
    for y in 0..height {
        let row = &mask[y * width..(y + 1) * width];
        for x in 0..width {
            let start = x.saturating_sub(radius);
            let end = (x + radius + 1).min(width);
            rows[y * width + x] = row[start..end].iter().any(|value| *value);
        }
    }
    let mut out = vec![false; mask.len()];
    for x in 0..width {
        for y in 0..height {
            let start = y.saturating_sub(radius);
            let end = (y + radius + 1).min(height);
            out[y * width + x] = (start..end).any(|row| rows[row * width + x]);
        }
    }
    out
}

/// The rendered crop as a 512x512 linear scene image.
fn scene_at_model_resolution(scene: &ResizedRemoveSceneCrop) -> Result<Rgb32FImage> {
    anyhow::ensure!(
        scene.width <= BIG_LAMA_INPUT_EDGE
            && scene.height <= BIG_LAMA_INPUT_EDGE
            && scene.pixels.len() == scene.width as usize * scene.height as usize * 3,
        "Remove working scene {}x{} is invalid",
        scene.width,
        scene.height,
    );
    let image: Rgb32FImage = ImageBuffer::from_raw(scene.width, scene.height, scene.pixels.clone())
        .context("construct Remove working scene")?;
    if scene.width == BIG_LAMA_INPUT_EDGE && scene.height == BIG_LAMA_INPUT_EDGE {
        return Ok(image);
    }
    Ok(image::imageops::resize(
        &image,
        BIG_LAMA_INPUT_EDGE,
        BIG_LAMA_INPUT_EDGE,
        FilterType::Lanczos3,
    ))
}

/// Exposure for the model's view, metered on the context the model keeps so
/// the brightness of the removed object does not shift it.
fn metered_view_gain(raw: &LoadedRaw, scene: &Rgb32FImage, model_mask: &[bool]) -> f32 {
    let context = scene
        .pixels()
        .zip(model_mask)
        .filter(|(_, masked)| !**masked)
        .flat_map(|(pixel, _)| pixel.0)
        .collect::<Vec<_>>();
    if context.len() / 3 >= MIN_METERED_PIXELS {
        remove_model_view_gain(raw, &context)
    } else {
        remove_model_view_gain(raw, scene.as_raw())
    }
}

fn run_big_lama(
    model_path: &Path,
    image_values: Vec<f32>,
    mask_values: Vec<f32>,
) -> Result<Rgb32FImage> {
    let edge = BIG_LAMA_INPUT_EDGE as usize;
    let plane = edge * edge;
    let image_tensor = Tensor::from_array(([1usize, 3, edge, edge], image_values))
        .context("create Big-LaMa image tensor")?;
    let mask_tensor = Tensor::from_array(([1usize, 1, edge, edge], mask_values))
        .context("create Big-LaMa mask tensor")?;
    let values = with_model_session(
        AiModel::BigLama,
        model_path,
        SessionOptions::new("Big-LaMa Remove"),
        ModelRetention::WhileWarm,
        |session| {
            session.run_with_fallback(
                "Big-LaMa Remove ONNX inference",
                |ort_session, _accelerated| {
                    let outputs = ort_session
                        .run(ort::inputs![&image_tensor, &mask_tensor])
                        .context("run Big-LaMa ONNX inference")?;
                    let output = outputs
                        .values()
                        .next()
                        .context("Big-LaMa returned no output tensor")?;
                    let (shape, values) = output
                        .try_extract_tensor::<f32>()
                        .context("read Big-LaMa output tensor")?;
                    anyhow::ensure!(
                        shape.as_ref() == [1, 3, edge as i64, edge as i64],
                        "unexpected Big-LaMa output shape {shape:?}"
                    );
                    anyhow::ensure!(
                        values.len() == plane * 3 && values.iter().all(|value| value.is_finite()),
                        "Big-LaMa output tensor is invalid"
                    );
                    Ok(values.to_vec())
                },
            )
        },
    )?;
    let mut interleaved = vec![0.0f32; plane * 3];
    for index in 0..plane {
        for channel in 0..3 {
            interleaved[index * 3 + channel] =
                (values[channel * plane + index] / 255.0).clamp(0.0, 1.0);
        }
    }
    ImageBuffer::from_raw(BIG_LAMA_INPUT_EDGE, BIG_LAMA_INPUT_EDGE, interleaved)
        .context("construct Big-LaMa output image")
}

/// Bicubic (Catmull-Rom) sample at continuous pixel coordinates, where pixel
/// `i` is centred on `i`. Sharper than bilinear when the model ran downscaled.
fn sample_catmull_rom(image: &Rgb32FImage, x: f32, y: f32) -> [f32; 3] {
    fn weights(t: f32) -> [f32; 4] {
        let t2 = t * t;
        let t3 = t2 * t;
        [
            0.5 * (-t3 + 2.0 * t2 - t),
            0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
            0.5 * (-3.0 * t3 + 4.0 * t2 + t),
            0.5 * (t3 - t2),
        ]
    }
    let (width, height) = (image.width() as i64, image.height() as i64);
    let (base_x, base_y) = (x.floor(), y.floor());
    let weights_x = weights(x - base_x);
    let weights_y = weights(y - base_y);
    let mut sum = [0.0f32; 3];
    for (row, weight_y) in weights_y.iter().enumerate() {
        let sample_y = (base_y as i64 + row as i64 - 1).clamp(0, height - 1) as u32;
        for (column, weight_x) in weights_x.iter().enumerate() {
            let sample_x = (base_x as i64 + column as i64 - 1).clamp(0, width - 1) as u32;
            let pixel = image.get_pixel(sample_x, sample_y);
            for channel in 0..3 {
                sum[channel] += pixel[channel] * weight_x * weight_y;
            }
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn square_mask(x: u32, y: u32, edge: u32) -> RemoveMask {
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

    fn crop(x: u32, y: u32, edge: u32) -> NativeRect {
        NativeRect {
            x,
            y,
            width: edge,
            height: edge,
        }
    }

    #[test]
    fn native_pass_maps_pixels_one_to_one() {
        let geometry = PassGeometry::new(crop(1000, 2000, BIG_LAMA_INPUT_EDGE));
        assert_eq!(geometry.native_to_model(1000, 2000), [0.0, 0.0]);
        assert_eq!(geometry.native_to_model(1511, 2511), [511.0, 511.0]);
        assert_eq!(geometry.feather(), MIN_FEATHER);
    }

    #[test]
    fn downscaled_mask_keeps_every_object_pixel_masked() {
        // One native pixel per 4x4 block would vanish under nearest sampling.
        let geometry = PassGeometry::new(crop(0, 0, BIG_LAMA_INPUT_EDGE * 4));
        let speck = square_mask(1001, 1003, 1);
        let mask = geometry.model_mask(&speck);
        assert!(mask[geometry.model_index(1001, 1003)]);
        // Grown by the feather (6 native = 2 model pixels with margin).
        assert!(mask[geometry.model_index(1001 + 8, 1003)]);
        assert!(!mask[geometry.model_index(1001 + 40, 1003)]);
    }

    #[test]
    fn alpha_is_opaque_on_the_target_and_feathers_outside_it() {
        let geometry = PassGeometry::new(crop(0, 0, BIG_LAMA_INPUT_EDGE));
        let target = square_mask(100, 100, 20);
        let model_mask = geometry.model_mask(&target);
        let alpha = geometry.feather_alpha(&target, &model_mask);
        let at = |x: u32, y: u32| {
            alpha.pixels
                [((y - alpha.bounds.y) * alpha.bounds.width + (x - alpha.bounds.x)) as usize]
        };
        for y in 100..120 {
            for x in 100..120 {
                assert_eq!(at(x, y), 255, "target pixel {x},{y} is not fully replaced");
            }
        }
        assert!(at(99, 110) > 0 && at(99, 110) < 255);
        assert!(at(98, 110) < at(99, 110));
        assert!(alpha.bounds.x >= 100 - 3 && alpha.bounds.right() <= 120 + 3);
    }

    #[test]
    fn alpha_never_extends_beyond_generated_pixels() {
        let geometry = PassGeometry::new(crop(0, 0, BIG_LAMA_INPUT_EDGE));
        let target = square_mask(100, 100, 20);
        let model_mask = vec![false; (BIG_LAMA_INPUT_EDGE * BIG_LAMA_INPUT_EDGE) as usize];
        let alpha = geometry.feather_alpha(&target, &model_mask);
        assert!(alpha.pixels.is_empty());
    }

    #[test]
    fn catmull_rom_reproduces_pixels_and_linear_ramps() {
        let mut image = Rgb32FImage::new(8, 1);
        for x in 0..8 {
            image.put_pixel(x, 0, Rgb([x as f32 / 8.0; 3]));
        }
        assert!((sample_catmull_rom(&image, 3.0, 0.0)[0] - 3.0 / 8.0).abs() < 1e-6);
        assert!((sample_catmull_rom(&image, 3.5, 0.0)[0] - 3.5 / 8.0).abs() < 1e-6);
    }
}
