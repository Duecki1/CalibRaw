//! Uploading full-image scene depth and the surface derived from it
//! (`scene_surface`), the one depth every effect reads.
//!
//! The generated map is linear in the depth model's inverse depth i (stored = 1 - i), so
//! most of a scene lies near 1. Effects read relative distance instead, as fog builds
//! up, the ground runs and light falls off over distance, converted here alone. Depth
//! Range masks select on the map itself and are not affected.

use super::scene_surface::{derive_scene_surface, SurfaceLevel};
use super::*;
use rayon::prelude::*;
use std::sync::Weak;

pub(super) fn valid_scene_depth(depth: &MaskImage) -> bool {
    depth.width > 0
        && depth.height > 0
        && (depth.width as usize).checked_mul(depth.height as usize) == Some(depth.pixels.len())
}

/// How much of the inverse depth's range lies beyond the farthest surface, as distance
/// goes as 1 / (i + shift). Fitted to a distance-linear depth model's output for the
/// same photo.
const INVERSE_DEPTH_SHIFT: f32 = 0.115;

/// Relative distance of a stored depth, both normalized with near 0 and far 1.
pub(super) fn distance_from_stored(stored: f32) -> f32 {
    stored * INVERSE_DEPTH_SHIFT / (1.0 - stored + INVERSE_DEPTH_SHIFT)
}

/// Change of `distance_from_stored` per change of stored depth, to convert a gradient.
pub(super) fn distance_per_stored(stored: f32) -> f32 {
    let gap = 1.0 - stored + INVERSE_DEPTH_SHIFT;
    INVERSE_DEPTH_SHIFT * (1.0 + INVERSE_DEPTH_SHIFT) / (gap * gap)
}

/// The depth result last uploaded, the normals predicted with it, and whether
/// its surface was.
pub(super) struct UploadedSceneDepth {
    depth: MaskImage,
    normals: Option<SceneNormals>,
    surface: bool,
    /// Keeps the shared surface levels (`SHARED_SURFACE`) alive while this
    /// pipeline shows them, so other pipelines reuse them.
    _levels: Arc<[EncodedLevel]>,
}

impl RawGpuPipeline {
    /// Uploads full-image scene depth, once an effect allocated its texture.
    /// Level 0 channel r holds the distance the scene map is built from.
    /// With `surface` the surface shading reads fills the other channels and the
    /// mip chain (`scene_surface`), its gradients from `normals` where the depth
    /// has them; without it they are not read, and level 0 repeats the depth
    /// with flat gradients.
    pub(super) fn upload_scene_depth(
        &self,
        queue: &wgpu::Queue,
        depth: Option<&MaskImage>,
        normals: Option<&SceneNormals>,
        surface: bool,
    ) {
        let mut uploaded = self
            .uploaded_scene_depth
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(depth) = depth.filter(|depth| valid_scene_depth(depth)) else {
            // Only when depth goes away: the scene map built without depth stays
            // valid for every later render, and export tiles must keep the one
            // built from the whole image.
            if uploaded.take().is_some() {
                self.invalidate_scene_shadow_map();
                self.invalidate_scene_map();
            }
            return; // The uniform presence flag prevents sampling stale texture data.
        };
        // No effect reads depth yet: the allocation that first needs it comes
        // back here.
        let Some(target) = self.effect_inputs.scene_depth_texture() else {
            return;
        };
        // Upload once per depth result, not per slider movement; cloned stacks
        // share the immutable pixel allocation.
        if uploaded.as_ref().is_some_and(|previous| {
            previous.depth.width == depth.width
                && previous.depth.height == depth.height
                && Arc::ptr_eq(&previous.depth.pixels, &depth.pixels)
                && same_normals(previous.normals.as_ref(), normals)
                && (previous.surface || !surface)
        }) {
            return;
        }

        let levels = encoded_scene_depth(depth, normals, surface);
        for (mip_level, level) in levels.iter().enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &target,
                    mip_level: mip_level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&level.texels),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(level.width * 4 * 2),
                    rows_per_image: Some(level.height),
                },
                texture_size(level.width, level.height),
            );
        }
        *uploaded = Some(UploadedSceneDepth {
            depth: depth.clone(),
            normals: normals.cloned(),
            surface,
            _levels: levels,
        });
        self.invalidate_scene_shadow_map();
        self.invalidate_scene_map();
    }

    /// The scene map's guided filter reads scene depth.
    fn invalidate_scene_map(&self) {
        self.full_image_map_current(FullImageMap::Scene)
            .store(false, Ordering::Release);
    }

    /// Shadows are traced through the scene-depth surface.
    fn invalidate_scene_shadow_map(&self) {
        *self
            .scene_shadow_map_key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

/// One mip level of the scene-depth texture as half-float bits, four per texel.
struct EncodedLevel {
    width: u32,
    height: u32,
    texels: Vec<u16>,
}

/// Whether two uploads carry the same normals: the same result or none.
fn same_normals(a: Option<&SceneNormals>, b: Option<&SceneNormals>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            Arc::ptr_eq(&a.x().pixels, &b.x().pixels) && Arc::ptr_eq(&a.y().pixels, &b.y().pixels)
        }
        _ => false,
    }
}

