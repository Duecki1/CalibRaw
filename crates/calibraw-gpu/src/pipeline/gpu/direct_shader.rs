//! Runs a small compute shader that imports production WGSL modules, so
//! regression tests exercise the real shader functions instead of copies.

use super::{CfaKind, ShaderManager};
use crate::wgpu;

/// Runs the `main` entry point of `source` (workgroup size 64) over `input`.
///
/// The shader declares, in group 0, a 16-byte uniform at binding 40 holding
/// `params`, the read-only `array<vec4<f32>>` input at binding 41 and the
/// output array of the same length at binding 42. `file_name` names the source
/// in composer errors.
pub(super) fn run_direct_shader(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &'static str,
    file_name: &str,
    source: &str,
    params: [u32; 4],
    input: &[[f32; 4]],
) -> anyhow::Result<Vec<[f32; 4]>> {
    let mut manager = ShaderManager::new(wgpu::TextureFormat::Rgba32Float, CfaKind::Bayer)?;
    let shader = manager.create_shader_module(device, label, source, file_name)?;
    let size = (input.len() * 16) as u64;
    let input_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("direct shader input"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("direct shader output"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("direct shader readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("direct shader params"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&input_buffer, 0, bytemuck::cast_slice(input));
    queue.write_buffer(&params_buffer, 0, bytemuck::cast_slice(&params));

    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("direct shader layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 40,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 41,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 42,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("direct shader bind group"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 40,
                resource: params_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 41,
                resource: input_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 42,
                resource: output_buffer.as_entire_binding(),
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("direct shader pipeline layout"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("direct shader encoder"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("direct shader pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(input.len().div_ceil(64) as u32, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output_buffer, 0, &readback, 0, size);
    let submission = queue.submit(Some(encoder.finish()));
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    readback.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = sender.send(result);
    });
    device.poll(wgpu::PollType::Wait {
        submission_index: Some(submission),
        timeout: None,
    })?;
    receiver.recv()??;
    let bytes = readback.get_mapped_range(..);
    let output = bytemuck::cast_slice::<u8, [f32; 4]>(&bytes).to_vec();
    drop(bytes);
    readback.unmap();
    Ok(output)
}
