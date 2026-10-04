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
    pub(in crate::pipeline::gpu) adjustment_creative_pass_index: usize,
    pub(in crate::pipeline::gpu) adjustment_render_pass_index: usize,
}

pub(in crate::pipeline::gpu) struct AssembledPasses {
    pub(in crate::pipeline::gpu) passes: Vec<Pass>,
    pub(in crate::pipeline::gpu) post_blur_glow_passes: Vec<Pass>,
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
    let BindGroupLayouts {
        bgl_scene_tone,
        bgl_effects,
        bgl_highlights,
        bgl1,
        bgl2,
        bgl3,
        bgl_dual_green,
        bgl_dual_rgb,
        bgl4,
        bgl_xtrans_derivatives,
        bgl_xtrans_homogeneity,
        bgl_xtrans_accumulate,
        bgl_xtrans_finish,
        bgl_color_denoise,
        bgl_tone_prepare,
        bgl_tone_blur,
        bgl_tone_reduce,
        bgl_adjust_prepare,
        bgl_adjust_tone,
        bgl_adjust_effects,
        bgl_mask_blur,
        bgl_glow_prepare,
        bgl_glow_blur,
        bgl_adjust_creative,
        bgl_adjust_render,
    } = layouts;
    let BindGroups {
        bg_highlights,
        bg1,
        bg2,
        bg3,
        bg_dual_green,
        bg_dual_rgb,
        bg4,
        bg_xtrans_derivatives,
        bg_xtrans_homogeneity,
        bg_xtrans_accumulate,
        bg_xtrans_finish,
        bg_color_denoise,
        bg_tone_prepare,
        bg_tone_horizontal,
        bg_tone_vertical,
        bg_tone_reduce,
        bg_adjust_prepare,
        bg_adjust_tone,
        bg_adjust_local_tone,
        bg_adjust_effects,
        bg_adjust_effects_copy,
        bg_mask_blur_0,
        bg_mask_blur_1,
        bg_mask_blur_2,
        bg_mask_blur_3,
        bg_mask_blur_4,
        bg_glow_prepare,
        bg_glow_blur_0,
        bg_glow_blur_1,
        bg_glow_blur_2,
        bg_glow_blur_3,
        bg_glow_blur_4,
        bg_glow_prepare_after_blur,
        bg_glow_blur_after_blur_0,
        bg_glow_blur_after_blur_1,
        bg_glow_blur_after_blur_2,
        bg_glow_blur_after_blur_3,
        bg_glow_blur_after_blur_4,
        bg_adjust_creative,
        bg_adjust_creative_after_blur,
        bg_adjust_render,
        bg_adjust_render_after_blur,
        ..
    } = groups;
    let ShaderSet {
        highlight_module,
        bayer_rcd_p1_module,
        bayer_rcd_p2_module,
        bayer_rcd_p3_module,
        bayer_rcd_p4_module,
        dual_demosaic_module,
        xtrans_demosaic_module,
        xtrans_finish_module,
        color_denoise_module,
        tone_analysis_module,
        scene_adjustments_module,
        creative_effects_module,
        view_transform_module,
    } = shaders;

    let mut assembler = PassAssembler {
        device,
        program_template,
        pipeline_cache,
        bgl_scene_tone,
        bgl_effects,
        next_program_index: 0,
    };
    let single_workgroup = [1, 1, 1];

    let mut passes = Vec::with_capacity(expected_pass_count(cfa_kind));

    passes.push(assembler.make_pass(
        highlight_module.as_ref(),
        "highlight_reconstruct",
        bgl_highlights,
        bg_highlights.clone(),
        image_workgroups,
    ));

