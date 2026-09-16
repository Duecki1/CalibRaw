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
        let extent = [
            width.clamp(1, SAMPLE_EDGE),
            height.clamp(1, SAMPLE_EDGE),
        ];
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
            &source._out_view,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_histogram_shader_validates() {
        let module = naga::front::wgsl::parse_str(SHADER).expect("histogram WGSL parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("histogram WGSL validates");
    }

    #[test]
    fn preview_histogram_sampling_is_bounded_and_respects_crop() {
        let geometry = GeometryTransform {
            crop: [0.25, 0.0, 0.75, 1.0],
            ..Default::default()
        };
        let sampling = Sampling::new(8000, 6000, geometry);
        assert_eq!(sampling.extent, [256, 256, 0, 0]);
        let map = GeometryInverseMap::new_with_lens(geometry, None, 8000, 6000, 256, 256);
        for (x, y) in [(0.0, 0.0), (17.0, 201.0), (255.0, 255.0)] {
            let expected = map.source_position(x, y);
            // The sampling grid only maps the two image axes; the affine
            // reconstruction has to match the exact inverse map on both.
            let actual = [
                sampling.origin[0] + x * sampling.step_x[0] + y * sampling.step_y[0],
                sampling.origin[1] + x * sampling.step_x[1] + y * sampling.step_y[1],
            ];
            for (actual, expected) in actual.into_iter().zip(expected.into_iter().take(2)) {
                assert!((actual - expected).abs() < 0.1);
            }
        }
    }

    #[test]
    fn preview_histogram_gpu_counts_channels_crop_and_clears_reused_bins() -> Result<()> {
        let Some((device, queue)) = super::super::tests::request_test_device() else {
            eprintln!("preview histogram GPU regression skipped: no headless wgpu adapter");
            return Ok(());
        };
        let texture = create_processing_texture(
            &device,
            wgpu::Extent3d {
                width: 6,
                height: 1,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            "histogram test texture",
        );
        queue.write_texture(
            texture.as_image_copy(),
            &[
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 128, 128, 128, 255, 0, 0, 0, 255,
                255, 255, 255, 255,
            ],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(24),
                rows_per_image: Some(1),
            },
            texture.size(),
        );
        let view = texture.create_view(&Default::default());
        let mut gpu = PreviewHistogramGpu::new(&device);
        let sampling = Sampling::new(6, 1, GeometryTransform::default());
        assert!(gpu.request_texture(&device, &queue, &view, sampling));
        assert!(!gpu.request_texture(&device, &queue, &view, sampling));
        device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })?;
        let histogram = gpu.poll(&device).expect("readback ready")?;
        for channel in &histogram.channels[..3] {
            assert_eq!(channel.iter().sum::<u32>(), 6);
            assert_eq!(channel[0], 3);
            assert_eq!(channel[128], 1);
            assert_eq!(channel[255], 2);
        }
        let luminance = &histogram.channels[3];
        assert_eq!(luminance.iter().sum::<u32>(), 6);
        for bin in [0, 76, 127, 128, 220, 255] {
            assert_eq!(luminance[bin], 1, "luminance bin {bin}");
        }

        // Crop to the white pixel, and reuse the same buffers. All old bins must clear.
        let crop = GeometryTransform {
            crop: [5.0 / 6.0, 0.0, 1.0, 1.0],
            ..Default::default()
        };
        assert!(gpu.request_texture(&device, &queue, &view, Sampling::new(6, 1, crop)));
        device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })?;
        let cropped = gpu.poll(&device).expect("cropped readback ready")?;
        for channel in &cropped.channels {
            assert_eq!(channel.iter().sum::<u32>(), 6);
            assert_eq!(channel[255], 6);
        }
        assert!(gpu.poll(&device).is_none());
        Ok(())
    }
}
