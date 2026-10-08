//! Texture and buffer allocation for a pipeline: derived geometry, surfaces and buffers.

use super::*;

pub(in crate::pipeline::gpu) struct DerivedGeometry {
    pub(in crate::pipeline::gpu) size: wgpu::Extent3d,
    pub(in crate::pipeline::gpu) tone_size: wgpu::Extent3d,
    pub(in crate::pipeline::gpu) work_format: wgpu::TextureFormat,
    pub(in crate::pipeline::gpu) demosaic_format: wgpu::TextureFormat,
    pub(in crate::pipeline::gpu) highlight_work_format: wgpu::TextureFormat,
    pub(in crate::pipeline::gpu) tone_format: wgpu::TextureFormat,
    pub(in crate::pipeline::gpu) image_workgroups: [u32; 3],
    pub(in crate::pipeline::gpu) tone_workgroups: [u32; 3],
    pub(in crate::pipeline::gpu) mask_atlas_edge: u32,
    pub(in crate::pipeline::gpu) mask_layer_capacity: usize,
}

pub(in crate::pipeline::gpu) fn compute_derived_geometry(
    raw: &LoadedRaw,
    params: &GpuParams,
    quality: ProcessingQuality,
    config: RawGpuPipelineConfig,
) -> DerivedGeometry {
    let size = texture_size(raw.width, raw.height);
    let work_format = processing_work_format(quality);
    let demosaic_format = work_format;
    let highlight_work_format = work_format;
    let tone_scale = tone_analysis_scale();
    let tone_size = texture_size(
        tone_guide_axis_cell_count(params.camera.tile_origin_x, raw.width, tone_scale),
        tone_guide_axis_cell_count(params.camera.tile_origin_y, raw.height, tone_scale),
    );
    let tone_format = tone_guide_format();
    let image_workgroups = dispatch_for_extent(raw.width, raw.height);
    let tone_workgroups = dispatch_for_extent(tone_size.width, tone_size.height);

    let mask_atlas_edge = config
        .mask_atlas_edge_override
        .unwrap_or_else(|| interactive_mask_atlas_edge(raw.width, raw.height))
        .clamp(64, export_mask_atlas_edge_limit());
    let mask_layer_capacity = if config.mask_atlas_edge_override.is_some() {
        (params.scene_tone.mask_counts[0] as usize).clamp(1, MAX_LOCAL_MASKS)
    } else {
        MAX_LOCAL_MASKS
    };

    DerivedGeometry {
        size,
        tone_size,
        work_format,
        demosaic_format,
        highlight_work_format,
        tone_format,
        image_workgroups,
        tone_workgroups,
        mask_atlas_edge,
        mask_layer_capacity,
    }
}

pub(in crate::pipeline::gpu) struct PipelineSurfaces {
    pub(in crate::pipeline::gpu) raw_texture: wgpu::Texture,
    pub(in crate::pipeline::gpu) color_texture: wgpu::Texture,
    pub(in crate::pipeline::gpu) black_texture: wgpu::Texture,
    pub(in crate::pipeline::gpu) reconstructed_raw_texture: wgpu::Texture,
    pub(in crate::pipeline::gpu) highlight_work_a: wgpu::Texture,
    pub(in crate::pipeline::gpu) highlight_work_b: wgpu::Texture,
    pub(in crate::pipeline::gpu) scene_texture: wgpu::Texture,
    pub(in crate::pipeline::gpu) display_linear_texture: wgpu::Texture,
    pub(in crate::pipeline::gpu) out_texture: wgpu::Texture,
    pub(in crate::pipeline::gpu) tex1: wgpu::Texture,
    pub(in crate::pipeline::gpu) tex2: wgpu::Texture,
    pub(in crate::pipeline::gpu) tone_guide_a: wgpu::Texture,
    pub(in crate::pipeline::gpu) tone_guide_b: wgpu::Texture,
    pub(in crate::pipeline::gpu) mask_texture: wgpu::Texture,
    pub(in crate::pipeline::gpu) light_rays_mask_texture: wgpu::Texture,
    pub(in crate::pipeline::gpu) scene_depth_texture: wgpu::Texture,
    pub(in crate::pipeline::gpu) out_view: wgpu::TextureView,
    pub(in crate::pipeline::gpu) display_linear_view: wgpu::TextureView,
    pub(in crate::pipeline::gpu) reconstructed_raw_view: wgpu::TextureView,
    pub(in crate::pipeline::gpu) highlight_work_a_view: wgpu::TextureView,
    pub(in crate::pipeline::gpu) highlight_work_b_view: wgpu::TextureView,
    pub(in crate::pipeline::gpu) scene_view: wgpu::TextureView,
    pub(in crate::pipeline::gpu) tex1_view: wgpu::TextureView,
    pub(in crate::pipeline::gpu) tex2_view: wgpu::TextureView,
    pub(in crate::pipeline::gpu) tone_guide_a_view: wgpu::TextureView,
    pub(in crate::pipeline::gpu) tone_guide_b_view: wgpu::TextureView,
    pub(in crate::pipeline::gpu) raw_view: wgpu::TextureView,
    pub(in crate::pipeline::gpu) color_view: wgpu::TextureView,
    pub(in crate::pipeline::gpu) black_view: wgpu::TextureView,
    pub(in crate::pipeline::gpu) mask_view: wgpu::TextureView,
    pub(in crate::pipeline::gpu) light_rays_mask_view: wgpu::TextureView,
    pub(in crate::pipeline::gpu) scene_depth_view: wgpu::TextureView,
    pub(in crate::pipeline::gpu) mask_sampler: wgpu::Sampler,
}

