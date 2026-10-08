//! Building a `RawGpuPipeline` and compiling or prewarming its programs.

use super::*;

impl RawGpuPipeline {
    pub fn program_template(&self) -> RawGpuProgramTemplate {
        RawGpuProgramTemplate {
            cfa_kind: self.cfa_kind,
            processing_quality: self.processing_quality,
            pipelines: self
                .passes
                .iter()
                .map(|pass| pass.pipeline.clone())
                .collect(),
            pipeline_cache: self.pipeline_cache.clone(),
        }
    }

    fn into_program_template(self) -> RawGpuProgramTemplate {
        RawGpuProgramTemplate {
            cfa_kind: self.cfa_kind,
            processing_quality: self.processing_quality,
            pipelines: self.passes.into_iter().map(|pass| pass.pipeline).collect(),
            pipeline_cache: self.pipeline_cache,
        }
    }

    pub fn prewarm_preview_template_with_cache(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cfa_kind: CfaKind,
        pipeline_cache: Option<Arc<PersistentGpuPipelineCache>>,
    ) -> Result<Self> {
        Self::prewarm_template_with_quality_and_cache(
            device,
            queue,
            cfa_kind,
            ProcessingQuality::Preview,
            pipeline_cache,
            None,
        )
    }

    pub fn prewarm_export_program_template_with_cache(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cfa_kind: CfaKind,
        pipeline_cache: Option<Arc<PersistentGpuPipelineCache>>,
    ) -> Result<RawGpuProgramTemplate> {
        Self::prewarm_template_with_quality_and_cache(
            device,
            queue,
            cfa_kind,
            ProcessingQuality::High,
            pipeline_cache,
            Some(64),
        )
        .map(Self::into_program_template)
    }

    fn prewarm_template_with_quality_and_cache(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cfa_kind: CfaKind,
        quality: ProcessingQuality,
        pipeline_cache: Option<Arc<PersistentGpuPipelineCache>>,
        mask_atlas_edge_override: Option<u32>,
    ) -> Result<Self> {
        const EDGE: u32 = 16;
        let pixels = (EDGE * EDGE) as usize;
        let cfa_pattern = match cfa_kind {
            CfaKind::Bayer => vec![0u8, 1, 1, 2],
            CfaKind::XTrans => vec![
                0u8, 1, 0, 0, 1, 0, 1, 2, 1, 2, 1, 2, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0, 1, 2, 1,
                2, 1, 2, 0, 1, 0, 0, 1, 0,
            ],
        };
        let cfa_period = match cfa_kind {
            CfaKind::Bayer => (2, 2),
            CfaKind::XTrans => (6, 6),
        };
        let raw = LoadedRaw {
            width: EDGE,
            height: EDGE,
            camera_make: "CalibRaw".to_owned(),
            camera_model: "GPU prewarm".to_owned(),
            lens_make: String::new(),
            lens_model: String::new(),
            focal_length: 0.0,
            aperture: 0.0,
            focus_distance: 0.0,
            capture_metadata: Default::default(),
            cfa_kind,
            raw_pixels: vec![0u16; pixels],
            scene_linear_raster: None,
            color_indices: crate::pipeline::CompactPixelMap::repeating(
                EDGE,
                EDGE,
                cfa_period.0,
                cfa_period.1,
                cfa_pattern,
            ),
            wb_coeffs: [1.0; 4],
            cam_to_srgb: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
            black_levels: [0.0; 4],
            black_levels_per_pixel: crate::pipeline::CompactPixelMap::repeating(
                EDGE,
                EDGE,
                1,
                1,
                vec![0.0f32],
            ),
            white_levels: [65535.0; 4],
            noise_profile: crate::pipeline::NoiseProfile::default(),
            camera_profile: crate::pipeline::CameraProfile::default(),
            camera_profile_source: None,
            available_camera_profiles: Vec::new(),
            white_balance_model: None,
            lens_geometry: None,
            ai_denoised: Arc::new(std::sync::RwLock::new(None)),
            opposed_chroma_cache: Default::default(),
            opposed_chroma_source_identity: Default::default(),
            opposed_chroma_reference_source: true,
        };
        let exposure = ExposureParams::scene_referred_default();
        let masks = MaskStack::default();
        let params = GpuParams::new(&exposure, &masks, &raw);
        Self::new_internal(RawGpuPipelineBuild {
            device,
            queue,
            program_template: None,
            pipeline_cache,
            raw: &raw,
            params: &params,
            quality,
            config: RawGpuPipelineConfig {
                mask_atlas_edge_override,
            },
        })
    }

