//! Scene context for remove/retouch edits rendered inside the pipeline.

use super::*;

#[derive(Clone, Copy)]
pub struct RemoveSceneContext<'a> {
    pub edits: &'a RemoveEditState,
    pub source_raw: &'a LoadedRaw,
    pub exposure: &'a ExposureParams,
    pub source_origin: [f32; 2],
    pub source_size: [f32; 2],
}

impl<'a> RemoveSceneContext<'a> {
    /// Remove edits applied to the whole of `source_raw`.
    pub fn full_frame(
        edits: &'a RemoveEditState,
        source_raw: &'a LoadedRaw,
        exposure: &'a ExposureParams,
    ) -> Self {
        Self::new(
            edits,
            source_raw,
            exposure,
            [0.0, 0.0],
            [source_raw.width as f32, source_raw.height as f32],
        )
    }

    pub const fn new(
        edits: &'a RemoveEditState,
        source_raw: &'a LoadedRaw,
        exposure: &'a ExposureParams,
        source_origin: [f32; 2],
        source_size: [f32; 2],
    ) -> Self {
        Self {
            edits,
            source_raw,
            exposure,
            source_origin,
            source_size,
        }
    }
}

fn remove_patch_coverage(patch: &RemovePatch, x: f32, y: f32) -> u8 {
    if patch.alpha.is_empty() || patch.bounds.width == 0 || patch.bounds.height == 0 {
        return 0;
    }
    let px = x
        .round()
        .clamp(0.0, patch.bounds.width.saturating_sub(1) as f32) as usize;
    let py = y
        .round()
        .clamp(0.0, patch.bounds.height.saturating_sub(1) as f32) as usize;
    patch.alpha[py * patch.bounds.width as usize + px]
}

