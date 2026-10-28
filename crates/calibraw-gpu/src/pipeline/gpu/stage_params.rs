//! Packing of camera, scene-tone and effects uniforms for one render stage.

use super::*;

pub(super) fn pack_camera_params(ctx: &GpuParamContext<'_>) -> CameraUniforms {
    let exposure = ctx.exposure;
    let raw = ctx.raw;
    let GpuTileInfo {
        origin_x: tile_origin_x,
        origin_y: tile_origin_y,
        full_width,
        full_height,
    } = ctx.tile;
    let color = ctx.color;
    let (temperature, tint) = color.white_balance_offsets(exposure.white_balance);
    let (white_balance, camera_transform, profile_weight) = color
        .adjusted_white_balance_and_camera_transform(
            raw.wb_coeffs,
            temperature.clamp(-GLOBAL_TEMPERATURE_LIMIT, GLOBAL_TEMPERATURE_LIMIT),
            tint.clamp(-GLOBAL_TINT_OFFSET_LIMIT, GLOBAL_TINT_OFFSET_LIMIT),
        );
    let camera_linear_raster = raw.is_camera_linear_raster();
    // LinearRaw DNGs arrive as a raster of camera-space RGB samples. The
    // raster shader does not have the CFA sampler's per-channel WB multiply,
    // so fold the current WB into the camera transform exactly once here.
    let camera_transform = if camera_linear_raster {
        let mut transform = camera_transform;
        for row in &mut transform {
            for (column, gain) in white_balance.iter().enumerate() {
                row[column] *= *gain;
            }
        }
        transform
    } else {
        camera_transform
    };
    let mut profile_layout = color.camera_profile.gpu_layout();
    profile_layout.flags[3] = profile_weight.clamp(0.0, 1.0).to_bits();
    let profile_stages = profile_layout.stages();
    debug_assert_eq!(
        profile_stages.characterization.hue_sat_2,
        profile_layout.hue_sat_2
    );
    let highlight_method = if raw.is_pre_demosaiced_raster() {
        0.0
    } else {
        shader_highlight_method(raw.cfa_kind, exposure.highlight_method)
    };
    let opposed_chroma = if highlight_method >= 1.5 {
        raw.inpaint_opposed_chroma(
            exposure.black_point,
            exposure.highlight_clip,
            exposure.ai_denoise_enabled,
            white_balance,
        )
    } else {
        [0.0; 3]
    };

    let chromatic_aberration = if exposure.chromatic_aberration.is_finite() {
        (exposure.chromatic_aberration / 100.0).clamp(0.0, 1.0)
    } else {
        0.0
    };
    // The fringe pass takes colour relative to neutral. A camera-linear
    // raster's scene is camera RGB before white balance, which the camera
    // transform above applies instead; every other scene is balanced already.
    let neutral_gains = if camera_linear_raster {
        [white_balance[0], white_balance[1], white_balance[2]]
    } else {
        [1.0; 3]
    };
    let (film_base, film_exponents, film_gains) =
        pack_film(&exposure.film, raw, white_balance, camera_linear_raster);
    // The lateral shift grows linearly with the amount.
    let lateral_chromatic_aberration = |channel| {
        raw.lateral_chromatic_aberration
            .coefficients(channel)
            .map(|coefficient| coefficient * chromatic_aberration)
    };

    CameraUniforms {
        black_point: exposure.black_point,
        // The shaders compare normalized sensor values against it, so it is scaled by where
        // the photo's photosites saturate. `inpaint_opposed_chroma` scales its own threshold.
        highlight_clip: raw.sensor_saturation.clip_level(exposure.highlight_clip),
        chroma_denoise: exposure.chroma_denoise,
        highlight_reconstruction: exposure.highlight_reconstruction,
        tone_analysis_scale: tone_analysis_scale() as f32,
        tone_guide_radius: crate::pipeline::TONE_GUIDE_RADIUS_CELLS as f32,
        demosaic_mode: exposure.demosaic_mode.shader_value(),
        dual_threshold: exposure.dual_threshold.clamp(0.0, 100.0),
        frequency_chroma: exposure.frequency_chroma.clamp(0.0, 1.0),
        view_transform_mode: ViewTransformMode::select(raw, exposure, camera_linear_raster)
            .shader_value(),
        padding_0: 0,
        padding_1: 0,
        highlight_options: [
            highlight_method,
            opposed_chroma[0],
            opposed_chroma[1],
            opposed_chroma[2],
        ],
        noise_shot: canonicalize_green_noise(
            std::array::from_fn(|channel| {
                raw.noise_profile.shot[channel] * white_balance[channel].max(0.0)
            }),
            raw.noise_profile.green2_present,
        ),
        noise_read: canonicalize_green_noise(
            std::array::from_fn(|channel| {
                let wb = white_balance[channel].max(0.0);
                raw.noise_profile.read[channel] * wb * wb
            }),
            raw.noise_profile.green2_present,
        ),
        noise_options: [
            (exposure.luminance_denoise / 100.0).clamp(0.0, 1.0),
            (exposure.denoise_detail / 100.0).clamp(0.0, 1.0),
            exposure.denoise_quality.shader_value(),
            raw.noise_profile.confidence.clamp(0.0, 1.0),
        ],
        wb: white_balance,
        cam_to_srgb_0: camera_transform[0],
        cam_to_srgb_1: camera_transform[1],
        cam_to_srgb_2: camera_transform[2],
        black_levels: raw.black_levels,
        white_levels: raw.white_levels,
        width: raw.width,
        height: raw.height,
        tile_origin_x,
        tile_origin_y,
        full_width,
        full_height,
        abi_version: GPU_PARAMS_ABI_VERSION,
        abi_size_bytes: GPU_PARAMS_ABI_SIZE_BYTES,
        tone_histogram_bounds: [0, 0, full_width, full_height],
        profile_hue_sat: profile_stages.characterization.hue_sat,
        profile_look: profile_stages.optional_look.look_table,
        profile_tone: profile_stages.view.profile_tone,
        profile_flags: profile_layout.flags,
        ai_denoise_enabled: u32::from(exposure.ai_denoise_enabled),
        user_exposure_bits: exposure.exposure.to_bits(),
        ai_cfa_weight: if exposure.ai_denoise_enabled {
            exposure.ai_denoise_amount.clamp(0.0, 1.0)
        } else {
            0.0
        },
        source_short_edge: raw.source_extent[0].min(raw.source_extent[1]),
        chromatic_aberration: [
            chromatic_aberration,
            neutral_gains[0],
            neutral_gains[1],
            neutral_gains[2],
        ],
        lateral_ca_red: lateral_chromatic_aberration(LateralChannel::Red),
        lateral_ca_blue: lateral_chromatic_aberration(LateralChannel::Blue),
        film_base,
        film_exponents,
        film_gains,
    }
}

