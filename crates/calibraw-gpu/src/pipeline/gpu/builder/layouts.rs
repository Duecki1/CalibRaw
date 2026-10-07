//! Bind group layouts of every pipeline stage.

use super::*;

pub(in crate::pipeline::gpu) struct BindGroupLayouts {
    pub(in crate::pipeline::gpu) bgl_scene_tone: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_effects: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_highlights: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl1: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl2: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl3: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_dual_green: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_dual_rgb: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl4: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_xtrans_derivatives: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_xtrans_homogeneity: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_xtrans_accumulate: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_xtrans_finish: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_color_denoise: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_tone_prepare: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_tone_blur: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_tone_reduce: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_adjust_prepare: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_adjust_tone: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_adjust_effects: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_mask_blur: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_glow_prepare: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_glow_blur: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_pixelate_blocks: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_adjust_creative: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_adjust_render: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_image_light_accumulate: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_image_light_resolve: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_image_light_blur_horizontal: wgpu::BindGroupLayout,
    pub(in crate::pipeline::gpu) bgl_image_light_blur_vertical: wgpu::BindGroupLayout,
}

pub(in crate::pipeline::gpu) fn create_bind_group_layouts(
    device: &wgpu::Device,
    program_template: Option<&RawGpuProgramTemplate>,
    cfa_kind: CfaKind,
    demosaic_format: wgpu::TextureFormat,
    work_format: wgpu::TextureFormat,
    tone_format: wgpu::TextureFormat,
) -> BindGroupLayouts {
    let common_entries = [
        buffer_entry(0),
        texture_entry(1, wgpu::TextureSampleType::Uint),
        texture_entry(2, wgpu::TextureSampleType::Uint),
        texture_entry(19, wgpu::TextureSampleType::Float { filterable: false }),
    ];

    let bgl_scene_tone = program_template
        .map(|template| template.pipelines[0].get_bind_group_layout(1))
        .unwrap_or_else(|| {
            create_bind_group_layout(device, "bgl scene-tone uniforms", &[buffer_entry(0)])
        });
    let bgl_effects = program_template
        .map(|template| template.pipelines[0].get_bind_group_layout(2))
        .unwrap_or_else(|| {
            create_bind_group_layout(device, "bgl effects uniforms", &[buffer_entry(0)])
        });

    let demosaic_start_for_programs = 1;
    let demosaic_high_pass_count = match cfa_kind {
        CfaKind::Bayer => 3,
        CfaKind::XTrans => 7,
    };
    let dual_green_for_programs = demosaic_start_for_programs + demosaic_high_pass_count;
    let dual_rgb_for_programs = dual_green_for_programs + 1;
    let demosaic_finish_for_programs = dual_rgb_for_programs + 1;
    let color_denoise_for_programs = demosaic_finish_for_programs + 1;
    let tone_prepare_for_programs = color_denoise_for_programs + COLOR_DENOISE_ENTRY_POINTS.len();
    let adjustment_prepare_for_programs = tone_prepare_for_programs + 4;
    let reused_layout = |pass_index: usize| {
        program_template.map(|template| template.pipelines[pass_index].get_bind_group_layout(0))
    };

    let bgl_highlights = reused_layout(0).unwrap_or_else(|| {
        create_bind_group_layout(
            device,
            "bgl highlights",
            &[
                common_entries[0],
                common_entries[1],
                common_entries[2],
                common_entries[3],
                storage_texture_entry(
                    3,
                    wgpu::TextureFormat::R32Float,
                    wgpu::StorageTextureAccess::WriteOnly,
                ),
            ],
        )
    });

    let bgl1 = reused_layout(demosaic_start_for_programs).unwrap_or_else(|| {
        create_bind_group_layout(
            device,
            "bgl1",
            &[
                common_entries[0],
                common_entries[1],
                common_entries[2],
                common_entries[3],
                texture_entry(3, wgpu::TextureSampleType::Float { filterable: false }),
                storage_texture_entry(4, demosaic_format, wgpu::StorageTextureAccess::WriteOnly),
            ],
        )
    });

    let bgl2 = reused_layout(demosaic_start_for_programs + 1).unwrap_or_else(|| {
        create_bind_group_layout(
            device,
            "bgl2",
            &[
                common_entries[0],
                common_entries[1],
                common_entries[2],
                common_entries[3],
                texture_entry(3, wgpu::TextureSampleType::Float { filterable: false }),
                texture_entry(5, wgpu::TextureSampleType::Float { filterable: false }),
                storage_texture_entry(6, demosaic_format, wgpu::StorageTextureAccess::WriteOnly),
            ],
        )
    });

    let bgl3 = reused_layout(demosaic_start_for_programs + 2).unwrap_or_else(|| {
        create_bind_group_layout(
            device,
            "bgl3",
            &[
                common_entries[0],
                common_entries[1],
                common_entries[2],
                common_entries[3],
                texture_entry(3, wgpu::TextureSampleType::Float { filterable: false }),
                texture_entry(7, wgpu::TextureSampleType::Float { filterable: false }),
                storage_texture_entry(8, demosaic_format, wgpu::StorageTextureAccess::WriteOnly),
            ],
        )
    });

    let bgl_dual_green = reused_layout(dual_green_for_programs).unwrap_or_else(|| {
        create_bind_group_layout(
            device,
            "bgl dual demosaic green",
            &[
                common_entries[0],
                common_entries[1],
                common_entries[2],
                common_entries[3],
                texture_entry(3, wgpu::TextureSampleType::Float { filterable: false }),
                storage_texture_entry(20, demosaic_format, wgpu::StorageTextureAccess::WriteOnly),
            ],
        )
    });

    let bgl_dual_rgb = reused_layout(dual_rgb_for_programs).unwrap_or_else(|| {
        create_bind_group_layout(
            device,
            "bgl dual demosaic rgb",
            &[
                common_entries[0],
                common_entries[1],
                common_entries[2],
                common_entries[3],
                texture_entry(3, wgpu::TextureSampleType::Float { filterable: false }),
                texture_entry(21, wgpu::TextureSampleType::Float { filterable: false }),
                storage_texture_entry(22, demosaic_format, wgpu::StorageTextureAccess::WriteOnly),
            ],
        )
    });

    let bgl4 = (matches!(cfa_kind, CfaKind::Bayer)
        .then(|| reused_layout(demosaic_finish_for_programs))
        .flatten())
    .unwrap_or_else(|| {
        create_bind_group_layout(
            device,
            "bgl4",
            &[
                common_entries[0],
                common_entries[1],
                common_entries[2],
                common_entries[3],
                texture_entry(3, wgpu::TextureSampleType::Float { filterable: false }),
                texture_entry(7, wgpu::TextureSampleType::Float { filterable: false }),
                texture_entry(9, wgpu::TextureSampleType::Float { filterable: false }),
                texture_entry(23, wgpu::TextureSampleType::Float { filterable: false }),
                storage_texture_entry(10, demosaic_format, wgpu::StorageTextureAccess::WriteOnly),
            ],
        )
    });

    let bgl_xtrans_derivatives = (matches!(cfa_kind, CfaKind::XTrans)
        .then(|| reused_layout(demosaic_start_for_programs + 4))
        .flatten())
    .unwrap_or_else(|| {
        create_bind_group_layout(
            device,
            "bgl X-Trans derivatives",
            &[
                common_entries[0],
                common_entries[1],
                common_entries[2],
                common_entries[3],
                texture_entry(3, wgpu::TextureSampleType::Float { filterable: false }),
                texture_entry(9, wgpu::TextureSampleType::Float { filterable: false }),
                storage_texture_entry(20, demosaic_format, wgpu::StorageTextureAccess::WriteOnly),
                storage_texture_entry(21, demosaic_format, wgpu::StorageTextureAccess::WriteOnly),
            ],
        )
    });

    let bgl_xtrans_homogeneity = (matches!(cfa_kind, CfaKind::XTrans)
        .then(|| reused_layout(demosaic_start_for_programs + 5))
        .flatten())
    .unwrap_or_else(|| {
        create_bind_group_layout(
            device,
            "bgl X-Trans homogeneity",
            &[
                common_entries[0],
                common_entries[1],
                common_entries[2],
                common_entries[3],
                texture_entry(3, wgpu::TextureSampleType::Float { filterable: false }),
                texture_entry(27, wgpu::TextureSampleType::Float { filterable: false }),
                texture_entry(28, wgpu::TextureSampleType::Float { filterable: false }),
                storage_texture_entry(24, demosaic_format, wgpu::StorageTextureAccess::WriteOnly),
                storage_texture_entry(25, demosaic_format, wgpu::StorageTextureAccess::WriteOnly),
            ],
        )
    });

    let bgl_xtrans_accumulate = (matches!(cfa_kind, CfaKind::XTrans)
        .then(|| reused_layout(demosaic_start_for_programs + 6))
        .flatten())
    .unwrap_or_else(|| {
        create_bind_group_layout(
            device,
            "bgl X-Trans accumulate",
            &[
                common_entries[0],
                common_entries[1],
                common_entries[2],
                common_entries[3],
                texture_entry(3, wgpu::TextureSampleType::Float { filterable: false }),
                texture_entry(9, wgpu::TextureSampleType::Float { filterable: false }),
                texture_entry(29, wgpu::TextureSampleType::Float { filterable: false }),
                texture_entry(30, wgpu::TextureSampleType::Float { filterable: false }),
                storage_texture_entry(26, demosaic_format, wgpu::StorageTextureAccess::WriteOnly),
            ],
        )
    });

    let bgl_xtrans_finish = (matches!(cfa_kind, CfaKind::XTrans)
        .then(|| reused_layout(demosaic_finish_for_programs))
        .flatten())
    .unwrap_or_else(|| {
        create_bind_group_layout(
            device,
            "bgl X-Trans finish",
            &[
                common_entries[0],
                common_entries[1],
                common_entries[2],
                common_entries[3],
                texture_entry(3, wgpu::TextureSampleType::Float { filterable: false }),
                texture_entry(26, wgpu::TextureSampleType::Float { filterable: false }),
                texture_entry(23, wgpu::TextureSampleType::Float { filterable: false }),
                storage_texture_entry(10, demosaic_format, wgpu::StorageTextureAccess::WriteOnly),
            ],
        )
    });

    let bgl_color_denoise = reused_layout(color_denoise_for_programs).unwrap_or_else(|| {
        create_bind_group_layout(
            device,
            "bgl multiscale color denoise",
            &[
                buffer_entry(0),
                storage_texture_entry(10, demosaic_format, wgpu::StorageTextureAccess::WriteOnly),
                texture_entry(11, wgpu::TextureSampleType::Float { filterable: false }),
            ],
        )
    });

    let bgl_tone_prepare = reused_layout(tone_prepare_for_programs).unwrap_or_else(|| {
        create_bind_group_layout(
            device,
            "bgl tone prepare",
            &[
                buffer_entry(0),
                texture_entry(11, wgpu::TextureSampleType::Float { filterable: false }),
                storage_buffer_entry(15, false),
                storage_buffer_entry(20, true),
                storage_texture_entry(18, tone_format, wgpu::StorageTextureAccess::WriteOnly),
            ],
        )
    });

    let bgl_tone_blur = reused_layout(tone_prepare_for_programs + 1).unwrap_or_else(|| {
        create_bind_group_layout(
            device,
            "bgl tone guide blur",
            &[
                buffer_entry(0),
                texture_entry(17, wgpu::TextureSampleType::Float { filterable: false }),
                storage_texture_entry(18, tone_format, wgpu::StorageTextureAccess::WriteOnly),
            ],
        )
    });

    let bgl_tone_reduce = reused_layout(tone_prepare_for_programs + 3).unwrap_or_else(|| {
        create_bind_group_layout(
            device,
            "bgl tone histogram reduction",
            &[
                storage_buffer_entry(15, false),
                storage_buffer_entry(16, false),
            ],
        )
    });

    let bgl_adjust_prepare = reused_layout(adjustment_prepare_for_programs).unwrap_or_else(|| {
        create_bind_group_layout(
            device,
            "bgl scene preparation",
            &[
                common_entries[0],
                common_entries[1],
                common_entries[2],
                common_entries[3],
                texture_entry(11, wgpu::TextureSampleType::Float { filterable: false }),
                storage_texture_entry(21, work_format, wgpu::StorageTextureAccess::WriteOnly),
                storage_buffer_entry(16, true),
                texture_entry(17, wgpu::TextureSampleType::Float { filterable: false }),
                storage_buffer_entry(20, true),
                texture_array_entry(27, wgpu::TextureSampleType::Float { filterable: true }),
                sampler_entry(28),
                storage_buffer_entry(33, true),
            ],
        )
    });

    let bgl_adjust_tone = reused_layout(adjustment_prepare_for_programs + 1).unwrap_or_else(|| {
        create_bind_group_layout(
            device,
            "bgl scene tone edits",
            &[
                buffer_entry(0),
                texture_entry(22, wgpu::TextureSampleType::Float { filterable: false }),
                storage_texture_entry(23, work_format, wgpu::StorageTextureAccess::WriteOnly),
                storage_buffer_entry(16, true),
                texture_entry(17, wgpu::TextureSampleType::Float { filterable: false }),
                storage_buffer_entry(20, true),
                texture_array_entry(27, wgpu::TextureSampleType::Float { filterable: true }),
                sampler_entry(28),
                storage_buffer_entry(33, true),
            ],
        )
    });

    let bgl_adjust_effects =
        reused_layout(adjustment_prepare_for_programs + 3).unwrap_or_else(|| {
            create_bind_group_layout(
                device,
                "bgl scene presence and color",
                &[
                    buffer_entry(0),
                    texture_entry(22, wgpu::TextureSampleType::Float { filterable: false }),
                    storage_texture_entry(23, work_format, wgpu::StorageTextureAccess::WriteOnly),
                    storage_buffer_entry(16, true),
                    texture_array_entry(27, wgpu::TextureSampleType::Float { filterable: true }),
                    sampler_entry(28),
                    storage_buffer_entry(33, true),
                ],
            )
        });

    let bgl_mask_blur = reused_layout(adjustment_prepare_for_programs + 5).unwrap_or_else(|| {
        create_bind_group_layout(
            device,
            "bgl mask Blur diffusion",
            &[
                buffer_entry(0),
                texture_entry(24, wgpu::TextureSampleType::Float { filterable: false }),
                storage_texture_entry(25, work_format, wgpu::StorageTextureAccess::WriteOnly),
                texture_array_entry(27, wgpu::TextureSampleType::Float { filterable: true }),
                sampler_entry(28),
                storage_buffer_entry(33, true),
            ],
        )
    });

    let bgl_glow_prepare =
        reused_layout(adjustment_prepare_for_programs + 10).unwrap_or_else(|| {
            create_bind_group_layout(
                device,
                "bgl Glow source extraction",
                &[
                    buffer_entry(0),
                    texture_entry(24, wgpu::TextureSampleType::Float { filterable: false }),
                    storage_texture_entry(31, work_format, wgpu::StorageTextureAccess::WriteOnly),
                    texture_array_entry(27, wgpu::TextureSampleType::Float { filterable: true }),
                    sampler_entry(28),
                    storage_buffer_entry(33, true),
                ],
            )
        });

    let bgl_glow_blur = reused_layout(adjustment_prepare_for_programs + 11).unwrap_or_else(|| {
        create_bind_group_layout(
            device,
            "bgl Glow diffusion",
            &[
                buffer_entry(0),
                texture_entry(30, wgpu::TextureSampleType::Float { filterable: false }),
                storage_texture_entry(31, work_format, wgpu::StorageTextureAccess::WriteOnly),
            ],
        )
    });

    let bgl_pixelate_blocks =
        reused_layout(adjustment_prepare_for_programs + 16).unwrap_or_else(|| {
            create_bind_group_layout(
                device,
                "bgl Pixelate block averages",
                &[
                    buffer_entry(0),
                    texture_entry(24, wgpu::TextureSampleType::Float { filterable: false }),
                    storage_texture_entry(37, work_format, wgpu::StorageTextureAccess::WriteOnly),
                    storage_buffer_entry(33, true),
                ],
            )
        });

    let bgl_adjust_creative =
        reused_layout(adjustment_prepare_for_programs + 17).unwrap_or_else(|| {
            create_bind_group_layout(
                device,
                "bgl creative glow",
                &[
                    buffer_entry(0),
                    texture_entry(24, wgpu::TextureSampleType::Float { filterable: false }),
                    storage_texture_entry(25, work_format, wgpu::StorageTextureAccess::WriteOnly),
                    texture_entry(30, wgpu::TextureSampleType::Float { filterable: false }),
                    texture_array_entry(27, wgpu::TextureSampleType::Float { filterable: true }),
                    sampler_entry(28),
                    storage_buffer_entry(33, true),
                    texture_array_entry(34, wgpu::TextureSampleType::Float { filterable: true }),
                    texture_entry(35, wgpu::TextureSampleType::Float { filterable: true }),
                    storage_buffer_entry(16, true),
                    texture_entry(36, wgpu::TextureSampleType::Float { filterable: false }),
                    texture_entry(45, wgpu::TextureSampleType::Float { filterable: true }),
                ],
            )
        });

    let bgl_adjust_render = create_bind_group_layout(
        device,
        "bgl scene look view and output",
        &[
            buffer_entry(0),
            storage_texture_entry(
                12,
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::StorageTextureAccess::WriteOnly,
            ),
            texture_entry(26, wgpu::TextureSampleType::Float { filterable: false }),
            storage_buffer_entry(16, true),
            storage_buffer_entry(20, true),
            texture_array_entry(27, wgpu::TextureSampleType::Float { filterable: true }),
            sampler_entry(28),
            storage_texture_entry(29, work_format, wgpu::StorageTextureAccess::WriteOnly),
            storage_buffer_entry(33, true),
        ],
    );
    let bgl_adjust_render =
        reused_layout(adjustment_prepare_for_programs + 18).unwrap_or(bgl_adjust_render);

    // Image-light passes follow every other pass (`assemble_passes`).
    let image_light_for_programs = adjustment_prepare_for_programs + 19;
    let image_light_write = |binding| {
        storage_texture_entry(
            binding,
            IMAGE_LIGHT_FORMAT,
            wgpu::StorageTextureAccess::WriteOnly,
        )
    };
    let image_light_read = |binding| {
        texture_entry(
            binding,
            wgpu::TextureSampleType::Float { filterable: false },
        )
    };
    let bgl_image_light_accumulate = reused_layout(image_light_for_programs).unwrap_or_else(|| {
        create_bind_group_layout(
            device,
            "bgl image-light accumulation",
            &[
                buffer_entry(0),
                texture_entry(11, wgpu::TextureSampleType::Float { filterable: false }),
                storage_buffer_entry(20, true),
                storage_buffer_entry(38, false),
            ],
        )
    });
    let bgl_image_light_resolve =
        reused_layout(image_light_for_programs + 1).unwrap_or_else(|| {
            create_bind_group_layout(
                device,
                "bgl image-light resolve",
                &[
                    buffer_entry(0),
                    storage_buffer_entry(16, false),
                    storage_buffer_entry(38, false),
                    image_light_write(39),
                ],
            )
        });
    let bgl_image_light_blur_horizontal = reused_layout(image_light_for_programs + 2)
        .unwrap_or_else(|| {
            create_bind_group_layout(
                device,
                "bgl image-light horizontal blur",
                &[
                    buffer_entry(0),
                    image_light_read(40),
                    image_light_write(41),
                    image_light_write(42),
                ],
            )
        });
    let bgl_image_light_blur_vertical =
        reused_layout(image_light_for_programs + 3).unwrap_or_else(|| {
            create_bind_group_layout(
                device,
                "bgl image-light vertical blur",
                &[
                    buffer_entry(0),
                    image_light_read(43),
                    image_light_read(44),
                    image_light_write(39),
                ],
            )
        });

    BindGroupLayouts {
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
        bgl_pixelate_blocks,
        bgl_adjust_creative,
        bgl_adjust_render,
        bgl_image_light_accumulate,
        bgl_image_light_resolve,
        bgl_image_light_blur_horizontal,
        bgl_image_light_blur_vertical,
    }
}