    /// Builds the processing graph for `raw`. Nothing is presented; register
    /// [`RawGpuPipeline::output_view`] to display the output.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        raw: &LoadedRaw,
        params: &GpuParams,
        options: PipelineOptions<'_>,
    ) -> Result<Self> {
        Self::new_internal(RawGpuPipelineBuild {
            device,
            queue,
            program_template: options.programs,
            pipeline_cache: options
                .programs
                .and_then(|template| template.pipeline_cache.clone()),
            raw,
            params,
            quality: options.quality,
            config: RawGpuPipelineConfig {
                mask_atlas_edge_override: options.mask_atlas_edge,
            },
        })
    }

    fn new_internal(build: RawGpuPipelineBuild<'_>) -> Result<Self> {
        let RawGpuPipelineBuild {
            device,
            queue,
            program_template,
            pipeline_cache,
            raw,
            params,
            quality,
            config,
        } = build;
        validate_raw(raw)?;
        if let Some(template) = program_template {
            if template.cfa_kind != raw.cfa_kind
                || template.processing_quality != quality
                || template.pipelines.len() != expected_pass_count(raw.cfa_kind)
            {
                return Err(anyhow!(
                    "cannot reuse GPU programs from an incompatible pipeline"
                ));
            }
        }

        let geometry = compute_derived_geometry(raw, params, quality, config);

        let profile_gpu_data = raw.camera_profile.gpu_data();
        profile_gpu_data.validate()?;
        let profile_buffer_size_bytes = u64::try_from(
            profile_gpu_data
                .words
                .len()
                .checked_mul(std::mem::size_of::<[f32; 4]>())
                .ok_or_else(|| anyhow!("GPU profile buffer size overflows"))?,
        )
        .map_err(|_| anyhow!("GPU profile buffer size does not fit in u64"))?;

        let resource_plan = build_gpu_resource_plan(GpuResourcePlanInput {
            width: raw.width,
            height: raw.height,
            quality,
            tone_scale: tone_analysis_scale(),
            mask_atlas_edge: geometry.mask_atlas_edge,
            mask_layers: u32::try_from(geometry.mask_layer_capacity)
                .map_err(|_| anyhow!("mask layer capacity does not fit in u32"))?,
            profile_buffer_bytes: profile_buffer_size_bytes,
            stage_uniform_buffer_bytes: GPU_STAGE_UNIFORM_ALLOCATION_BYTES,
            mask_data_buffer_bytes: MASK_DATA_SIZE_BYTES,
        })?;
        let gpu_budget_reservation =
            GpuBudgetReservation::acquire(&resource_plan, gpu_working_set_limit_bytes())?;

        let gpu_error_scopes = GpuErrorScopes::push(device);

        let ai_image = raw.ai_denoised_image();
        let ai_cfa = params
            .uses_ai_denoise()
            .then(|| ai_image.as_ref().and_then(AiDenoisedImage::bayer_cfa))
            .flatten();
        let has_ai_cfa = ai_cfa.is_some();
        let (surfaces, has_ai_scene) =
            create_pipeline_surfaces(device, queue, raw, ai_cfa, &geometry)?;
        let has_raster_scene = raw.is_pre_demosaiced_raster();

        let buffers = create_pipeline_buffers(device, params, &profile_gpu_data.words);

        let layouts = create_bind_group_layouts(
            device,
            program_template,
            raw.cfa_kind,
            geometry.demosaic_format,
            geometry.work_format,
            geometry.tone_format,
        );
        let groups = create_bind_groups(device, &layouts, &buffers, &surfaces, raw.cfa_kind);

        let shaders = load_shader_set(
            device,
            program_template.is_some(),
            raw.cfa_kind,
            geometry.demosaic_format,
            geometry.work_format,
        )?;

        let assembled = assemble_passes(
            device,
            program_template,
            pipeline_cache.as_ref(),
            &layouts,
            &groups,
            &shaders,
            raw.cfa_kind,
            geometry.image_workgroups,
            geometry.tone_workgroups,
        )?;
        let AssembledPasses {
            passes,
            post_blur_glow_passes,
            post_blur_pixelate_blocks_pass,
            post_blur_creative_pass,
            post_blur_render_pass,
            indices,
        } = assembled;

        let (
            remove_composite_pipeline,
            remove_composite_bind_group,
            remove_composite_params_buffer,
        ) = create_remove_composite_program(
            device,
            geometry.demosaic_format,
            &surfaces.scene_view,
            &surfaces.tex1_view,
            &surfaces.tex2_view,
        )?;

        let pipeline = Self {
            output_revision: std::sync::atomic::AtomicU64::new(0),
            width: raw.width,
            height: raw.height,
            tone_guide_extent: [geometry.tone_size.width, geometry.tone_size.height],
            cfa_kind: raw.cfa_kind,
            processing_quality: quality,
            camera_uniforms_buffer: buffers.camera_uniforms_buffer,
            scene_tone_uniforms_buffer: buffers.scene_tone_uniforms_buffer,
            effects_uniforms_buffer: buffers.effects_uniforms_buffer,
            scene_tone_bind_group: groups.scene_tone_bind_group,
            effects_bind_group: groups.effects_bind_group,
            remove_composite_pipeline,
            remove_composite_bind_group,
            remove_composite_params_buffer,
            uploaded_stage_uniforms: Mutex::new(UploadedStageUniforms {
                camera: params.camera,
                scene_tone: params.scene_tone,
                effects: params.effects,
            }),
            mask_data_buffer: buffers.mask_data_buffer,
            tone_histogram_buffer: buffers.tone_histogram_buffer,
            tone_stats_buffer: buffers.tone_stats_buffer,
            image_light_cells_buffer: buffers.image_light_cells_buffer,
            image_light_texture: surfaces.image_light_texture,
            _image_light_core_texture: surfaces.image_light_core_texture,
            _image_light_tail_texture: surfaces.image_light_tail_texture,
            indices,
            post_blur_glow_passes,
            post_blur_pixelate_blocks_pass,
            post_blur_creative_pass,
            post_blur_render_pass,
            passes,
            raw_texture: surfaces.raw_texture,
            color_texture: surfaces.color_texture,
            black_texture: surfaces.black_texture,
            _reconstructed_raw_texture: surfaces.reconstructed_raw_texture,
            _highlight_work_a: surfaces.highlight_work_a,
            _highlight_work_b: surfaces.highlight_work_b,
            _tex1: surfaces.tex1,
            _tex2: surfaces.tex2,
            scene_texture: surfaces.scene_texture,
            scene_format: geometry.demosaic_format,
            has_ai_scene,
            has_raster_scene,
            has_ai_cfa,
            display_linear_texture: surfaces.display_linear_texture,
            _tone_guide_a: surfaces.tone_guide_a,
            _tone_guide_b: surfaces.tone_guide_b,
            mask_texture: surfaces.mask_texture,
            light_rays_mask_texture: surfaces.light_rays_mask_texture,
            scene_depth_texture: surfaces.scene_depth_texture,
            uploaded_scene_depth: Mutex::new(None),
            _relight_shadow_map: surfaces.relight_shadow_map,
            relight_shadow_map_key: Mutex::new(None),
            image_lights_current: AtomicBool::new(false),
            mask_layer_capacity: geometry.mask_layer_capacity,
            mask_atlas_edge: geometry.mask_atlas_edge,
            out_texture: surfaces.out_texture,
            out_view: surfaces.out_view,
            pipeline_cache,
            _gpu_budget_reservation: gpu_budget_reservation,
        };
        // Compile only the programs used by these edits, within the constructor's
        // error scopes. The encoder is discarded: warming needs no GPU execution.
        let mut warmup = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("calibraw active program warmup"),
        });
        pipeline.encode_raw_stage(&mut warmup, params);
        pipeline.encode_pass_range(
            &mut warmup,
            pipeline.indices.tone_prepare_pass_index,
            pipeline.indices.tone_stage_end,
        );
        pipeline.encode_tone_image_lights(&mut warmup, params);
        pipeline.encode_output_stage(&mut warmup, params);
        drop(warmup);
        // Nothing ran: the maps the warmup encoded are not built.
        pipeline
            .image_lights_current
            .store(false, Ordering::Release);
        *pipeline
            .relight_shadow_map_key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        gpu_error_scopes.finish("create RAW GPU pipeline")?;
        Ok(pipeline)
    }
}