/// The film-negative uniforms of `film` (`film_positive` in
/// scene_source.wgsl): base, exponents and gains. `white_balance` holds the
/// multipliers of this render. A mosaic's scene texture carries them, so the
/// base is scaled into its units and the division by the base cancels them;
/// a raster's scene does not. The positive then takes the white balance
/// relative to the as-shot one, which leaves an as-shot positive neutral on
/// the base, less what a camera-linear raster's camera transform applies.
fn pack_film(
    film: &FilmSource,
    raw: &LoadedRaw,
    white_balance: [f32; 4],
    camera_linear_raster: bool,
) -> ([f32; 4], [f32; 4], [f32; 4]) {
    let Some(negative) = film.negative().map(|negative| negative.sanitized()) else {
        return (
            [0.0, 0.0, 0.0, FILM_MODE_POSITIVE as f32],
            [0.0; 4],
            [1.0, 1.0, 1.0, 0.0],
        );
    };
    let mode = match negative.stock {
        FilmStock::Color => FILM_MODE_COLOR,
        FilmStock::BlackAndWhite => FILM_MODE_MONOCHROME,
    };
    let usable = |gain: f32| gain.is_finite() && gain > 0.0;
    let multipliers: [f32; 3] = std::array::from_fn(|channel| {
        let gain = white_balance[channel];
        if usable(gain) {
            gain
        } else {
            1.0
        }
    });
    // 1 where the photo has no as-shot multiplier.
    let relative: [f32; 3] = std::array::from_fn(|channel| {
        let as_shot = raw.wb_coeffs[channel];
        if usable(as_shot) {
            multipliers[channel] / as_shot
        } else {
            1.0
        }
    });
    let scene_gains = if raw.is_pre_demosaiced_raster() {
        [1.0; 3]
    } else {
        multipliers
    };
    let transform_gains = if camera_linear_raster {
        multipliers
    } else {
        [1.0; 3]
    };
    let [red, green, blue] = negative.base;
    let [red_exponent, green_exponent, blue_exponent] = negative.exponents;
    (
        [
            red * scene_gains[0],
            green * scene_gains[1],
            blue * scene_gains[2],
            mode as f32,
        ],
        [
            red_exponent,
            green_exponent,
            blue_exponent,
            negative.print_exposure,
        ],
        [
            relative[0] / relative[1] / transform_gains[0],
            1.0 / transform_gains[1],
            relative[2] / relative[1] / transform_gains[2],
            0.0,
        ],
    )
}

