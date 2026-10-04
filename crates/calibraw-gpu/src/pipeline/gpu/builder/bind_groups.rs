//! Bind groups connecting the allocated surfaces and buffers to each stage.

use super::*;

pub(in crate::pipeline::gpu) struct BindGroups {
    pub(in crate::pipeline::gpu) scene_tone_bind_group: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) effects_bind_group: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_highlights: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg1: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg2: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg3: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_dual_green: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_dual_rgb: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg4: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_xtrans_derivatives: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_xtrans_homogeneity: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_xtrans_accumulate: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_xtrans_finish: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_color_denoise: [wgpu::BindGroup; 6],
    pub(in crate::pipeline::gpu) bg_tone_prepare: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_tone_horizontal: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_tone_vertical: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_tone_reduce: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_adjust_prepare: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_adjust_tone: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_adjust_local_tone: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_adjust_effects: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_adjust_effects_copy: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_mask_blur_0: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_mask_blur_1: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_mask_blur_2: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_mask_blur_3: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_mask_blur_4: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_glow_prepare: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_glow_blur_0: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_glow_blur_1: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_glow_blur_2: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_glow_blur_3: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_glow_blur_4: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_glow_prepare_after_blur: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_glow_blur_after_blur_0: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_glow_blur_after_blur_1: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_glow_blur_after_blur_2: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_glow_blur_after_blur_3: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_glow_blur_after_blur_4: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_adjust_creative: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_adjust_creative_after_blur: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_adjust_render: wgpu::BindGroup,
    pub(in crate::pipeline::gpu) bg_adjust_render_after_blur: wgpu::BindGroup,
}

