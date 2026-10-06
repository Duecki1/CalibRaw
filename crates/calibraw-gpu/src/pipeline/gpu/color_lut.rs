//! The colour look's 3D texture: a fixed-size cube the active table is
//! resampled into, so changing tables never reallocates or rebuilds bind groups.

use super::*;

impl RawGpuPipeline {
    pub(super) fn upload_color_lut(&self, queue: &wgpu::Queue, lut: Option<&Arc<ColorLut>>) {
        let mut uploaded = self
            .uploaded_color_lut
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(lut) = lut else {
            *uploaded = None;
            return; // The zero mix keeps the shader from sampling stale texels.
        };
        if uploaded
            .as_ref()
            .is_some_and(|previous| Arc::ptr_eq(previous, lut) || **previous == **lut)
        {
            return;
        }
        let texels = lut.gpu_texels(COLOR_LUT_EDGE);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.color_lut_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&texels),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(COLOR_LUT_EDGE * 8),
                rows_per_image: Some(COLOR_LUT_EDGE),
            },
            wgpu::Extent3d {
                width: COLOR_LUT_EDGE,
                height: COLOR_LUT_EDGE,
                depth_or_array_layers: COLOR_LUT_EDGE,
            },
        );
        *uploaded = Some(Arc::clone(lut));
    }
}
