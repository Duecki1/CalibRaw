use super::*;
use crate::pipeline::geometry::GeometryInverseMap;
use std::sync::mpsc::{self, Receiver, TryRecvError};

const HISTOGRAM_BYTES: u64 = 4 * 256 * std::mem::size_of::<u32>() as u64;
const SAMPLE_EDGE: u32 = 256;
const SHADER: &str = include_str!("../../shaders/preview_histogram.wgsl");

#[derive(Clone, Debug)]
pub struct PreviewHistogram {
    /// Red, green, blue, and display-encoded relative luminance; 256 bins each.
    pub channels: [[u32; 256]; 4],
}

/// Lazily created by the editor, reusing the pipeline's compute/buffer helpers.
/// Only one 4 KiB readback can be in flight; polling never waits for the GPU.
pub struct PreviewHistogramGpu {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    bins: wgpu::Buffer,
    readback: wgpu::Buffer,
    sampling: wgpu::Buffer,
    pending: Option<Receiver<std::result::Result<(), wgpu::BufferAsyncError>>>,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Sampling {
    origin: [f32; 4],
    step_x: [f32; 4],
    step_y: [f32; 4],
    extent: [u32; 4],
}

impl Sampling {
    fn new(width: u32, height: u32, geometry: GeometryTransform) -> Self {
        let extent = [width.clamp(1, SAMPLE_EDGE), height.clamp(1, SAMPLE_EDGE)];
        let map =
            GeometryInverseMap::new_with_lens(geometry, None, width, height, extent[0], extent[1]);
        let origin = map.source_position(0.0, 0.0);
        let x = map.source_position(1.0, 0.0);
        let y = map.source_position(0.0, 1.0);
        Self {
            origin: [origin[0], origin[1], 0.0, 0.0],
            step_x: [x[0] - origin[0], x[1] - origin[1], 0.0, 0.0],
            step_y: [y[0] - origin[0], y[1] - origin[1], 0.0, 0.0],
            extent: [extent[0], extent[1], 0, 0],
        }
    }
}

impl RawGpuPipeline {
    /// Changes whenever the displayed output is encoded, including original view.
    pub fn output_revision(&self) -> u64 {
        self.output_revision
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl PreviewHistogramGpu {
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = create_bind_group_layout(
            device,
            "preview histogram layout",
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
                storage_buffer_entry(1, false),
                buffer_entry(2),
            ],
        );
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("preview histogram shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("preview histogram pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        Self {
            pipeline: create_compute_pipeline(
                device,
                "preview histogram",
                &pipeline_layout,
                &module,
                "preview_histogram",
                None,
            ),
            layout,
            bins: create_gpu_buffer(
                device,
                "preview histogram bins",
                HISTOGRAM_BYTES,
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
            ),
            readback: create_gpu_buffer(
                device,
                "preview histogram readback",
                HISTOGRAM_BYTES,
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            ),
            sampling: create_gpu_buffer(
                device,
                "preview histogram sampling",
                std::mem::size_of::<Sampling>() as u64,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            pending: None,
        }
    }

    pub fn request(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &RawGpuPipeline,
        geometry: GeometryTransform,
    ) -> bool {
        self.request_texture(
            device,
            queue,
            source.output_view(),
            Sampling::new(source.width, source.height, geometry),
        )
    }

    fn request_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        sampling: Sampling,
    ) -> bool {
        if self.pending.is_some() {
            return false;
        }
        queue.write_buffer(&self.sampling, 0, bytemuck::bytes_of(&sampling));
        let group = create_bind_group(
            device,
            "preview histogram bind group",
            &self.layout,
            &[
                texture_binding(0, view),
                buffer_binding(1, &self.bins),
                buffer_binding(2, &self.sampling),
            ],
        );
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("preview histogram encoder"),
        });
        encoder.clear_buffer(&self.bins, 0, None);
        dispatch_compute(
            &mut encoder,
            "preview histogram",
            &self.pipeline,
            &[&group],
            [
                sampling.extent[0].div_ceil(16),
                sampling.extent[1].div_ceil(16),
                1,
            ],
        );
        encoder.copy_buffer_to_buffer(&self.bins, 0, &self.readback, 0, HISTOGRAM_BYTES);
        queue.submit(Some(encoder.finish()));
        let (sender, receiver) = mpsc::channel();
        self.readback
            .map_async(wgpu::MapMode::Read, .., move |result| {
                let _ = sender.send(result);
            });
        self.pending = Some(receiver);
        true
    }

    pub fn poll(&mut self, device: &wgpu::Device) -> Option<Result<PreviewHistogram>> {
        let receiver = self.pending.as_ref()?;
        let result = device
            .poll(wgpu::PollType::Poll)
            .map_err(|error| anyhow!(error));
        let result = match result {
            Err(error) => Err(error),
            Ok(_) => match receiver.try_recv() {
                Ok(result) => result.map_err(|error| anyhow!(error)),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => Err(anyhow!("histogram callback dropped")),
            },
        };
        self.pending = None;
        let result = result.map(|()| {
            let mapped = self.readback.get_mapped_range(..);
            let mut channels = [[0; 256]; 4];
            for (count, bytes) in channels.iter_mut().flatten().zip(mapped.chunks_exact(4)) {
                *count = u32::from_ne_bytes(bytes.try_into().expect("four-byte histogram bin"));
            }
            PreviewHistogram { channels }
        });
        self.readback.unmap();
        Some(result)
    }
}
