//! Ordering compute passes into stages and recording their indices.

use super::*;

#[derive(Clone, Copy, Debug)]
pub(in crate::pipeline::gpu) struct StageIndices {
    pub(in crate::pipeline::gpu) tone_prepare_pass_index: usize,
    pub(in crate::pipeline::gpu) tone_reduce_pass_index: usize,
    pub(in crate::pipeline::gpu) tone_stage_end: usize,
    pub(in crate::pipeline::gpu) demosaic_start_index: usize,
    pub(in crate::pipeline::gpu) demosaic_dual_start_index: usize,
    pub(in crate::pipeline::gpu) demosaic_dual_end_index: usize,
    pub(in crate::pipeline::gpu) demosaic_finish_index: usize,
    pub(in crate::pipeline::gpu) color_denoise_start_index: usize,
    pub(in crate::pipeline::gpu) color_denoise_end_index: usize,
    pub(in crate::pipeline::gpu) adjustment_prepare_pass_index: usize,
    pub(in crate::pipeline::gpu) adjustment_tone_pass_index: usize,
    pub(in crate::pipeline::gpu) adjustment_effects_pass_index: usize,
    pub(in crate::pipeline::gpu) mask_blur_start_index: usize,
    pub(in crate::pipeline::gpu) mask_blur_end_index: usize,
    pub(in crate::pipeline::gpu) glow_prepare_pass_index: usize,
    pub(in crate::pipeline::gpu) glow_blur_start_index: usize,
    pub(in crate::pipeline::gpu) glow_blur_end_index: usize,
    pub(in crate::pipeline::gpu) pixelate_blocks_pass_index: usize,
    pub(in crate::pipeline::gpu) adjustment_creative_pass_index: usize,
    pub(in crate::pipeline::gpu) adjustment_render_pass_index: usize,
    /// Accumulation into the image-light grid; resolve and the two blurs follow.
    pub(in crate::pipeline::gpu) image_light_accumulate_pass_index: usize,
    pub(in crate::pipeline::gpu) image_light_resolve_pass_index: usize,
    pub(in crate::pipeline::gpu) image_light_end_index: usize,
}

pub(in crate::pipeline::gpu) struct AssembledPasses {
    pub(in crate::pipeline::gpu) passes: Vec<Pass>,
    pub(in crate::pipeline::gpu) post_blur_glow_passes: Vec<Pass>,
    pub(in crate::pipeline::gpu) post_blur_pixelate_blocks_pass: Pass,
    pub(in crate::pipeline::gpu) post_blur_creative_pass: Pass,
    pub(in crate::pipeline::gpu) post_blur_render_pass: Pass,
    pub(in crate::pipeline::gpu) indices: StageIndices,
}

struct PassAssembler<'a> {
    pub(super) device: &'a wgpu::Device,
    program_template: Option<&'a RawGpuProgramTemplate>,
    pipeline_cache: Option<&'a Arc<PersistentGpuPipelineCache>>,
    bgl_scene_tone: &'a wgpu::BindGroupLayout,
    bgl_effects: &'a wgpu::BindGroupLayout,
    next_program_index: usize,
}