/// The surface last derived, shared by every pipeline: the fitted,
/// navigation and zoomed previews and export tiles upload the same depth
/// result, and deriving it takes far longer than uploading it.
struct SharedSurface {
    /// Identifies the depth result without keeping it alive.
    pixels: Weak<[u8]>,
    /// Identifies the normals' x and y the same way, if there were normals.
    normals: Option<[Weak<[u8]>; 2]>,
    width: u32,
    height: u32,
    /// Alive while a pipeline shows them (`UploadedSceneDepth`), so closing
    /// the photo frees them.
    levels: Weak<[EncodedLevel]>,
}

static SHARED_SURFACE: Mutex<Option<SharedSurface>> = Mutex::new(None);

/// Whether `cached` identifies the allocation `pixels`.
fn still_holds(cached: &Weak<[u8]>, pixels: &Arc<[u8]>) -> bool {
    cached
        .upgrade()
        .is_some_and(|cached| Arc::ptr_eq(&cached, pixels))
}

/// The scene-depth texture's levels for `depth`: with `surface`, the stored
/// depth and surface with its mip chain, from `normals` where given, taken
/// from the shared cache when another pipeline already derived them; without,
/// level 0 alone, repeating the depth with flat gradients.
fn encoded_scene_depth(
    depth: &MaskImage,
    normals: Option<&SceneNormals>,
    surface: bool,
) -> Arc<[EncodedLevel]> {
    if !surface {
        let level = SurfaceLevel {
            width: SCENE_DEPTH_EDGE,
            height: SCENE_DEPTH_EDGE,
            texels: resampled_depth(depth)
                .iter()
                .map(|&stored| {
                    let distance = distance_from_stored(stored);
                    [distance, distance, 0.0, 0.0]
                })
                .collect(),
        };
        return Arc::from([encode_level(&level)]);
    }
    // Held while deriving, so pipelines asking for the same result together
    // derive it once.
    let mut shared = SHARED_SURFACE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cached = shared.as_ref().filter(|cached| {
        cached.width == depth.width
            && cached.height == depth.height
            && still_holds(&cached.pixels, &depth.pixels)
            && match (&cached.normals, normals) {
                (None, None) => true,
                (Some([x, y]), Some(normals)) => {
                    still_holds(x, &normals.x().pixels) && still_holds(y, &normals.y().pixels)
                }
                _ => false,
            }
    });
    if let Some(levels) = cached.and_then(|cached| cached.levels.upgrade()) {
        return levels;
    }
    // Derived once per depth result; the derivation is linear in the texel
    // count and has no per-edit inputs.
    let normals_resampled = normals.map(|normals| {
        let x = resampled(normals.x(), SceneNormals::decode);
        let y = resampled(normals.y(), SceneNormals::decode);
        x.into_iter()
            .zip(y)
            .map(|(x, y)| [x, y])
            .collect::<Vec<_>>()
    });
    let levels: Arc<[EncodedLevel]> = derive_scene_surface(
        &resampled_depth(depth),
        normals_resampled.as_deref(),
        SCENE_DEPTH_EDGE,
        depth.width as f32 / depth.height as f32,
    )
    .iter()
    .map(encode_level)
    .collect();
    *shared = Some(SharedSurface {
        pixels: Arc::downgrade(&depth.pixels),
        normals: normals.map(|normals| {
            [
                Arc::downgrade(&normals.x().pixels),
                Arc::downgrade(&normals.y().pixels),
            ]
        }),
        width: depth.width,
        height: depth.height,
        levels: Arc::downgrade(&levels),
    });
    levels
}