    let demosaic_start_index = passes.len();
    match cfa_kind {
        CfaKind::Bayer => passes.extend([
            assembler.make_pass(
                bayer_rcd_p1_module.as_ref(),
                "bayer_rcd_directional",
                bgl1,
                bg1.clone(),
                image_workgroups,
            ),
            assembler.make_pass(
                bayer_rcd_p2_module.as_ref(),
                "bayer_rcd_green",
                bgl2,
                bg2.clone(),
                image_workgroups,
            ),
            assembler.make_pass(
                bayer_rcd_p3_module.as_ref(),
                "bayer_rcd_chroma",
                bgl3,
                bg3.clone(),
                image_workgroups,
            ),
        ]),
        CfaKind::XTrans => passes.extend([
            assembler.make_pass(
                xtrans_demosaic_module.as_ref(),
                "xtrans_seed",
                bgl1,
                bg1.clone(),
                image_workgroups,
            ),
            assembler.make_pass(
                xtrans_demosaic_module.as_ref(),
                "xtrans_markesteijn_pass1",
                bgl2,
                bg2.clone(),
                image_workgroups,
            ),
            assembler.make_pass(
                xtrans_demosaic_module.as_ref(),
                "xtrans_markesteijn_pass2",
                bgl3,
                bg3.clone(),
                image_workgroups,
            ),
            assembler.make_pass(
                xtrans_demosaic_module.as_ref(),
                "xtrans_markesteijn_pass3",
                bgl2,
                bg2.clone(),
                image_workgroups,
            ),
            assembler.make_pass(
                xtrans_demosaic_module.as_ref(),
                "xtrans_markesteijn_derivatives",
                bgl_xtrans_derivatives,
                bg_xtrans_derivatives.clone(),
                image_workgroups,
            ),
            assembler.make_pass(
                xtrans_demosaic_module.as_ref(),
                "xtrans_markesteijn_homogeneity",
                bgl_xtrans_homogeneity,
                bg_xtrans_homogeneity.clone(),
                image_workgroups,
            ),
            assembler.make_pass(
                xtrans_demosaic_module.as_ref(),
                "xtrans_markesteijn_accumulate",
                bgl_xtrans_accumulate,
                bg_xtrans_accumulate.clone(),
                image_workgroups,
            ),
        ]),
    }

    let demosaic_dual_start_index = passes.len();
    passes.extend([
        assembler.make_pass(
            dual_demosaic_module.as_ref(),
            "dual_green_reconstruct",
            bgl_dual_green,
            bg_dual_green.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            dual_demosaic_module.as_ref(),
            "dual_rgb_reconstruct",
            bgl_dual_rgb,
            bg_dual_rgb.clone(),
            image_workgroups,
        ),
    ]);
    let demosaic_dual_end_index = passes.len();

    let demosaic_finish_index = passes.len();
    match cfa_kind {
        CfaKind::Bayer => passes.push(assembler.make_pass(
            bayer_rcd_p4_module.as_ref(),
            "bayer_rcd_output",
            bgl4,
            bg4.clone(),
            image_workgroups,
        )),
        CfaKind::XTrans => passes.push(assembler.make_pass(
            xtrans_finish_module.as_ref(),
            "xtrans_demosaic_finish",
            bgl_xtrans_finish,
            bg_xtrans_finish.clone(),
            image_workgroups,
        )),
    }

    let color_denoise_start_index = passes.len();
    for (entry, bind_group) in COLOR_DENOISE_ENTRY_POINTS
        .iter()
        .zip(bg_color_denoise.iter())
    {
        passes.push(assembler.make_pass(
            color_denoise_module.as_ref(),
            entry,
            bgl_color_denoise,
            bind_group.clone(),
            image_workgroups,
        ));
    }
    let color_denoise_end_index = passes.len();