impl PassAssembler<'_> {
    fn make_pass(
        &mut self,
        shader: Option<&wgpu::ShaderModule>,
        entry: &str,
        bgl: &wgpu::BindGroupLayout,
        bind_group: wgpu::BindGroup,
        workgroups: [u32; 3],
    ) -> Pass {
        let program_index = self.next_program_index;
        self.next_program_index += 1;
        let pipeline = if let Some(template) = self.program_template {
            template.pipelines[program_index].clone()
        } else {
            let shader = shader.expect("shader module exists without a program template");
            Arc::new(ComputeProgram {
                device: self.device.clone(),
                shader: shader.clone(),
                layouts: [
                    bgl.clone(),
                    self.bgl_scene_tone.clone(),
                    self.bgl_effects.clone(),
                ],
                entry: entry.to_owned(),
                cache: self.pipeline_cache.cloned(),
                compiled: OnceLock::new(),
                demosaic_variants: std::array::from_fn(|_| OnceLock::new()),
            })
        };
        Pass {
            pipeline,
            bind_group,
            workgroups,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::pipeline::gpu) fn assemble_passes(
    device: &wgpu::Device,
    program_template: Option<&RawGpuProgramTemplate>,
    pipeline_cache: Option<&Arc<PersistentGpuPipelineCache>>,
    layouts: &BindGroupLayouts,
    groups: &BindGroups,
    shaders: &ShaderSet,
    cfa_kind: CfaKind,
    image_workgroups: [u32; 3],
    tone_workgroups: [u32; 3],
) -> Result<AssembledPasses> {
    let mut assembler = PassAssembler {
        device,
        program_template,
        pipeline_cache,
        bgl_scene_tone: &layouts.bgl_scene_tone,
        bgl_effects: &layouts.bgl_effects,
        next_program_index: 0,
    };
    let single_workgroup = [1, 1, 1];

    let mut passes = Vec::with_capacity(expected_pass_count(cfa_kind));

    passes.push(assembler.make_pass(
        shaders.highlight_module.as_ref(),
        "highlight_reconstruct",
        &layouts.bgl_highlights,
        groups.bg_highlights.clone(),
        image_workgroups,
    ));

    let demosaic_start_index = passes.len();
    match cfa_kind {
        CfaKind::Bayer => passes.extend([
            assembler.make_pass(
                shaders.bayer_rcd_p1_module.as_ref(),
                "bayer_rcd_directional",
                &layouts.bgl1,
                groups.bg1.clone(),
                image_workgroups,
            ),
            assembler.make_pass(
                shaders.bayer_rcd_p2_module.as_ref(),
                "bayer_rcd_green",
                &layouts.bgl2,
                groups.bg2.clone(),
                image_workgroups,
            ),
            assembler.make_pass(
                shaders.bayer_rcd_p3_module.as_ref(),
                "bayer_rcd_chroma",
                &layouts.bgl3,
                groups.bg3.clone(),
                image_workgroups,
            ),
        ]),
        CfaKind::XTrans => passes.extend([
            assembler.make_pass(
                shaders.xtrans_demosaic_module.as_ref(),
                "xtrans_seed",
                &layouts.bgl1,
                groups.bg1.clone(),
                image_workgroups,
            ),
            assembler.make_pass(
                shaders.xtrans_demosaic_module.as_ref(),
                "xtrans_markesteijn_pass1",
                &layouts.bgl2,
                groups.bg2.clone(),
                image_workgroups,
            ),
            assembler.make_pass(
                shaders.xtrans_demosaic_module.as_ref(),
                "xtrans_markesteijn_pass2",
                &layouts.bgl3,
                groups.bg3.clone(),
                image_workgroups,
            ),
            assembler.make_pass(
                shaders.xtrans_demosaic_module.as_ref(),
                "xtrans_markesteijn_pass3",
                &layouts.bgl2,
                groups.bg2.clone(),
                image_workgroups,
            ),
            assembler.make_pass(
                shaders.xtrans_demosaic_module.as_ref(),
                "xtrans_markesteijn_derivatives",
                &layouts.bgl_xtrans_derivatives,
                groups.bg_xtrans_derivatives.clone(),
                image_workgroups,
            ),
            assembler.make_pass(
                shaders.xtrans_demosaic_module.as_ref(),
                "xtrans_markesteijn_homogeneity",
                &layouts.bgl_xtrans_homogeneity,
                groups.bg_xtrans_homogeneity.clone(),
                image_workgroups,
            ),
            assembler.make_pass(
                shaders.xtrans_demosaic_module.as_ref(),
                "xtrans_markesteijn_accumulate",
                &layouts.bgl_xtrans_accumulate,
                groups.bg_xtrans_accumulate.clone(),
                image_workgroups,
            ),
        ]),
    }

    let demosaic_dual_start_index = passes.len();
    passes.extend([
        assembler.make_pass(
            shaders.dual_demosaic_module.as_ref(),
            "dual_green_reconstruct",
            &layouts.bgl_dual_green,
            groups.bg_dual_green.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            shaders.dual_demosaic_module.as_ref(),
            "dual_rgb_reconstruct",
            &layouts.bgl_dual_rgb,
            groups.bg_dual_rgb.clone(),
            image_workgroups,
        ),
    ]);
    let demosaic_dual_end_index = passes.len();

    let demosaic_finish_index = passes.len();
    match cfa_kind {
        CfaKind::Bayer => passes.push(assembler.make_pass(
            shaders.bayer_rcd_p4_module.as_ref(),
            "bayer_rcd_output",
            &layouts.bgl4,
            groups.bg4.clone(),
            image_workgroups,
        )),
        CfaKind::XTrans => passes.push(assembler.make_pass(
            shaders.xtrans_finish_module.as_ref(),
            "xtrans_demosaic_finish",
            &layouts.bgl_xtrans_finish,
            groups.bg_xtrans_finish.clone(),
            image_workgroups,
        )),
    }

    let color_denoise_start_index = passes.len();
    for (entry, bind_group) in COLOR_DENOISE_ENTRY_POINTS
        .iter()
        .zip(groups.bg_color_denoise.iter())
    {
        passes.push(assembler.make_pass(
            shaders.color_denoise_module.as_ref(),
            entry,
            &layouts.bgl_color_denoise,
            bind_group.clone(),
            image_workgroups,
        ));
    }
    let color_denoise_end_index = passes.len();

    let tone_prepare_pass_index = passes.len();
    passes.extend([
        assembler.make_pass(
            shaders.tone_analysis_module.as_ref(),
            "tone_guide_prepare",
            &layouts.bgl_tone_prepare,
            groups.bg_tone_prepare.clone(),
            tone_workgroups,
        ),
        assembler.make_pass(
            shaders.tone_analysis_module.as_ref(),
            "tone_guide_horizontal",
            &layouts.bgl_tone_blur,
            groups.bg_tone_horizontal.clone(),
            tone_workgroups,
        ),
        assembler.make_pass(
            shaders.tone_analysis_module.as_ref(),
            "tone_guide_vertical",
            &layouts.bgl_tone_blur,
            groups.bg_tone_vertical.clone(),
            tone_workgroups,
        ),
        assembler.make_pass(
            shaders.tone_analysis_module.as_ref(),
            "tone_reduce_histogram",
            &layouts.bgl_tone_reduce,
            groups.bg_tone_reduce.clone(),
            single_workgroup,
        ),
    ]);

    let tone_reduce_pass_index = tone_prepare_pass_index + 3;
    let tone_stage_end = passes.len();
    let adjustment_prepare_pass_index = passes.len();
    let adjustment_tone_pass_index = adjustment_prepare_pass_index + 1;
    let adjustment_effects_pass_index = adjustment_prepare_pass_index + 3;
    let mask_blur_start_index = adjustment_prepare_pass_index + 5;
    let mask_blur_end_index = mask_blur_start_index + 5;
    let glow_prepare_pass_index = mask_blur_end_index;
    let glow_blur_start_index = glow_prepare_pass_index + 1;
    let glow_blur_end_index = glow_blur_start_index + 5;
    let pixelate_blocks_pass_index = glow_blur_end_index;
    let adjustment_creative_pass_index = pixelate_blocks_pass_index + 1;
    let adjustment_render_pass_index = adjustment_creative_pass_index + 1;

    passes.extend([
        assembler.make_pass(
            shaders.scene_adjustments_module.as_ref(),
            "prepare_scene_node",
            &layouts.bgl_adjust_prepare,
            groups.bg_adjust_prepare.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            shaders.scene_adjustments_module.as_ref(),
            "apply_scene_tone_node",
            &layouts.bgl_adjust_tone,
            groups.bg_adjust_tone.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            shaders.scene_adjustments_module.as_ref(),
            "apply_local_scene_tone_node",
            &layouts.bgl_adjust_tone,
            groups.bg_adjust_local_tone.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            shaders.creative_effects_module.as_ref(),
            "apply_scene_effects_node",
            &layouts.bgl_adjust_effects,
            groups.bg_adjust_effects.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            shaders.creative_effects_module.as_ref(),
            "copy_scene_effects_node",
            &layouts.bgl_adjust_effects,
            groups.bg_adjust_effects_copy.clone(),
            image_workgroups,
        ),
    ]);
    for (step, bind_group) in groups.bg_mask_blur.iter().enumerate() {
        passes.push(assembler.make_pass(
            shaders.creative_effects_module.as_ref(),
            &format!("diffuse_mask_blur_{step}"),
            &layouts.bgl_mask_blur,
            bind_group.clone(),
            image_workgroups,
        ));
    }
    passes.push(assembler.make_pass(
        shaders.creative_effects_module.as_ref(),
        "prepare_glow_source",
        &layouts.bgl_glow_prepare,
        groups.bg_glow_prepare.clone(),
        image_workgroups,
    ));
    for (step, bind_group) in groups.bg_glow_blur.iter().enumerate() {
        passes.push(assembler.make_pass(
            shaders.creative_effects_module.as_ref(),
            &format!("diffuse_glow_{step}"),
            &layouts.bgl_glow_blur,
            bind_group.clone(),
            image_workgroups,
        ));
    }
    passes.extend([
        assembler.make_pass(
            shaders.creative_effects_module.as_ref(),
            "prepare_pixelate_blocks",
            &layouts.bgl_pixelate_blocks,
            groups.bg_pixelate_blocks.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            shaders.creative_effects_module.as_ref(),
            "apply_creative_effects",
            &layouts.bgl_adjust_creative,
            groups.bg_adjust_creative.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            shaders.view_transform_module.as_ref(),
            "apply_view_node",
            &layouts.bgl_adjust_render,
            groups.bg_adjust_render.clone(),
            image_workgroups,
        ),
    ]);

    // Image lights run with the tone stage (or the export tone prepass), but
    // follow every other pass so the earlier pass indices stay put.
    let image_light_workgroups = [
        IMAGE_LIGHT_GRID_LONG.div_ceil(8),
        IMAGE_LIGHT_GRID_LONG.div_ceil(8),
        1,
    ];
    let image_light_accumulate_pass_index = passes.len();
    let image_light_resolve_pass_index = image_light_accumulate_pass_index + 1;
    passes.extend([
        assembler.make_pass(
            shaders.tone_analysis_module.as_ref(),
            "accumulate_image_lights",
            &layouts.bgl_image_light_accumulate,
            groups.bg_image_light_accumulate.clone(),
            image_light_workgroups,
        ),
        assembler.make_pass(
            shaders.tone_analysis_module.as_ref(),
            "resolve_image_lights",
            &layouts.bgl_image_light_resolve,
            groups.bg_image_light_resolve.clone(),
            image_light_workgroups,
        ),
        assembler.make_pass(
            shaders.tone_analysis_module.as_ref(),
            "blur_image_lights_horizontal",
            &layouts.bgl_image_light_blur_horizontal,
            groups.bg_image_light_blur_horizontal.clone(),
            image_light_workgroups,
        ),
        assembler.make_pass(
            shaders.tone_analysis_module.as_ref(),
            "blur_image_lights_vertical",
            &layouts.bgl_image_light_blur_vertical,
            groups.bg_image_light_blur_vertical.clone(),
            image_light_workgroups,
        ),
    ]);
    let image_light_end_index = passes.len();

    // The post-blur variants reuse the programs above with bind groups that
    // read the mask-blurred scene.
    let mut post_blur_glow_passes = vec![Pass {
        pipeline: passes[glow_prepare_pass_index].pipeline.clone(),
        bind_group: groups.bg_glow_prepare_after_blur.clone(),
        workgroups: image_workgroups,
    }];
    post_blur_glow_passes.extend(groups.bg_glow_blur_after_blur.iter().enumerate().map(
        |(step, bind_group)| Pass {
            pipeline: passes[glow_blur_start_index + step].pipeline.clone(),
            bind_group: bind_group.clone(),
            workgroups: image_workgroups,
        },
    ));
    let post_blur_pixelate_blocks_pass = Pass {
        pipeline: passes[pixelate_blocks_pass_index].pipeline.clone(),
        bind_group: groups.bg_pixelate_blocks_after_blur.clone(),
        workgroups: image_workgroups,
    };
    let post_blur_creative_pass = Pass {
        pipeline: passes[adjustment_creative_pass_index].pipeline.clone(),
        bind_group: groups.bg_adjust_creative_after_blur.clone(),
        workgroups: image_workgroups,
    };
    let post_blur_render_pass = Pass {
        pipeline: passes[adjustment_render_pass_index].pipeline.clone(),
        bind_group: groups.bg_adjust_render_after_blur.clone(),
        workgroups: image_workgroups,
    };

    let expected_programs = expected_pass_count(cfa_kind);
    if assembler.next_program_index != expected_programs || passes.len() != expected_programs {
        return Err(anyhow!(
            "GPU render-plan mismatch for {:?}: built {} passes and consumed {} programs; expected {}",
            cfa_kind,
            passes.len(),
            assembler.next_program_index,
            expected_programs,
        ));
    }

    Ok(AssembledPasses {
        passes,
        post_blur_glow_passes,
        post_blur_pixelate_blocks_pass,
        post_blur_creative_pass,
        post_blur_render_pass,
        indices: StageIndices {
            tone_prepare_pass_index,
            tone_reduce_pass_index,
            tone_stage_end,
            demosaic_start_index,
            demosaic_dual_start_index,
            demosaic_dual_end_index,
            demosaic_finish_index,
            color_denoise_start_index,
            color_denoise_end_index,
            adjustment_prepare_pass_index,
            adjustment_tone_pass_index,
            adjustment_effects_pass_index,
            mask_blur_start_index,
            mask_blur_end_index,
            glow_prepare_pass_index,
            glow_blur_start_index,
            glow_blur_end_index,
            pixelate_blocks_pass_index,
            adjustment_creative_pass_index,
            adjustment_render_pass_index,
            image_light_accumulate_pass_index,
            image_light_resolve_pass_index,
            image_light_end_index,
        },
    })
}
