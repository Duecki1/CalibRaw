//! Uploading rasterized mask layers into the pipeline's mask atlases.

use super::*;

impl RawGpuPipeline {
    pub fn update_mask_layer(
        &self,
        queue: &wgpu::Queue,
        layer: usize,
        values: &[u16],
    ) -> Result<()> {
        self.check_mask_layer(layer, "local-mask")?;
        write_r16_mask_layer(
            queue,
            &self.mask_texture,
            layer,
            [self.mask_atlas_edge, self.mask_atlas_edge],
            values,
            "local-mask layer",
        )
    }

    pub fn update_mask_layer_region(
        &self,
        queue: &wgpu::Queue,
        layer: usize,
        width: u32,
        height: u32,
        values: &[u16],
    ) -> Result<()> {
        self.check_mask_layer(layer, "local-mask")?;
        if width == 0
            || height == 0
            || width > self.mask_atlas_edge
            || height > self.mask_atlas_edge
        {
            return Err(anyhow!(
                "local-mask region {width}x{height} exceeds {}x{} atlas",
                self.mask_atlas_edge,
                self.mask_atlas_edge
            ));
        }
        write_r16_mask_layer(
            queue,
            &self.mask_texture,
            layer,
            [width, height],
            values,
            "local-mask region",
        )
    }

    pub(crate) fn update_light_rays_mask_layer(
        &self,
        queue: &wgpu::Queue,
        layer: usize,
        values: &[u16],
    ) -> Result<()> {
        self.check_mask_layer(layer, "Light Rays mask")?;
        let edge = LIGHT_RAYS_MASK_ATLAS_EDGE;
        write_r16_mask_layer(
            queue,
            &self.light_rays_mask_texture,
            layer,
            [edge, edge],
            values,
            "Light Rays mask layer",
        )
    }

    /// Rejects array layers past the mask atlas capacity; `kind` names the atlas
    /// in the error.
    fn check_mask_layer(&self, layer: usize, kind: &str) -> Result<()> {
        if layer >= self.mask_layer_capacity {
            return Err(anyhow!(
                "{kind} layer {layer} exceeds atlas capacity {}",
                self.mask_layer_capacity
            ));
        }
        Ok(())
    }

    pub fn update_light_rays_mask_layers(
        &self,
        queue: &wgpu::Queue,
        masks: &MaskStack,
        image_width: u32,
        image_height: u32,
        lens_geometry: Option<&LensGeometryMap>,
    ) -> Result<()> {
        self.update_dirty_light_rays_mask_layers(
            queue,
            masks,
            image_width,
            image_height,
            lens_geometry,
            None,
        )
    }

    /// Refresh emission masks only for changed layers. Full uploads (including
    /// newly built pipelines and exports) pass `None` to initialize every layer.
    pub fn update_dirty_light_rays_mask_layers(
        &self,
        queue: &wgpu::Queue,
        masks: &MaskStack,
        image_width: u32,
        image_height: u32,
        lens_geometry: Option<&LensGeometryMap>,
        dirty_layers: Option<&[bool; MAX_LOCAL_MASKS]>,
    ) -> Result<()> {
        let edge = LIGHT_RAYS_MASK_ATLAS_EDGE;
        for (layer, mask) in masks
            .masks
            .iter()
            .take(self.mask_layer_capacity)
            .enumerate()
        {
            if dirty_layers.is_some_and(|dirty| !dirty[layer]) || !mask.has_light_rays_effect() {
                continue;
            }
            let values = masks.rasterize_layer_region_f16(
                layer,
                [edge, edge],
                [0, 0, image_width, image_height],
                [image_width, image_height],
                lens_geometry,
            );
            self.update_light_rays_mask_layer(queue, layer, &values)?;
        }
        Ok(())
    }
}

/// Writes `width`x`height` R16 samples (row-major, one `u16` per texel) to the
/// top-left corner of array layer `layer`. `name` labels the sample-count error.
fn write_r16_mask_layer(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    layer: usize,
    [width, height]: [u32; 2],
    values: &[u16],
    name: &str,
) -> Result<()> {
    let expected = width as usize * height as usize;
    if values.len() != expected {
        return Err(anyhow!(
            "{name} has {} samples, expected {expected}",
            values.len()
        ));
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: layer as u32,
            },
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(values),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 2),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    Ok(())
}