pub(in crate::pipeline::gpu) fn create_pipeline_surfaces(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    raw: &LoadedRaw,
    ai_cfa: Option<&[u16]>,
    geometry: &DerivedGeometry,
) -> Result<(PipelineSurfaces, bool)> {
    let DerivedGeometry {
        size,
        tone_size,
        work_format,
        demosaic_format,
        highlight_work_format,
        tone_format,
        mask_atlas_edge,
        mask_layer_capacity,
        ..
    } = *geometry;

    let raw_texture = create_raw_texture(
        device,
        queue,
        raw,
        ai_cfa.unwrap_or(raw.raw_pixels.as_slice()),
    );
    let color_texture = create_color_texture(device, queue, raw);
    let black_texture = create_black_texture(device, queue, raw);

    let reconstructed_raw_texture = create_processing_texture(
        device,
        size,
        wgpu::TextureFormat::R32Float,
        wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        "calibraw reconstructed raw CFA",
    );

    let highlight_work_a = create_float_work_texture(
        device,
        size,
        highlight_work_format,
        "calibraw highlight work A",
    );
    let highlight_work_b = create_float_work_texture(
        device,
        size,
        highlight_work_format,
        "calibraw highlight work B",
    );

    let scene_texture = create_demosaic_texture(
        device,
        size,
        demosaic_format,
        "calibraw scene-linear camera RGB",
    );
    let has_ai_scene = upload_ai_scene_texture(queue, &scene_texture, demosaic_format, raw)?;

    let display_linear_texture = create_demosaic_texture(
        device,
        size,
        work_format,
        "calibraw display-linear Rec.2020",
    );

    let out_texture = create_processing_texture(
        device,
        size,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        "calibraw output texture",
    );

    let tex1 = create_demosaic_texture(device, size, demosaic_format, "calibraw tex1");
    let tex2 = create_demosaic_texture(device, size, demosaic_format, "calibraw tex2");
    let tone_guide_a = create_tone_guide_texture(
        device,
        tone_size,
        tone_format,
        "calibraw adaptive tone guide A",
    );
    let tone_guide_b = create_tone_guide_texture(
        device,
        tone_size,
        tone_format,
        "calibraw adaptive tone guide B",
    );
    let mask_texture = create_processing_texture_array(
        device,
        mask_atlas_edge,
        mask_atlas_edge,
        mask_layer_capacity as u32,
        wgpu::TextureFormat::R16Float,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        "calibraw normalized local-mask atlas",
    );
    let light_rays_mask_texture = create_processing_texture_array(
        device,
        LIGHT_RAYS_MASK_ATLAS_EDGE,
        LIGHT_RAYS_MASK_ATLAS_EDGE,
        mask_layer_capacity as u32,
        wgpu::TextureFormat::R16Float,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        "calibraw full-image Light Rays emission atlas",
    );

    // Stored depth that Fog and Smoke read. Relight's surface replaces it
    // once allocated (`effect_inputs`).
    let scene_depth_texture = create_processing_texture(
        device,
        texture_size(SCENE_DEPTH_EDGE, SCENE_DEPTH_EDGE),
        SCENE_DEPTH_FORMAT,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        "calibraw full-image scene depth",
    );
    let scene_depth_view = default_texture_view(&scene_depth_texture);

    let out_view = default_texture_view(&out_texture);
    let display_linear_view = default_texture_view(&display_linear_texture);
    let reconstructed_raw_view = default_texture_view(&reconstructed_raw_texture);
    let highlight_work_a_view = default_texture_view(&highlight_work_a);
    let highlight_work_b_view = default_texture_view(&highlight_work_b);
    let scene_view = default_texture_view(&scene_texture);
    let tex1_view = default_texture_view(&tex1);
    let tex2_view = default_texture_view(&tex2);
    let tone_guide_a_view = default_texture_view(&tone_guide_a);
    let tone_guide_b_view = default_texture_view(&tone_guide_b);
    let raw_view = default_texture_view(&raw_texture);
    let color_view = default_texture_view(&color_texture);
    let black_view = default_texture_view(&black_texture);
    let mask_view = mask_texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("calibraw local-mask array view"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let light_rays_mask_view = light_rays_mask_texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("calibraw full-image Light Rays mask array view"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let mask_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("calibraw local-mask linear sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        // Single-level mask atlases are unaffected; Relight's scene-depth
        // surface blends its mip levels for broad light sizes.
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    });

    Ok((
        PipelineSurfaces {
            raw_texture,
            color_texture,
            black_texture,
            reconstructed_raw_texture,
            highlight_work_a,
            highlight_work_b,
            scene_texture,
            display_linear_texture,
            out_texture,
            tex1,
            tex2,
            tone_guide_a,
            tone_guide_b,
            mask_texture,
            light_rays_mask_texture,
            scene_depth_texture,
            out_view,
            display_linear_view,
            reconstructed_raw_view,
            highlight_work_a_view,
            highlight_work_b_view,
            scene_view,
            tex1_view,
            tex2_view,
            tone_guide_a_view,
            tone_guide_b_view,
            raw_view,
            color_view,
            black_view,
            mask_view,
            light_rays_mask_view,
            scene_depth_view,
            mask_sampler,
        },
        has_ai_scene,
    ))
}

pub(in crate::pipeline::gpu) struct PipelineBuffers {
    pub(in crate::pipeline::gpu) profile_buffer: wgpu::Buffer,
    pub(in crate::pipeline::gpu) camera_uniforms_buffer: wgpu::Buffer,
    pub(in crate::pipeline::gpu) scene_tone_uniforms_buffer: wgpu::Buffer,
    pub(in crate::pipeline::gpu) effects_uniforms_buffer: wgpu::Buffer,
    pub(in crate::pipeline::gpu) mask_data_buffer: wgpu::Buffer,
    pub(in crate::pipeline::gpu) tone_histogram_buffer: wgpu::Buffer,
    pub(in crate::pipeline::gpu) tone_stats_buffer: wgpu::Buffer,
}

pub(in crate::pipeline::gpu) fn create_pipeline_buffers(
    device: &wgpu::Device,
    params: &GpuParams,
    profile_words: &[[f32; 4]],
) -> PipelineBuffers {
    let profile_buffer = create_initialized_buffer(
        device,
        "calibraw DCP and sRGB output LUTs",
        bytemuck::cast_slice(profile_words),
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    );

    let camera_uniforms_buffer = create_initialized_buffer(
        device,
        "calibraw camera uniforms",
        params.camera_bytes(),
        wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    );
    let scene_tone_uniforms_buffer = create_initialized_buffer(
        device,
        "calibraw scene-tone uniforms",
        params.scene_tone_bytes(),
        wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    );
    let effects_uniforms_buffer = create_initialized_buffer(
        device,
        "calibraw effects uniforms",
        params.effects_bytes(),
        wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    );
    let mask_data_buffer = create_initialized_buffer(
        device,
        "calibraw local-mask data",
        params.mask_data_bytes(),
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    );

    let tone_histogram_buffer = create_gpu_buffer(
        device,
        "calibraw tone histogram",
        u64::from(TONE_HISTOGRAM_BIN_COUNT) * std::mem::size_of::<u32>() as u64,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    );
    let tone_stats_buffer = create_gpu_buffer(
        device,
        "calibraw tone statistics",
        TONE_STATS_SIZE_BYTES,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
    );

    PipelineBuffers {
        profile_buffer,
        camera_uniforms_buffer,
        scene_tone_uniforms_buffer,
        effects_uniforms_buffer,
        mask_data_buffer,
        tone_histogram_buffer,
        tone_stats_buffer,
    }
}
