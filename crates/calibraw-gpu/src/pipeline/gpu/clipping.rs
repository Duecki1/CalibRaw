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
        self.update_texture(device, queue, &source._out_view, shadows, highlights);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_clipping_shader_validates() {
        let module = naga::front::wgsl::parse_str(SHADER).expect("clipping WGSL parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("clipping WGSL validates");
    }

    #[test]
    fn preview_clipping_gpu_toggles_boundaries_and_preserves_source() -> Result<()> {
        let Some((device, queue)) = super::super::tests::request_test_device() else {
            eprintln!("preview clipping GPU regression skipped: no headless wgpu adapter");
            return Ok(());
        };
        let pixels: [[u8; 4]; 9] = [
            [0, 0, 0, 255],
            [255, 255, 255, 255],
            [255, 64, 32, 255],
            [32, 255, 64, 255],
            [32, 64, 255, 255],
            [0, 64, 32, 255],
            [1, 0, 0, 255],
            [254, 254, 254, 255],
            [128, 128, 128, 255],
        ];
        let texture = create_processing_texture(
            &device,
            wgpu::Extent3d {
                width: 9,
                height: 1,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            "clipping test source",
        );
        queue.write_texture(
            texture.as_image_copy(),
            bytemuck::cast_slice(&pixels),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(36),
                rows_per_image: Some(1),
            },
            texture.size(),
        );
        let gpu = PreviewClippingGpu::new(&device, 9, 1);
        let view = default_texture_view(&texture);
        let region = TextureReadbackRegion::full(9, 1, "clipping test readback");
        for (shadows, highlights) in [
            (false, false),
            (true, false),
            (false, true),
            (true, true),
            (false, false),
        ] {
            gpu.update_texture(&device, &queue, &view, shadows, highlights);
            let actual = read_rgba8_texture_region_blocking(&device, &queue, &gpu.texture, region)?;
            let mut expected = pixels;
            if shadows {
                expected[0] = [0, 0, 255, 255];
            }
            if highlights {
                expected[1..5].fill([255, 0, 0, 255]);
            }
            assert_eq!(
                actual,
                bytemuck::cast_slice::<_, u8>(&expected),
                "shadows={shadows}, highlights={highlights}"
            );
        }
        assert_eq!(
            read_rgba8_texture_region_blocking(&device, &queue, &texture, region)?,
            bytemuck::cast_slice::<_, u8>(&pixels),
        );
        // An edit can remove clipping while the same source texture is reused.
        queue.write_texture(
            texture.as_image_copy(),
            &[128; 36],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(36),
                rows_per_image: Some(1),
            },
            texture.size(),
        );
        gpu.update_texture(&device, &queue, &view, true, true);
        assert_eq!(
            read_rgba8_texture_region_blocking(&device, &queue, &gpu.texture, region)?,
            vec![128; 36]
        );
        Ok(())
    }
}
