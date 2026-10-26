use super::*;

const SHADER: &str = include_str!("../../shaders/preview_clipping.wgsl");

/// A display-only composite; the processed source remains untouched for export and analysis.
pub struct PreviewClippingGpu {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    enabled: wgpu::Buffer,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl PreviewClippingGpu {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let layout = create_bind_group_layout(
            device,
            "preview clipping layout",
            &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                storage_texture_entry(
                    1,
                    wgpu::TextureFormat::Rgba8Unorm,
                    wgpu::StorageTextureAccess::WriteOnly,
                ),
                buffer_entry(2),
            ],
        );
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("preview clipping shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("preview clipping pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let texture = create_processing_texture(
            device,
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            "preview clipping composite",
        );
        let view = default_texture_view(&texture);
        Self {
            pipeline: create_compute_pipeline(
                device,
                "preview clipping",
                &pipeline_layout,
                &module,
                "preview_clipping",
                None,
            ),
            layout,
            enabled: create_gpu_buffer(
                device,
                "preview clipping toggles",
                16,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            texture,
            view,
        }
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn size(&self) -> [u32; 2] {
        [self.texture.width(), self.texture.height()]
    }

    pub fn update(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &RawGpuPipeline,
        shadows: bool,
        highlights: bool,
    ) {
        self.update_texture(device, queue, source.output_view(), shadows, highlights);
    }

    fn update_texture(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &wgpu::TextureView,
        shadows: bool,
        highlights: bool,
    ) {
        queue.write_buffer(
            &self.enabled,
            0,
            bytemuck::cast_slice(&[u32::from(shadows), u32::from(highlights), 0, 0]),
        );
        let group = create_bind_group(
            device,
            "preview clipping bind group",
            &self.layout,
            &[
                texture_binding(0, source),
                texture_binding(1, &self.view),
                buffer_binding(2, &self.enabled),
            ],
        );
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("preview clipping encoder"),
        });
        dispatch_compute(
            &mut encoder,
            "preview clipping",
            &self.pipeline,
            &[&group],
            [
                self.texture.width().div_ceil(16),
                self.texture.height().div_ceil(16),
                1,
            ],
        );
        queue.submit(Some(encoder.finish()));
    }
}
