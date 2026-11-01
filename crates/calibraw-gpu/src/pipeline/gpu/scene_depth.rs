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

/// The depth result last uploaded, and whether its surface was.
pub(super) struct UploadedSceneDepth {
    depth: MaskImage,
    surface: bool,
    /// Keeps the shared surface levels (`SHARED_SURFACE`) alive while this
    /// pipeline shows them, so other pipelines reuse them.
    _levels: Arc<[EncodedLevel]>,
}

impl RawGpuPipeline {
    /// Uploads full-image scene depth, once an effect allocated its texture.
    /// Level 0 channel r holds the distance the scene map is built from.
    /// With `surface` the surface shading reads fills the other channels and the
    /// mip chain (`scene_surface`); without it they are not read, and level 0
    /// repeats the depth with flat gradients.
    pub(super) fn upload_scene_depth(
        &self,
        queue: &wgpu::Queue,
        depth: Option<&MaskImage>,
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
                self.invalidate_relight_shadow_map();
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
                && (previous.surface || !surface)
        }) {
            return;
        }

        let levels = encoded_scene_depth(depth, surface);
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
            surface,
            _levels: levels,
        });
        self.invalidate_relight_shadow_map();
        self.invalidate_scene_map();
    }

    /// The scene map's guided filter reads scene depth.
    fn invalidate_scene_map(&self) {
        self.full_image_map_current(FullImageMap::Scene)
            .store(false, Ordering::Release);
    }

    /// Shadows are traced through the scene-depth surface.
    fn invalidate_relight_shadow_map(&self) {
        *self
            .relight_shadow_map_key
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

/// The surface last derived, shared by every pipeline: the fitted,
/// navigation and zoomed previews and export tiles upload the same depth
/// result, and deriving it takes far longer than uploading it.
struct SharedSurface {
    /// Identifies the depth result without keeping it alive.
    pixels: Weak<[u8]>,
    width: u32,
    height: u32,
    /// Alive while a pipeline shows them (`UploadedSceneDepth`), so closing
    /// the photo frees them.
    levels: Weak<[EncodedLevel]>,
}

static SHARED_SURFACE: Mutex<Option<SharedSurface>> = Mutex::new(None);

/// The scene-depth texture's levels for `depth`: with `surface`, the stored
/// depth and surface with its mip chain, taken from the shared cache when
/// another pipeline already derived them; without, level 0 alone, repeating the
/// depth with flat gradients.
fn encoded_scene_depth(depth: &MaskImage, surface: bool) -> Arc<[EncodedLevel]> {
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
            && cached
                .pixels
                .upgrade()
                .is_some_and(|pixels| Arc::ptr_eq(&pixels, &depth.pixels))
    });
    if let Some(levels) = cached.and_then(|cached| cached.levels.upgrade()) {
        return levels;
    }
    // Derived once per depth result; the derivation is linear in the texel
    // count and has no per-edit inputs.
    let levels: Arc<[EncodedLevel]> = derive_scene_surface(
        &resampled_depth(depth),
        SCENE_DEPTH_EDGE,
        depth.width as f32 / depth.height as f32,
    )
    .iter()
    .map(encode_level)
    .collect();
    *shared = Some(SharedSurface {
        pixels: Arc::downgrade(&depth.pixels),
        width: depth.width,
        height: depth.height,
        levels: Arc::downgrade(&levels),
    });
    levels
}

/// Bilinear resample of `depth` to the `SCENE_DEPTH_EDGE` square, retaining
/// the full-image coordinate frame even for export tiles. Converted to float
/// before filtering so smooth depth ramps do not acquire another 8-bit
/// quantization step.
fn resampled_depth(depth: &MaskImage) -> Vec<f32> {
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
    let xs = axis(depth.width);
    let ys = axis(depth.height);
    let edge = SCENE_DEPTH_EDGE as usize;
    let mut stored = vec![0.0; edge * edge];
    stored
        .par_chunks_mut(edge)
        .zip(&ys)
        .for_each(|(row, &(y0, y1, fy))| {
            for (value, &(x0, x1, fx)) in row.iter_mut().zip(&xs) {
                let at = |x, y| depth.pixels[y * depth.width as usize + x] as f32 / 255.0;
                let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
                let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
                *value = top * (1.0 - fy) + bottom * fy;
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
