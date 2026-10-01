use super::*;

pub(super) fn valid_scene_depth(depth: &MaskImage) -> bool {
    depth.width > 0
        && depth.height > 0
        && (depth.width as usize).checked_mul(depth.height as usize) == Some(depth.pixels.len())
}

impl RawGpuPipeline {
    pub(super) fn upload_scene_depth(&self, queue: &wgpu::Queue, depth: Option<&MaskImage>) {
        let mut uploaded = self
            .uploaded_scene_depth
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(depth) = depth.filter(|depth| valid_scene_depth(depth)) else {
            *uploaded = None;
            return; // The uniform presence flag prevents sampling stale texture data.
        };
        if uploaded.as_ref().is_some_and(|previous| {
            previous.width == depth.width
                && previous.height == depth.height
                && Arc::ptr_eq(&previous.pixels, &depth.pixels)
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
        let mut values = Vec::with_capacity((SCENE_DEPTH_EDGE * SCENE_DEPTH_EDGE) as usize);
        for (y0, y1, fy) in ys {
            for &(x0, x1, fx) in &xs {
                let at = |x, y| depth.pixels[y * depth.width as usize + x] as f32 / 255.0;
                let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
                let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
                values.push(half::f16::from_f32(top * (1.0 - fy) + bottom * fy).to_bits());
            }
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.scene_depth_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&values),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(SCENE_DEPTH_EDGE * 2),
                rows_per_image: Some(SCENE_DEPTH_EDGE),
            },
            texture_size(SCENE_DEPTH_EDGE, SCENE_DEPTH_EDGE),
        );
        *uploaded = Some(depth.clone());
    }
}