pub(super) fn pack_scene_tone_params(ctx: &GpuParamContext<'_>) -> SceneToneUniforms {
    let exposure = ctx.exposure;
    let masks = ctx.masks;
    let (sigmoid_curve, sigmoid_power) = pack_sigmoid(tone_sigmoid_coefficients(exposure));
    let (reference_sigmoid_curve, reference_sigmoid_power) =
        pack_sigmoid(reference_sigmoid_coefficients());
    let (hsl_hue_0, hsl_hue_1) = split_eight(exposure.hsl_hue);
    let (hsl_saturation_0, hsl_saturation_1) = split_eight(exposure.hsl_saturation);
    let (hsl_luminance_0, hsl_luminance_1) = split_eight(exposure.hsl_luminance);
    let local_point_color_flags = u32::from(masks.masks.iter().any(|mask| {
        mask.enabled
            && mask.adjustments_enabled
            && mask.effect.uses_adjustments()
            && mask.adjustments.point_colors.has_adjustments()
    })) | (u32::from(masks.masks.iter().any(|mask| {
        mask.enabled
            && mask.adjustments_enabled
            && mask.effect.uses_adjustments()
            && mask.adjustments.point_color_visualize.is_some()
    })) << 1);
    let tone_curves = [
        &exposure.tone_curve,
        &exposure.tone_curve_red,
        &exposure.tone_curve_green,
        &exposure.tone_curve_blue,
    ]
    .map(pack_local_point_curve);
    let mut point_colors = [PackedPointColor::zeroed(); MAX_POINT_COLORS];
    for (destination, point) in point_colors.iter_mut().zip(exposure.point_colors.iter()) {
        *destination = pack_point_color(*point);
    }

    SceneToneUniforms {
        exposure: exposure.exposure,
        saturation: exposure.saturation,
        vibrance: exposure.vibrance,
        scene_depth_present: u32::from(masks.scene_depth_image().is_some_and(valid_scene_depth)),
        basic_tone: [
            exposure.highlights,
            exposure.shadows,
            exposure.whites,
            exposure.blacks,
        ],
        sigmoid_curve,
        sigmoid_power,
        reference_sigmoid_curve,
        reference_sigmoid_power,
        tone_curves,
        hsl_hue_0,
        hsl_hue_1,
        hsl_saturation_0,
        hsl_saturation_1,
        hsl_luminance_0,
        hsl_luminance_1,
        mask_counts: [
            render_mask_slot_count(masks).min(MAX_RENDER_MASK_SLOTS) as u32,
            0,
            0,
            0,
        ],
        grade_shadows: pack_color_grade_wheel(exposure.color_grading.shadows),
        grade_midtones: pack_color_grade_wheel(exposure.color_grading.midtones),
        grade_highlights: pack_color_grade_wheel(exposure.color_grading.highlights),
        grade_global: pack_color_grade_wheel(exposure.color_grading.global),
        grade_options: pack_view_color_options(exposure.color_grading, exposure.hue),
        rec2020_to_xyz: [
            [0.636_958, 0.262_700_2, 0.0, 0.0],
            [0.144_616_9, 0.677_998_1, 0.028_072_7, 0.0],
            [0.168_880_9, 0.059_301_7, 1.060_985_1, 0.0],
        ],
        xyz_to_rec2020: [
            [1.716_651_2, -0.666_684_4, 0.017_639_9, 0.0],
            [-0.355_670_8, 1.616_481_2, -0.042_770_6, 0.0],
            [-0.253_366_3, 0.015_768_5, 0.942_103_1, 0.0],
        ],
        xyz_to_bradford: [
            [0.8951, -0.7502, 0.0389, 0.0],
            [0.2664, 1.7135, -0.0685, 0.0],
            [-0.1614, 0.0367, 1.0296, 0.0],
        ],
        bradford_to_xyz: [
            [0.986_992_9, 0.432_305_3, -0.008_528_7, 0.0],
            [-0.147_054_3, 0.518_360_3, 0.040_042_8, 0.0],
            [0.159_962_7, 0.049_291_2, 0.968_486_7, 0.0],
        ],
        point_colors,
        point_color_meta: [
            exposure.point_colors.len() as u32,
            exposure
                .point_color_visualize
                .map_or(0, |index| (index + 1) as u32),
            0,
            local_point_color_flags | (u32::from(exposure.point_colors.has_adjustments()) << 2),
        ],
        // Set by `GpuParams::with_look_lut`.
        look_lut: [0.0; 4],
        look_lut_domain_min: [0.0; 4],
        look_lut_domain_scale: [0.0; 4],
    }
}

