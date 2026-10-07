use super::scene_surface::{derive_scene_surface, SurfaceLevel};
use super::*;

pub(super) fn valid_scene_depth(depth: &MaskImage) -> bool {
    depth.width > 0
        && depth.height > 0
        && (depth.width as usize).checked_mul(depth.height as usize) == Some(depth.pixels.len())
}

/// The depth result last uploaded, and whether its relighting surface was.
pub(super) struct UploadedSceneDepth {
    depth: MaskImage,
    surface: bool,
}

impl RawGpuPipeline {
    /// Uploads full-image scene depth. Channel r of level 0 is the stored depth
    /// that fog reads. With `surface`, the relighting surface fills the other
    /// channels and the mip chain (`scene_surface`); without it they are not
    /// read, and level 0 repeats the depth with flat gradients.
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
            *uploaded = None;
            return; // The uniform presence flag prevents sampling stale texture data.
        };
        if uploaded.as_ref().is_some_and(|previous| {
            previous.depth.width == depth.width
                && previous.depth.height == depth.height
                && Arc::ptr_eq(&previous.depth.pixels, &depth.pixels)
                && (previous.surface || !surface)
        }) {
            return;
        }

        // Retain the full-image coordinate frame even for export tiles. Convert
        // to half float before filtering so smooth depth ramps do not acquire
        // another 8-bit quantization step. Upload once per depth result, not per
        // slider movement; cloned stacks share the immutable pixel allocation.
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
        let mut stored = Vec::with_capacity((SCENE_DEPTH_EDGE * SCENE_DEPTH_EDGE) as usize);
        for (y0, y1, fy) in ys {
            for &(x0, x1, fx) in &xs {
                let at = |x, y| depth.pixels[y * depth.width as usize + x] as f32 / 255.0;
                let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
                let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
                stored.push(top * (1.0 - fy) + bottom * fy);
            }
        }
        let levels = if surface {
            // Derived once per depth result; the derivation is linear in the
            // texel count and has no per-edit inputs.
            derive_scene_surface(
                &stored,
                SCENE_DEPTH_EDGE,
                depth.width as f32 / depth.height as f32,
            )
        } else {
            vec![SurfaceLevel {
                width: SCENE_DEPTH_EDGE,
                height: SCENE_DEPTH_EDGE,
                texels: stored
                    .iter()
                    .map(|&value| [value, value, 0.0, 0.0])
                    .collect(),
            }]
        };
        for (mip_level, level) in levels.iter().enumerate() {
            let values: Vec<u16> = level
                .texels
                .iter()
                .flatten()
                .map(|&value| half::f16::from_f32(value).to_bits())
                .collect();
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.scene_depth_texture,
                    mip_level: mip_level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&values),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(level.width * 8),
                    rows_per_image: Some(level.height),
                },
                texture_size(level.width, level.height),
            );
        }
        *uploaded = Some(UploadedSceneDepth {
            depth: depth.clone(),
            surface,
        });
    }
}
