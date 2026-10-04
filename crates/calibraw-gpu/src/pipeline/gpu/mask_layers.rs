//! Uploading rasterized mask layers into the pipeline's mask atlases.

use super::*;

impl RawGpuPipeline {
    pub fn update_mask_layer(
        &self,
        queue: &wgpu::Queue,
        layer: usize,
        values: &[u16],
    ) -> Result<()> {
        if layer >= self.mask_layer_capacity {
            return Err(anyhow!(
                "local-mask layer {layer} exceeds atlas capacity {}",
                self.mask_layer_capacity
            ));
        }
        let expected = self.mask_atlas_edge as usize * self.mask_atlas_edge as usize;
        if values.len() != expected {
            return Err(anyhow!(
                "local-mask layer has {} samples, expected {expected}",
                values.len()
            ));
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.mask_texture,
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
                bytes_per_row: Some(self.mask_atlas_edge * 2),
                rows_per_image: Some(self.mask_atlas_edge),
            },
            wgpu::Extent3d {
                width: self.mask_atlas_edge,
                height: self.mask_atlas_edge,
                depth_or_array_layers: 1,
            },
        );
        Ok(())
    }

    pub fn update_mask_layer_region(
        &self,
        queue: &wgpu::Queue,
        layer: usize,
        width: u32,
        height: u32,
        values: &[u16],
    ) -> Result<()> {
        if layer >= self.mask_layer_capacity {
            return Err(anyhow!(
                "local-mask layer {layer} exceeds atlas capacity {}",
                self.mask_layer_capacity
            ));
        }
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
        let expected = width as usize * height as usize;
        if values.len() != expected {
            return Err(anyhow!(
                "local-mask region has {} samples, expected {expected}",
                values.len()
            ));
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.mask_texture,
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

    pub(crate) fn update_light_rays_mask_layer(
        &self,
        queue: &wgpu::Queue,
        layer: usize,
        values: &[u16],
    ) -> Result<()> {
        if layer >= self.mask_layer_capacity {
            return Err(anyhow!(
                "Light Rays mask layer {layer} exceeds atlas capacity {}",
                self.mask_layer_capacity
            ));
        }
        let edge = LIGHT_RAYS_MASK_ATLAS_EDGE;
        let expected = edge as usize * edge as usize;
        if values.len() != expected {
            return Err(anyhow!(
                "Light Rays mask layer has {} samples, expected {expected}",
                values.len()
            ));
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.light_rays_mask_texture,
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
                bytes_per_row: Some(edge * 2),
                rows_per_image: Some(edge),
            },
            wgpu::Extent3d {
                width: edge,
                height: edge,
                depth_or_array_layers: 1,
            },
        );
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