pub(in crate::pipeline::gpu) fn create_bind_groups(
    device: &wgpu::Device,
    layouts: &BindGroupLayouts,
    buffers: &PipelineBuffers,
    surfaces: &PipelineSurfaces,
    cfa_kind: CfaKind,
) -> BindGroups {
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
    let PipelineBuffers {
        camera_uniforms_buffer,
        scene_tone_uniforms_buffer,
        effects_uniforms_buffer,
        mask_data_buffer,
        tone_histogram_buffer,
        tone_stats_buffer,
        profile_buffer,
        ..
    } = buffers;
    let PipelineSurfaces {
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
        ..
    } = surfaces;

    let scene_tone_bind_group = create_bind_group(
        device,
        "bg scene-tone uniforms",
        bgl_scene_tone,
        &[buffer_binding(0, scene_tone_uniforms_buffer)],
    );
    let effects_bind_group = create_bind_group(
        device,
        "bg effects uniforms",
        bgl_effects,
        &[buffer_binding(0, effects_uniforms_buffer)],
    );

    let bg_highlights = create_bind_group(
        device,
        "bg highlight reconstruction",
        bgl_highlights,
        &[
            buffer_binding(0, camera_uniforms_buffer),
            texture_binding(1, raw_view),
            texture_binding(2, color_view),
            texture_binding(19, black_view),
            texture_binding(3, reconstructed_raw_view),
        ],
    );

    let bg1 = create_bind_group(
        device,
        "bg1",
        bgl1,
        &[
            buffer_binding(0, camera_uniforms_buffer),
            texture_binding(1, raw_view),
            texture_binding(2, color_view),
            texture_binding(19, black_view),
            texture_binding(3, reconstructed_raw_view),
            texture_binding(4, tex1_view),
        ],
    );

    let bg2 = create_bind_group(
        device,
        "bg2",
        bgl2,
        &[
            buffer_binding(0, camera_uniforms_buffer),
            texture_binding(1, raw_view),
            texture_binding(2, color_view),
            texture_binding(19, black_view),
            texture_binding(3, reconstructed_raw_view),
            texture_binding(5, tex1_view),
            texture_binding(6, tex2_view),
        ],
    );

    let bg3 = create_bind_group(
        device,
        "bg3",
        bgl3,
        &[
            buffer_binding(0, camera_uniforms_buffer),
            texture_binding(1, raw_view),
            texture_binding(2, color_view),
            texture_binding(19, black_view),
            texture_binding(3, reconstructed_raw_view),
            texture_binding(7, tex2_view),
            texture_binding(8, tex1_view),
        ],
    );

    let (dual_green_view, dual_low_view) = match cfa_kind {
        CfaKind::Bayer => (highlight_work_a_view, highlight_work_b_view),
        CfaKind::XTrans => (tex1_view, tex2_view),
    };

    let bg_dual_green = create_bind_group(
        device,
        "bg dual demosaic green",
        bgl_dual_green,
        &[
            buffer_binding(0, camera_uniforms_buffer),
            texture_binding(1, raw_view),
            texture_binding(2, color_view),
            texture_binding(19, black_view),
            texture_binding(3, reconstructed_raw_view),
            texture_binding(20, dual_green_view),
        ],
    );

    let bg_dual_rgb = create_bind_group(
        device,
        "bg dual demosaic rgb",
        bgl_dual_rgb,
        &[
            buffer_binding(0, camera_uniforms_buffer),
            texture_binding(1, raw_view),
            texture_binding(2, color_view),
            texture_binding(19, black_view),
            texture_binding(3, reconstructed_raw_view),
            texture_binding(21, dual_green_view),
            texture_binding(22, dual_low_view),
        ],
    );

    let bg4 = create_bind_group(
        device,
        "bg4",
        bgl4,
        &[
            buffer_binding(0, camera_uniforms_buffer),
            texture_binding(1, raw_view),
            texture_binding(2, color_view),
            texture_binding(19, black_view),
            texture_binding(3, reconstructed_raw_view),
            texture_binding(7, tex2_view),
            texture_binding(9, tex1_view),
            texture_binding(23, dual_low_view),
            texture_binding(10, scene_view),
        ],
    );

    let bg_xtrans_derivatives = create_bind_group(
        device,
        "bg X-Trans derivatives",
        bgl_xtrans_derivatives,
        &[
            buffer_binding(0, camera_uniforms_buffer),
            texture_binding(1, raw_view),
            texture_binding(2, color_view),
            texture_binding(19, black_view),
            texture_binding(3, reconstructed_raw_view),
            texture_binding(9, tex2_view),
            texture_binding(20, highlight_work_a_view),
            texture_binding(21, highlight_work_b_view),
        ],
    );

    let bg_xtrans_homogeneity = create_bind_group(
        device,
        "bg X-Trans homogeneity",
        bgl_xtrans_homogeneity,
        &[
            buffer_binding(0, camera_uniforms_buffer),
            texture_binding(1, raw_view),
            texture_binding(2, color_view),
            texture_binding(19, black_view),
            texture_binding(3, reconstructed_raw_view),
            texture_binding(27, highlight_work_a_view),
            texture_binding(28, highlight_work_b_view),
            texture_binding(24, tex1_view),
            texture_binding(25, scene_view),
        ],
    );

    let bg_xtrans_accumulate = create_bind_group(
        device,
        "bg X-Trans accumulate",
        bgl_xtrans_accumulate,
        &[
            buffer_binding(0, camera_uniforms_buffer),
            texture_binding(1, raw_view),
            texture_binding(2, color_view),
            texture_binding(19, black_view),
            texture_binding(3, reconstructed_raw_view),
            texture_binding(9, tex2_view),
            texture_binding(29, tex1_view),
            texture_binding(30, scene_view),
            texture_binding(26, highlight_work_a_view),
        ],
    );

    let bg_xtrans_finish = create_bind_group(
        device,
        "bg X-Trans finish",
        bgl_xtrans_finish,
        &[
            buffer_binding(0, camera_uniforms_buffer),
            texture_binding(1, raw_view),
            texture_binding(2, color_view),
            texture_binding(19, black_view),
            texture_binding(3, reconstructed_raw_view),
            texture_binding(26, highlight_work_a_view),
            texture_binding(23, dual_low_view),
            texture_binding(10, scene_view),
        ],
    );

    let make_color_denoise_bind_group =
        |label: &str, read_view: &wgpu::TextureView, write_view: &wgpu::TextureView| {
            create_bind_group(
                device,
                label,
                bgl_color_denoise,
                &[
                    buffer_binding(0, camera_uniforms_buffer),
                    texture_binding(10, write_view),
                    texture_binding(11, read_view),
                ],
            )
        };
    let bg_color_denoise = [
        make_color_denoise_bind_group("bg color denoise scale 1", scene_view, tex1_view),
        make_color_denoise_bind_group("bg color denoise scale 2", tex1_view, tex2_view),
        make_color_denoise_bind_group("bg color denoise scale 4", tex2_view, tex1_view),
        make_color_denoise_bind_group("bg color denoise scale 8", tex1_view, tex2_view),
        make_color_denoise_bind_group("bg color denoise scale 16", tex2_view, tex1_view),
        make_color_denoise_bind_group("bg color denoise scale 32", tex1_view, scene_view),
    ];

    let bg_tone_prepare = create_bind_group(
        device,
        "bg tone prepare",
        bgl_tone_prepare,
        &[
            buffer_binding(0, camera_uniforms_buffer),
            texture_binding(11, scene_view),
            buffer_binding(15, tone_histogram_buffer),
            buffer_binding(20, profile_buffer),
            texture_binding(18, tone_guide_a_view),
        ],
    );

    let make_tone_blur_bind_group =
        |label: &str, read_view: &wgpu::TextureView, write_view: &wgpu::TextureView| {
            create_bind_group(
                device,
                label,
                bgl_tone_blur,
                &[
                    buffer_binding(0, camera_uniforms_buffer),
                    texture_binding(17, read_view),
                    texture_binding(18, write_view),
                ],
            )
        };
    let bg_tone_horizontal = make_tone_blur_bind_group(
        "bg tone guide horizontal",
        tone_guide_a_view,
        tone_guide_b_view,
    );
    let bg_tone_vertical = make_tone_blur_bind_group(
        "bg tone guide vertical",
        tone_guide_b_view,
        tone_guide_a_view,
    );

    let bg_tone_reduce = create_bind_group(
        device,
        "bg tone histogram reduction",
        bgl_tone_reduce,
        &[
            buffer_binding(15, tone_histogram_buffer),
            buffer_binding(16, tone_stats_buffer),
        ],
    );

    let bg_adjust_prepare = create_bind_group(
        device,
        "bg adjustment preparation",
        bgl_adjust_prepare,
        &[
            buffer_binding(0, camera_uniforms_buffer),
            texture_binding(1, raw_view),
            texture_binding(2, color_view),
            texture_binding(19, black_view),
            texture_binding(11, scene_view),
            texture_binding(21, tex1_view),
            buffer_binding(16, tone_stats_buffer),
            texture_binding(17, tone_guide_a_view),
            buffer_binding(20, profile_buffer),
            texture_binding(27, mask_view),
            sampler_binding(28, mask_sampler),
            buffer_binding(33, mask_data_buffer),
        ],
    );

    let make_adjust_tone_bind_group =
        |label: &str, input: &wgpu::TextureView, output: &wgpu::TextureView| {
            create_bind_group(
                device,
                label,
                bgl_adjust_tone,
                &[
                    buffer_binding(0, camera_uniforms_buffer),
                    texture_binding(22, input),
                    texture_binding(23, output),
                    buffer_binding(16, tone_stats_buffer),
                    texture_binding(17, tone_guide_a_view),
                    buffer_binding(20, profile_buffer),
                    texture_binding(27, mask_view),
                    sampler_binding(28, mask_sampler),
                    buffer_binding(33, mask_data_buffer),
                ],
            )
        };
    let bg_adjust_tone = make_adjust_tone_bind_group("bg scene tone edits", tex1_view, tex2_view);
    let bg_adjust_local_tone =
        make_adjust_tone_bind_group("bg local scene tone edits", tex2_view, tex1_view);

    let make_adjust_effects_bind_group =
        |label: &str, input: &wgpu::TextureView, output: &wgpu::TextureView| {
            create_bind_group(
                device,
                label,
                bgl_adjust_effects,
                &[
                    buffer_binding(0, camera_uniforms_buffer),
                    texture_binding(22, input),
                    texture_binding(23, output),
                    buffer_binding(16, tone_stats_buffer),
                    texture_binding(27, mask_view),
                    sampler_binding(28, mask_sampler),
                    buffer_binding(33, mask_data_buffer),
                ],
            )
        };
    let bg_adjust_effects =
        make_adjust_effects_bind_group("bg scene presence and color", tex1_view, tex2_view);
    let bg_adjust_effects_copy =
        make_adjust_effects_bind_group("bg scene effects copy", tex2_view, tex1_view);

    let make_mask_blur_bind_group =
        |label: &str, read_view: &wgpu::TextureView, write_view: &wgpu::TextureView| {
            create_bind_group(
                device,
                label,
                bgl_mask_blur,
                &[
                    buffer_binding(0, camera_uniforms_buffer),
                    texture_binding(24, read_view),
                    texture_binding(25, write_view),
                    texture_binding(27, mask_view),
                    sampler_binding(28, mask_sampler),
                    buffer_binding(33, mask_data_buffer),
                ],
            )
        };
    let bg_mask_blur_0 =
        make_mask_blur_bind_group("bg mask Blur diffusion 0", tex1_view, tex2_view);
    let bg_mask_blur_1 =
        make_mask_blur_bind_group("bg mask Blur diffusion 1", tex2_view, display_linear_view);
    let bg_mask_blur_2 =
        make_mask_blur_bind_group("bg mask Blur diffusion 2", display_linear_view, tex2_view);
    let bg_mask_blur_3 =
        make_mask_blur_bind_group("bg mask Blur diffusion 3", tex2_view, display_linear_view);
    let bg_mask_blur_4 =
        make_mask_blur_bind_group("bg mask Blur diffusion 4", display_linear_view, tex2_view);

    let make_glow_prepare_bind_group =
        |label: &str, source: &wgpu::TextureView, extracted: &wgpu::TextureView| {
            create_bind_group(
                device,
                label,
                bgl_glow_prepare,
                &[
                    buffer_binding(0, camera_uniforms_buffer),
                    texture_binding(24, source),
                    texture_binding(31, extracted),
                    texture_binding(27, mask_view),
                    sampler_binding(28, mask_sampler),
                    buffer_binding(33, mask_data_buffer),
                ],
            )
        };
    let bg_glow_prepare =
        make_glow_prepare_bind_group("bg Glow source extraction", tex1_view, tex2_view);

    let make_glow_blur_bind_group =
        |label: &str, read_view: &wgpu::TextureView, write_view: &wgpu::TextureView| {
            create_bind_group(
                device,
                label,
                bgl_glow_blur,
                &[
                    buffer_binding(0, camera_uniforms_buffer),
                    texture_binding(30, read_view),
                    texture_binding(31, write_view),
                ],
            )
        };
    let bg_glow_blur_0 =
        make_glow_blur_bind_group("bg Glow diffusion 0", tex2_view, display_linear_view);
    let bg_glow_blur_1 =
        make_glow_blur_bind_group("bg Glow diffusion 1", display_linear_view, tex2_view);
    let bg_glow_blur_2 =
        make_glow_blur_bind_group("bg Glow diffusion 2", tex2_view, display_linear_view);
    let bg_glow_blur_3 =
        make_glow_blur_bind_group("bg Glow diffusion 3", display_linear_view, tex2_view);
    let bg_glow_blur_4 =
        make_glow_blur_bind_group("bg Glow diffusion 4", tex2_view, display_linear_view);

    let bg_glow_prepare_after_blur = make_glow_prepare_bind_group(
        "bg Glow source extraction after mask Blur",
        tex2_view,
        tex1_view,
    );
    let bg_glow_blur_after_blur_0 = make_glow_blur_bind_group(
        "bg Glow diffusion after mask Blur 0",
        tex1_view,
        display_linear_view,
    );
    let bg_glow_blur_after_blur_1 = make_glow_blur_bind_group(
        "bg Glow diffusion after mask Blur 1",
        display_linear_view,
        tex1_view,
    );
    let bg_glow_blur_after_blur_2 = make_glow_blur_bind_group(
        "bg Glow diffusion after mask Blur 2",
        tex1_view,
        display_linear_view,
    );
    let bg_glow_blur_after_blur_3 = make_glow_blur_bind_group(
        "bg Glow diffusion after mask Blur 3",
        display_linear_view,
        tex1_view,
    );
    let bg_glow_blur_after_blur_4 = make_glow_blur_bind_group(
        "bg Glow diffusion after mask Blur 4",
        tex1_view,
        display_linear_view,
    );

    let make_adjust_creative_bind_group =
        |label: &str, input: &wgpu::TextureView, output: &wgpu::TextureView| {
            create_bind_group(
                device,
                label,
                bgl_adjust_creative,
                &[
                    buffer_binding(0, camera_uniforms_buffer),
                    texture_binding(24, input),
                    texture_binding(25, output),
                    texture_binding(30, display_linear_view),
                    texture_binding(27, mask_view),
                    sampler_binding(28, mask_sampler),
                    buffer_binding(33, mask_data_buffer),
                    texture_binding(34, light_rays_mask_view),
                    texture_binding(35, scene_depth_view),
                    buffer_binding(16, tone_stats_buffer),
                ],
            )
        };
    let bg_adjust_creative =
        make_adjust_creative_bind_group("bg creative glow", tex1_view, tex2_view);
    let bg_adjust_creative_after_blur = make_adjust_creative_bind_group(
        "bg creative effects after mask Blur",
        tex2_view,
        tex1_view,
    );

    let make_adjust_render_bind_group = |label: &str, source: &wgpu::TextureView| {
        create_bind_group(
            device,
            label,
            bgl_adjust_render,
            &[
                buffer_binding(0, camera_uniforms_buffer),
                texture_binding(12, out_view),
                texture_binding(26, source),
                buffer_binding(16, tone_stats_buffer),
                buffer_binding(20, profile_buffer),
                texture_binding(27, mask_view),
                sampler_binding(28, mask_sampler),
                texture_binding(29, display_linear_view),
                buffer_binding(33, mask_data_buffer),
            ],
        )
    };
    let bg_adjust_render =
        make_adjust_render_bind_group("bg scene look view and output", tex2_view);
    let bg_adjust_render_after_blur =
        make_adjust_render_bind_group("bg scene look view and output after mask Blur", tex1_view);

    BindGroups {
        scene_tone_bind_group,
        effects_bind_group,
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
    }
}