/// Stored depth of the sky, and of what lies beyond the depth model's far
/// limit. It converts to distance 1, which no nearer stored depth reaches
/// (254 converts to 0.963), and is resampled apart from nearer depth
/// (`resampled_depth`), so the shaders find it by value
/// (`SCENE_SKY_DISTANCE` in common.wgsl) and depth there never lies between
/// the sky and what stands in front of it.
const SKY_STORED: u8 = 255;

/// [`resampled`] for stored depth, with sky kept apart: a texel at least half
/// covered by sky is sky, at exactly 1; any other texel takes the depth of its
/// nearer samples alone. Texels without sky resample as `resampled` does.
fn resampled_depth(depth: &MaskImage) -> Vec<f32> {
    let decode = |stored: u8| f32::from(stored) / 255.0;
    resampled_with(depth, |taps| {
        let weights = taps.weights();
        let sky: f32 = taps
            .samples
            .iter()
            .zip(weights)
            .filter(|(stored, _)| **stored == SKY_STORED)
            .map(|(_, weight)| weight)
            .sum();
        if sky == 0.0 {
            return taps.bilinear(decode);
        }
        if sky >= 0.5 {
            return 1.0;
        }
        let (sum, total) = taps
            .samples
            .iter()
            .zip(weights)
            .filter(|(stored, _)| **stored != SKY_STORED)
            .fold((0.0, 0.0), |(sum, total), (&stored, weight)| {
                (sum + decode(stored) * weight, total + weight)
            });
        sum / total
    })
}

/// Bilinear resample of `image` to the `SCENE_DEPTH_EDGE` square, retaining
/// the full-image coordinate frame even for export tiles. Each value is
/// `decode`d to float before filtering so smooth depth ramps do not acquire
/// another 8-bit quantization step.
fn resampled(image: &MaskImage, decode: impl Fn(u8) -> f32 + Sync) -> Vec<f32> {
    resampled_with(image, |taps| taps.bilinear(&decode))
}

/// The four samples around a texel, top left, top right, bottom left and
/// bottom right, and how far the texel lies between them.
struct Taps {
    samples: [u8; 4],
    fx: f32,
    fy: f32,
}

impl Taps {
    fn weights(&self) -> [f32; 4] {
        let (fx, fy) = (self.fx, self.fy);
        [
            (1.0 - fx) * (1.0 - fy),
            fx * (1.0 - fy),
            (1.0 - fx) * fy,
            fx * fy,
        ]
    }

    fn bilinear(&self, decode: impl Fn(u8) -> f32) -> f32 {
        let [a, b, c, d] = self.samples.map(decode);
        let top = a * (1.0 - self.fx) + b * self.fx;
        let bottom = c * (1.0 - self.fx) + d * self.fx;
        top * (1.0 - self.fy) + bottom * self.fy
    }
}

/// Resamples `image` to the `SCENE_DEPTH_EDGE` square, each texel `combine`d
/// from its four [`Taps`].
fn resampled_with(image: &MaskImage, combine: impl Fn(&Taps) -> f32 + Sync) -> Vec<f32> {
    let axis = |extent: u32| {
        (0..SCENE_DEPTH_EDGE)
            .map(|i| {
                let p = ((i as f32 + 0.5) * extent as f32 / SCENE_DEPTH_EDGE as f32 - 0.5)
                    .clamp(0.0, (extent - 1) as f32);
                let lo = p.floor() as usize;
                (lo, (lo + 1).min(extent as usize - 1), p.fract())
            })
            .collect::<Vec<_>>()
    };
    let xs = axis(image.width);
    let ys = axis(image.height);
    let edge = SCENE_DEPTH_EDGE as usize;
    let mut stored = vec![0.0; edge * edge];
    stored
        .par_chunks_mut(edge)
        .zip(&ys)
        .for_each(|(row, &(y0, y1, fy))| {
            for (value, &(x0, x1, fx)) in row.iter_mut().zip(&xs) {
                let at = |x, y| image.pixels[y * image.width as usize + x];
                *value = combine(&Taps {
                    samples: [at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1)],
                    fx,
                    fy,
                });
            }
        });
    stored
}

fn encode_level(level: &SurfaceLevel) -> EncodedLevel {
    EncodedLevel {
        width: level.width,
        height: level.height,
        texels: level
            .texels
            .par_iter()
            .flat_map_iter(|texel| texel.map(|value| half::f16::from_f32(value).to_bits()))
            .collect(),
    }
}