fn sample_remove_patch_scene(patch: &RemovePatch, x: f32, y: f32) -> [f32; 3] {
    let width = patch.bounds.width as usize;
    let height = patch.bounds.height as usize;
    if width == 0 || height == 0 {
        return [0.0; 3];
    }
    let x = x.clamp(0.0, width.saturating_sub(1) as f32);
    let y = y.clamp(0.0, height.saturating_sub(1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let decode = |index: usize| {
        if patch.rgb_scene16f.len() == width * height * 3 {
            [
                half::f16::from_bits(patch.rgb_scene16f[index * 3]).to_f32(),
                half::f16::from_bits(patch.rgb_scene16f[index * 3 + 1]).to_f32(),
                half::f16::from_bits(patch.rgb_scene16f[index * 3 + 2]).to_f32(),
            ]
        } else {
            [0.0; 3]
        }
    };
    let a = decode(y0 * width + x0);
    let b = decode(y0 * width + x1);
    let c = decode(y1 * width + x0);
    let d = decode(y1 * width + x1);
    std::array::from_fn(|channel| {
        let top = a[channel] + (b[channel] - a[channel]) * tx;
        let bottom = c[channel] + (d[channel] - c[channel]) * tx;
        top + (bottom - top) * ty
    })
}

pub(super) fn dispatch_for_extent(width: u32, height: u32) -> [u32; 3] {
    [
        width.div_ceil(WORKGROUP_EDGE),
        height.div_ceil(WORKGROUP_EDGE),
        1,
    ]
}

impl RawGpuPipeline {
    pub fn upload_remove_scene_patches(
        &self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        remove: RemoveSceneContext<'_>,
    ) -> Result<()> {
        let RemoveSceneContext {
            edits,
            source_raw,
            exposure,
            source_origin,
            source_size,
        } = remove;
        if edits.is_empty() || source_size[0] <= 0.0 || source_size[1] <= 0.0 {
            return Ok(());
        }
        let scale_x = self.width as f32 / source_size[0];
        let scale_y = self.height as f32 / source_size[1];
        if !scale_x.is_finite() || !scale_y.is_finite() || scale_x <= 0.0 || scale_y <= 0.0 {
            return Ok(());
        }

        for stroke in &edits.strokes {
            let opacity = stroke.composite_opacity();
            if opacity.abs() <= f32::EPSILON {
                continue;
            }
            for patch in &stroke.patches {
                let left = (((patch.bounds.x as f32 - source_origin[0]) * scale_x).floor() as i64)
                    .clamp(0, self.width as i64) as u32;
                let top = (((patch.bounds.y as f32 - source_origin[1]) * scale_y).floor() as i64)
                    .clamp(0, self.height as i64) as u32;
                let right = (((patch.bounds.right() as f32 - source_origin[0]) * scale_x).ceil()
                    as i64)
                    .clamp(0, self.width as i64) as u32;
                let bottom = (((patch.bounds.bottom() as f32 - source_origin[1]) * scale_y).ceil()
                    as i64)
                    .clamp(0, self.height as i64) as u32;
                if right <= left || bottom <= top {
                    continue;
                }

                let output_width = right - left;
                let output_height = bottom - top;
                let sample = |x: u32, y: u32| {
                    let native_y = source_origin[1] + (y as f32 + 0.5) / scale_y;
                    let local_y = native_y - patch.bounds.y as f32 - 0.5;
                    let native_x = source_origin[0] + (x as f32 + 0.5) / scale_x;
                    let local_x = native_x - patch.bounds.x as f32 - 0.5;
                    if remove_patch_coverage(patch, local_x, local_y) == 0 {
                        return [0.0; 4];
                    }
                    let canonical = sample_remove_patch_scene(patch, local_x, local_y);
                    let source_scene =
                        canonical_remove_scene_to_pipeline_scene(source_raw, exposure, canonical);
                    let scene = if self.has_raster_scene {
                        pipeline_scene_to_working_rec2020(source_raw, source_scene)
                    } else {
                        source_scene
                    };
                    [scene[0], scene[1], scene[2], opacity]
                };
                let upload_origin = wgpu::Origin3d {
                    x: left,
                    y: top,
                    z: 0,
                };
                let upload_extent = wgpu::Extent3d {
                    width: output_width,
                    height: output_height,
                    depth_or_array_layers: 1,
                };
                match self.scene_format {
                    wgpu::TextureFormat::Rgba16Float => {
                        let mut rgba = Vec::<u16>::with_capacity(
                            output_width as usize * output_height as usize * 4,
                        );
                        for y in top..bottom {
                            for x in left..right {
                                for value in sample(x, y) {
                                    let finite = if value.is_finite() { value } else { 0.0 };
                                    rgba.push(
                                        half::f16::from_f32(finite.clamp(-65_504.0, 65_504.0))
                                            .to_bits(),
                                    );
                                }
                            }
                        }
                        queue.write_texture(
                            wgpu::TexelCopyTextureInfo {
                                texture: &self._tex1,
                                mip_level: 0,
                                origin: upload_origin,
                                aspect: wgpu::TextureAspect::All,
                            },
                            bytemuck::cast_slice(&rgba),
                            wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(output_width * 8),
                                rows_per_image: Some(output_height),
                            },
                            upload_extent,
                        );
                    }
                    wgpu::TextureFormat::Rgba32Float => {
                        let mut rgba = Vec::<f32>::with_capacity(
                            output_width as usize * output_height as usize * 4,
                        );
                        for y in top..bottom {
                            for x in left..right {
                                rgba.extend(sample(x, y));
                            }
                        }
                        queue.write_texture(
                            wgpu::TexelCopyTextureInfo {
                                texture: &self._tex1,
                                mip_level: 0,
                                origin: upload_origin,
                                aspect: wgpu::TextureAspect::All,
                            },
                            bytemuck::cast_slice(&rgba),
                            wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(output_width * 16),
                                rows_per_image: Some(output_height),
                            },
                            upload_extent,
                        );
                    }
                    format => {
                        return Err(anyhow!("unsupported Remove scene format {format:?}"));
                    }
                }
                queue.write_buffer(
                    &self.remove_composite_params_buffer,
                    0,
                    bytemuck::bytes_of(&RemoveCompositeParams {
                        origin: [left, top],
                        extent: [output_width, output_height],
                    }),
                );
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("calibraw Remove patch composite encoder"),
                });
                dispatch_compute(
                    &mut encoder,
                    "calibraw Remove patch composite",
                    &self.remove_composite_pipeline,
                    &[&self.remove_composite_bind_group],
                    dispatch_for_extent(output_width, output_height),
                );
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self._tex2,
                        mip_level: 0,
                        origin: upload_origin,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.scene_texture,
                        mip_level: 0,
                        origin: upload_origin,
                        aspect: wgpu::TextureAspect::All,
                    },
                    upload_extent,
                );
                queue.submit(Some(encoder.finish()));
            }
        }
        Ok(())
    }
}

pub(super) fn create_remove_composite_program(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    scene_view: &wgpu::TextureView,
    patch_view: &wgpu::TextureView,
    output_view: &wgpu::TextureView,
) -> Result<(wgpu::ComputePipeline, wgpu::BindGroup, wgpu::Buffer)> {
    let layout = create_bind_group_layout(
        device,
        "calibraw Remove composite layout",
        &[
            texture_entry(0, wgpu::TextureSampleType::Float { filterable: false }),
            texture_entry(1, wgpu::TextureSampleType::Float { filterable: false }),
            storage_texture_entry(2, format, wgpu::StorageTextureAccess::WriteOnly),
            buffer_entry(3),
        ],
    );
    let params_buffer = create_initialized_buffer(
        device,
        "calibraw Remove composite parameters",
        bytemuck::bytes_of(&RemoveCompositeParams {
            origin: [0; 2],
            extent: [0; 2],
        }),
        wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    );
    let bind_group = create_bind_group(
        device,
        "calibraw Remove composite bind group",
        &layout,
        &[
            texture_binding(0, scene_view),
            texture_binding(1, patch_view),
            texture_binding(2, output_view),
            buffer_binding(3, &params_buffer),
        ],
    );
    let shader = shaders::REMOVE_COMPOSITE_ENTRY;
    let source = work_shader_source(shader.source.text, format)?;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(shader.label),
        source: wgpu::ShaderSource::Wgsl(source),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("calibraw Remove composite pipeline layout"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = create_compute_pipeline(
        device,
        "calibraw Remove composite pipeline",
        &pipeline_layout,
        &module,
        "composite_remove_patch",
        None,
    );
    Ok((pipeline, bind_group, params_buffer))
}