/// The `sigmoid_curve` and `sigmoid_power` lanes read by tonemap.wgsl.
fn pack_sigmoid(sigmoid: SigmoidCoefficients) -> ([f32; 4], [f32; 4]) {
    (
        [
            sigmoid.white_target,
            sigmoid.black_target,
            sigmoid.paper_exposure,
            sigmoid.film_fog,
        ],
        [
            sigmoid.film_power,
            sigmoid.paper_power,
            sigmoid.hue_preservation,
            sigmoid.color_processing,
        ],
    )
}

pub(super) fn pack_effect_params(
    ctx: &GpuParamContext<'_>,
    mask_data: &[MaskData],
) -> EffectsUniforms {
    let exposure = ctx.exposure;
    let GpuTileInfo {
        full_width,
        full_height,
        ..
    } = ctx.tile;
    // Self-illuminating Glow masks emit into the shared Glow diffusion, which
    // spreads as far as the widest of them.
    let glow_diffusion_radius = mask_data
        .iter()
        .filter(|mask| is_self_illuminating_glow(mask))
        .map(|mask| mask.adjust_0[1])
        .fold(0.0_f32, f32::max);
    EffectsUniforms {
        presence: [exposure.texture, exposure.clarity, exposure.dehaze, 0.0],
        sharpen: [
            exposure.sharpen_amount.clamp(0.0, 150.0),
            exposure.sharpen_radius.clamp(0.5, 3.0),
            exposure.sharpen_detail.clamp(0.0, 100.0),
            exposure.sharpen_masking.clamp(0.0, 100.0),
        ],
        glow_diffusion: [glow_diffusion_radius, 0.0, 0.0, 0.0],
        vignette_frame: [
            0.5,
            0.5,
            full_width.max(1) as f32,
            full_height.max(1) as f32,
        ],
        vignette_transform: [1.0, 0.0, 0.0, 1.0],
        vignette_dark_half_fit: [0.10, 1.235, 2.88, 0.86],
        vignette_dark_full_fit: [0.02, 1.135, 3.46, 1.0],
        vignette_light_half_fit: [0.305, 1.24, 4.36, 0.90],
        vignette_light_full_fit: [0.13, 1.075, 5.66, 1.0],
        capture_scale_sigma: [0.74, 1.75, 0.58, 1.65],
        capture_thresholds: [0.015, 0.0045, 0.055, 0.28],
        capture_mask_coherence: [0.035, 0.62, 0.055, 0.22],
    }
}