    let tone_prepare_pass_index = passes.len();
    passes.extend([
        assembler.make_pass(
            tone_analysis_module.as_ref(),
            "tone_guide_prepare",
            bgl_tone_prepare,
            bg_tone_prepare.clone(),
            tone_workgroups,
        ),
        assembler.make_pass(
            tone_analysis_module.as_ref(),
            "tone_guide_horizontal",
            bgl_tone_blur,
            bg_tone_horizontal.clone(),
            tone_workgroups,
        ),
        assembler.make_pass(
            tone_analysis_module.as_ref(),
            "tone_guide_vertical",
            bgl_tone_blur,
            bg_tone_vertical.clone(),
            tone_workgroups,
        ),
        assembler.make_pass(
            tone_analysis_module.as_ref(),
            "tone_reduce_histogram",
            bgl_tone_reduce,
            bg_tone_reduce.clone(),
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
    let adjustment_creative_pass_index = glow_blur_end_index;
    let adjustment_render_pass_index = adjustment_creative_pass_index + 1;

    passes.extend([
        assembler.make_pass(
            scene_adjustments_module.as_ref(),
            "prepare_scene_node",
            bgl_adjust_prepare,
            bg_adjust_prepare.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            scene_adjustments_module.as_ref(),
            "apply_scene_tone_node",
            bgl_adjust_tone,
            bg_adjust_tone.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            scene_adjustments_module.as_ref(),
            "apply_local_scene_tone_node",
            bgl_adjust_tone,
            bg_adjust_local_tone.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            creative_effects_module.as_ref(),
            "apply_scene_effects_node",
            bgl_adjust_effects,
            bg_adjust_effects.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            creative_effects_module.as_ref(),
            "copy_scene_effects_node",
            bgl_adjust_effects,
            bg_adjust_effects_copy.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            creative_effects_module.as_ref(),
            "diffuse_mask_blur_0",
            bgl_mask_blur,
            bg_mask_blur_0.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            creative_effects_module.as_ref(),
            "diffuse_mask_blur_1",
            bgl_mask_blur,
            bg_mask_blur_1.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            creative_effects_module.as_ref(),
            "diffuse_mask_blur_2",
            bgl_mask_blur,
            bg_mask_blur_2.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            creative_effects_module.as_ref(),
            "diffuse_mask_blur_3",
            bgl_mask_blur,
            bg_mask_blur_3.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            creative_effects_module.as_ref(),
            "diffuse_mask_blur_4",
            bgl_mask_blur,
            bg_mask_blur_4.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            creative_effects_module.as_ref(),
            "prepare_glow_source",
            bgl_glow_prepare,
            bg_glow_prepare.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            creative_effects_module.as_ref(),
            "diffuse_glow_0",
            bgl_glow_blur,
            bg_glow_blur_0.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            creative_effects_module.as_ref(),
            "diffuse_glow_1",
            bgl_glow_blur,
            bg_glow_blur_1.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            creative_effects_module.as_ref(),
            "diffuse_glow_2",
            bgl_glow_blur,
            bg_glow_blur_2.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            creative_effects_module.as_ref(),
            "diffuse_glow_3",
            bgl_glow_blur,
            bg_glow_blur_3.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            creative_effects_module.as_ref(),
            "diffuse_glow_4",
            bgl_glow_blur,
            bg_glow_blur_4.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            creative_effects_module.as_ref(),
            "apply_creative_effects",
            bgl_adjust_creative,
            bg_adjust_creative.clone(),
            image_workgroups,
        ),
        assembler.make_pass(
            view_transform_module.as_ref(),
            "apply_view_node",
            bgl_adjust_render,
            bg_adjust_render.clone(),
            image_workgroups,
        ),
    ]);

    let post_blur_glow_passes = vec![
        Pass {
            pipeline: passes[glow_prepare_pass_index].pipeline.clone(),
            bind_group: bg_glow_prepare_after_blur.clone(),
            workgroups: image_workgroups,
        },
        Pass {
            pipeline: passes[glow_blur_start_index].pipeline.clone(),
            bind_group: bg_glow_blur_after_blur_0.clone(),
            workgroups: image_workgroups,
        },
        Pass {
            pipeline: passes[glow_blur_start_index + 1].pipeline.clone(),
            bind_group: bg_glow_blur_after_blur_1.clone(),
            workgroups: image_workgroups,
        },
        Pass {
            pipeline: passes[glow_blur_start_index + 2].pipeline.clone(),
            bind_group: bg_glow_blur_after_blur_2.clone(),
            workgroups: image_workgroups,
        },
        Pass {
            pipeline: passes[glow_blur_start_index + 3].pipeline.clone(),
            bind_group: bg_glow_blur_after_blur_3.clone(),
            workgroups: image_workgroups,
        },
        Pass {
            pipeline: passes[glow_blur_start_index + 4].pipeline.clone(),
            bind_group: bg_glow_blur_after_blur_4.clone(),
            workgroups: image_workgroups,
        },
    ];
    let post_blur_creative_pass = Pass {
        pipeline: passes[adjustment_creative_pass_index].pipeline.clone(),
        bind_group: bg_adjust_creative_after_blur.clone(),
        workgroups: image_workgroups,
    };
    let post_blur_render_pass = Pass {
        pipeline: passes[adjustment_render_pass_index].pipeline.clone(),
        bind_group: bg_adjust_render_after_blur.clone(),
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
            adjustment_creative_pass_index,
            adjustment_render_pass_index,
        },
    })
}
