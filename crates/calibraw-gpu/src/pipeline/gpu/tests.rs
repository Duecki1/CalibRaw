use super::{
    pack_effect_mask, pack_local_point_curve, pack_point_curve, processing_work_format,
    shader_manager::ShaderManager, work_shader_source, GpuParams, ProcessingQuality,
    RawGpuPipeline, SHADER_BAYER_RCD_P1, SHADER_BAYER_RCD_P2, SHADER_BAYER_RCD_P3,
    SHADER_BAYER_RCD_P4, SHADER_COLOR_DENOISE, SHADER_CREATIVE_EFFECTS, SHADER_DUAL_DEMOSAIC,
    SHADER_HIGHLIGHTS, SHADER_RAW_SAMPLING, SHADER_REMOVE_COMPOSITE, SHADER_SCENE_ADJUSTMENTS,
    SHADER_TONEMAP, SHADER_TONE_ANALYSIS, SHADER_VIEW_TRANSFORM, SHADER_XTRANS_DEMOSAIC,
    SHADER_XTRANS_FINISH,
};
use crate::pipeline::{
    extract_padded_tile, CameraProfile, CfaKind, CompactPixelMap, ExportTile, ExposureParams,
    HighlightReconstructionMethod, LoadedRaw, LocalMask, MaskEffect, MaskKind, MaskStack,
    NativeRect, PointCurve, ProcessingStage, TONE_GUIDE_CELL_SIZE,
};

#[test]
fn point_color_shader_uses_display_srgb_hsl_and_combined_unadjusted_selection() {
    assert!(SHADER_VIEW_TRANSFORM
        .contains("fn apply_point_colors(input_rgb: vec3<f32>, selection_sample: vec3<f32>)"));
    assert!(SHADER_VIEW_TRANSFORM
        .contains("fn point_color_selection_weight(sample: vec3<f32>, index: u32)"));
    assert!(SHADER_VIEW_TRANSFORM.contains("point_color_hue_weight"));
    assert!(SHADER_VIEW_TRANSFORM.contains("point_color_hsl_to_rgb"));
    assert!(SHADER_VIEW_TRANSFORM
        .contains("display_linear = apply_point_colors(display_linear, point_color_sample)"));
    assert!(SHADER_VIEW_TRANSFORM
        .contains("color_delta = color_delta + (adjusted - input_rgb) * weight"));
}

#[test]
fn point_color_visualization_matches_the_mask_overlay_style() {
    assert!(SHADER_VIEW_TRANSFORM
        .contains("let overlay_rgb = vec3<f32>(78.0 / 255.0, 163.0 / 255.0, 1.0);"));
    assert!(SHADER_VIEW_TRANSFORM.contains("let overlay_alpha = selected_weight * (92.0 / 255.0);"));
    assert!(
        SHADER_VIEW_TRANSFORM.contains("output_rgb = mix(output_rgb, overlay_rgb, overlay_alpha);")
    );
    assert!(!SHADER_VIEW_TRANSFORM
        .contains("adjusted = mix(vec3<f32>(luminance), adjusted, selected_weight);"));
}

#[test]
fn local_point_colors_pack_with_mask_adjustments() {
    let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
    let mut point = crate::pipeline::PointColor::from_srgb([0.8, 0.2, 0.1]);
    point.hue_shift = 25.0;
    mask.adjustments.point_colors.push(point);
    mask.adjustments.point_color_visualize = Some(0);
    let packed = super::pack_adjustment_mask(&mask);
    assert_eq!(packed.metadata[0], 1);
    assert_eq!(packed.metadata[1], 1);
    assert_eq!(packed.point_color_meta[0..2], [1, 1]);
    assert!((packed.point_colors[0].shifts[0] - 0.125).abs() < 1e-6);
}

fn validate_shader(name: &str, source: &str, quality: ProcessingQuality) {
    let format = processing_work_format(quality);
    let mut manager = ShaderManager::new(
        format,
        if name.starts_with("X-Trans") {
            CfaKind::XTrans
        } else {
            CfaKind::Bayer
        },
    )
    .unwrap();
    let source = match quality {
        ProcessingQuality::Preview => std::borrow::Cow::Borrowed(source),
        ProcessingQuality::High if source.contains("CALIBRAW_WORK_FORMAT") => {
            work_shader_source(source, format).unwrap()
        }
        ProcessingQuality::High => std::borrow::Cow::Borrowed(source),
    };
    let module = manager
        .compose_naga_module(source.as_ref(), "shader_test.wgsl")
        .unwrap_or_else(|error| panic!("{name} did not compose: {error:#}"));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("{name} did not validate: {error}"));
}

fn shader_f32_const(source: &str, name: &str) -> f32 {
    let declaration = format!("const {name}: f32 = ");
    source
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix(&declaration)
                .and_then(|value| value.strip_suffix(';'))
        })
        .unwrap_or_else(|| panic!("shader constant {name} is missing"))
        .parse::<f32>()
        .unwrap_or_else(|error| panic!("shader constant {name} is not numeric: {error}"))
}

fn shared_highlight_sensor_clip_for_test(highlight_clip: f32) -> f32 {
    let safety = shader_f32_const(SHADER_RAW_SAMPLING, "SHARED_HIGHLIGHT_CLIP_SAFETY");
    safety * highlight_clip.max(0.01)
}

fn shared_highlight_channel_clip_for_test(
    highlight_clip: f32,
    wb: [f32; 4],
    channel: usize,
) -> f32 {
    shared_highlight_sensor_clip_for_test(highlight_clip) * wb[channel].max(1e-6)
}

fn adjacent_f32(value: f32, above: bool) -> f32 {
    assert!(value.is_finite() && value > 0.0);
    let bits = value.to_bits();
    f32::from_bits(if above { bits + 1 } else { bits - 1 })
}

#[test]
fn lch_and_rcd_share_sensor_space_highlight_clip_definition() {
    assert!(SHADER_RAW_SAMPLING.contains("fn shared_highlight_sensor_clip() -> f32"));
    assert!(
        SHADER_RAW_SAMPLING.contains("return raw_sensor_at(p) >= shared_highlight_sensor_clip();")
    );
    assert!(SHADER_RAW_SAMPLING
        .contains("fn shared_highlight_clip_for_cfa_channel(channel: u32) -> f32"));
    assert!(!SHADER_RAW_SAMPLING.contains("min_wb"));
    assert!(SHADER_HIGHLIGHTS.contains("#import calibraw::raw_sampling as RawSampling"));
    assert!(SHADER_HIGHLIGHTS.contains("clipped = clipped || RawSampling::is_raw_clipped(p);"));
    assert!(SHADER_HIGHLIGHTS
        .contains("RawSampling::shared_highlight_clip_for_cfa_channel(physical_channel)"));
    assert!(!SHADER_HIGHLIGHTS.contains("fn lch_common_clip()"));

    validate_shader("highlights", SHADER_HIGHLIGHTS, ProcessingQuality::Preview);
    validate_shader(
        "Bayer pass 2",
        SHADER_BAYER_RCD_P2,
        ProcessingQuality::Preview,
    );
}

#[test]
fn unequal_wb_does_not_move_raw_highlight_clipping_boundary() {
    let highlight_clip = 1.0;
    let wb = [2.2, 1.0, 1.6, 1.0];
    let sensor_clip = shared_highlight_sensor_clip_for_test(highlight_clip);
    let below = adjacent_f32(sensor_clip, false);
    let above = adjacent_f32(sensor_clip, true);

    for (channel, gain) in wb.into_iter().enumerate() {
        let channel_clip = shared_highlight_channel_clip_for_test(highlight_clip, wb, channel);
        assert_eq!(
            below >= sensor_clip,
            below * gain >= channel_clip,
            "channel={channel}"
        );
        assert_eq!(
            above >= sensor_clip,
            above * gain >= channel_clip,
            "channel={channel}"
        );
    }

    // The old min(wb) threshold incorrectly marked high-gain channels clipped
    // even though the underlying sensor sample was comfortably below white.
    let raw_sensor = 0.75 * sensor_clip;
    let old_min_wb_clip = sensor_clip * wb.into_iter().fold(f32::INFINITY, f32::min);
    assert!(raw_sensor < sensor_clip);
    assert!(raw_sensor * wb[0] >= old_min_wb_clip);
    assert!(raw_sensor * wb[2] >= old_min_wb_clip);
    assert!(raw_sensor * wb[0] < shared_highlight_channel_clip_for_test(highlight_clip, wb, 0));
    assert!(raw_sensor * wb[2] < shared_highlight_channel_clip_for_test(highlight_clip, wb, 2));
}

#[test]
fn opposed_sensor_and_channel_wb_clipping_are_equivalent_with_unequal_wb() {
    const DARKTABLE_OPPOSED_CLIP_MAGIC: f32 = 0.987;

    assert_eq!(
        shader_f32_const(SHADER_HIGHLIGHTS, "DARKTABLE_OPPOSED_CLIP_MAGIC"),
        DARKTABLE_OPPOSED_CLIP_MAGIC
    );
    assert!(SHADER_RAW_SAMPLING
        .contains("raw_sensor_at(p) >= 0.987 * max(Common::camera_uniforms.highlight_clip, 0.01)"));

    let highlight_clip = 1.03_f32;
    let wb = [2.2_f32, 1.0, 1.6, 1.0];
    let sensor_clip = DARKTABLE_OPPOSED_CLIP_MAGIC * highlight_clip.max(0.01);
    let sensor_values = [
        adjacent_f32(sensor_clip, false),
        sensor_clip,
        adjacent_f32(sensor_clip, true),
        0.5 * sensor_clip,
        1.5 * sensor_clip,
    ];

    for (channel, gain) in wb.into_iter().enumerate() {
        for raw_sensor in sensor_values {
            let raw_camera = raw_sensor * gain;
            let sensor_domain = raw_sensor >= sensor_clip;
            let wb_domain =
                raw_camera >= DARKTABLE_OPPOSED_CLIP_MAGIC * highlight_clip.max(0.01) * gain;
            assert_eq!(
                sensor_domain, wb_domain,
                "channel={channel}, wb={gain}, raw_sensor={raw_sensor}"
            );
        }
    }
}

#[test]
fn compute_shaders_validate() {
    for (name, source) in [
        ("highlights", SHADER_HIGHLIGHTS),
        ("Bayer pass 1", SHADER_BAYER_RCD_P1),
        ("Bayer pass 2", SHADER_BAYER_RCD_P2),
        ("Bayer pass 3", SHADER_BAYER_RCD_P3),
        ("Bayer pass 4", SHADER_BAYER_RCD_P4),
        ("dual demosaic", SHADER_DUAL_DEMOSAIC),
        ("X-Trans demosaic", SHADER_XTRANS_DEMOSAIC),
        ("X-Trans finish", SHADER_XTRANS_FINISH),
        ("color denoise", SHADER_COLOR_DENOISE),
        ("tone analysis", SHADER_TONE_ANALYSIS),
        ("scene adjustments", SHADER_SCENE_ADJUSTMENTS),
        ("creative effects", SHADER_CREATIVE_EFFECTS),
        ("Remove composite", SHADER_REMOVE_COMPOSITE),
        ("view transform", SHADER_VIEW_TRANSFORM),
    ] {
        validate_shader(name, source, ProcessingQuality::Preview);
    }
}

#[test]
fn high_quality_shaders_validate() {
    for (name, source) in [
        ("Bayer pass 1", SHADER_BAYER_RCD_P1),
        ("dual demosaic", SHADER_DUAL_DEMOSAIC),
        ("X-Trans demosaic", SHADER_XTRANS_DEMOSAIC),
        ("color denoise", SHADER_COLOR_DENOISE),
        ("Remove composite", SHADER_REMOVE_COMPOSITE),
        ("scene adjustments", SHADER_SCENE_ADJUSTMENTS),
        ("creative effects", SHADER_CREATIVE_EFFECTS),
    ] {
        validate_shader(name, source, ProcessingQuality::High);
    }
}

#[test]
fn point_curve_packing_is_shared_between_global_and_local_uniforms() {
    let curve = PointCurve {
        points: [
            [0.0, 0.0],
            [0.2, 0.1],
            [0.4, 0.5],
            [0.7, 0.8],
            [1.0, 1.0],
            [1.0, 1.0],
            [1.0, 1.0],
            [1.0, 1.0],
        ],
        len: 5,
    };

    let packed = pack_point_curve(&curve);
    assert_eq!(packed.pairs[0], [0.0, 0.0, 0.2, 0.1]);
    assert_eq!(packed.pairs[1], [0.4, 0.5, 0.7, 0.8]);
    assert_eq!(packed.pairs[2], [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(packed.meta, [5.0, 0.0, 0.0, 0.0]);

    let local = pack_local_point_curve(&curve);
    assert_eq!(&local[..4], &packed.pairs);
    assert_eq!(local[4], packed.meta);
    assert_eq!(&local[5..], &[[0.0; 4]; 3]);
}

#[test]
fn mask_effect_packing_preserves_shader_id_activity_and_clamps() {
    let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
    mask.effect = MaskEffect::Blur;
    mask.effect_settings.blur.amount = 150.0;
    mask.effect_settings.blur.radius = 99.0;

    let packed = pack_effect_mask(mask.effect, &mask.effect_settings, mask.enabled)
        .expect("Blur is a GPU-backed mask effect");
    assert_eq!(packed.metadata[0], 1);
    assert_eq!(packed.metadata[1], 1);
    assert_eq!(packed.metadata[2], 0);
    assert_eq!(
        packed.metadata[3] >> super::MASK_EFFECT_ID_SHIFT,
        MaskEffect::Blur.shader_id()
    );
    assert_eq!(packed.adjust_0, [100.0, 16.0, 0.0, 0.0]);

    mask.enabled = false;
    let disabled = pack_effect_mask(mask.effect, &mask.effect_settings, mask.enabled)
        .expect("Blur remains representable when disabled");
    assert_eq!(disabled.metadata[0], 0);
    assert_eq!(disabled.metadata[1], 0);
    assert_eq!(disabled.adjust_0, packed.adjust_0);
}

#[test]
fn effect_components_share_mask_layer_and_global_effects_cover_image() {
    assert!(super::SHADER_COMMON.contains(&format!(
        "const MAX_RENDER_MASK_SLOTS: u32 = {}u;",
        super::MAX_RENDER_MASK_SLOTS
    )));
    let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
    mask.adjustments.exposure = 0.5;
    let mut blur = crate::pipeline::EffectComponent::new(MaskEffect::Blur);
    blur.settings.blur.amount = 60.0;
    let mut glow = crate::pipeline::EffectComponent::new(MaskEffect::Glow);
    glow.settings.glow.amount = 40.0;
    mask.effect_components = vec![blur, glow];

    let mut global = crate::pipeline::EffectComponent::new(MaskEffect::Pixelate);
    global.settings.pixelate.amount = 25.0;
    let masks = MaskStack {
        masks: vec![mask],
        global_effects: vec![global],
        ..Default::default()
    };
    let packed = super::pack_mask_params(&masks);
    assert_eq!(super::render_mask_slot_count(&masks), 4);
    assert_eq!(packed[0].point_color_meta[2], 0);
    assert_eq!(packed[1].point_color_meta[2], 0);
    assert_eq!(packed[2].point_color_meta[2], 0);
    assert_eq!(packed[3].point_color_meta[2], u32::MAX);
    assert_eq!(
        packed[1].metadata[3] >> super::MASK_EFFECT_ID_SHIFT,
        MaskEffect::Blur.shader_id()
    );
    assert_eq!(
        packed[2].metadata[3] >> super::MASK_EFFECT_ID_SHIFT,
        MaskEffect::Glow.shader_id()
    );
    assert_eq!(
        packed[3].metadata[3] >> super::MASK_EFFECT_ID_SHIFT,
        MaskEffect::Pixelate.shader_id()
    );
}

#[test]
fn global_and_fullscreen_mask_effects_render_the_same_pixels() -> anyhow::Result<()> {
    let Some((device, queue)) = request_test_device() else {
        return Ok(());
    };
    const EDGE: u32 = 32;
    let pixels = (0..EDGE * EDGE)
        .flat_map(|index| {
            let value = if (index % EDGE / 4 + index / EDGE / 4) % 2 == 0 {
                0.1
            } else {
                0.8
            };
            [value; 3]
        })
        .collect();
    let source = LoadedRaw::from_scene_linear_rec2020(EDGE, EDGE, pixels)?;
    let exposure = ExposureParams {
        sharpen_amount: 0.0,
        ..Default::default()
    };
    let mut component = crate::pipeline::EffectComponent::new(MaskEffect::Pixelate);
    component.settings.pixelate.amount = 100.0;
    component.settings.pixelate.block_size = 16.0;
    let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
    mask.effect_components.push(component.clone());
    let local = MaskStack {
        masks: vec![mask],
        ..Default::default()
    };
    let global = MaskStack {
        global_effects: vec![component],
        ..Default::default()
    };
    let pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
        &device,
        &queue,
        &source,
        &GpuParams::new(&exposure, &local, &source),
        ProcessingQuality::Preview,
        64,
    )?;
    pipeline.update_mask_layer(&queue, 0, &vec![half::f16::ONE.to_bits(); 64 * 64])?;
    let render = |masks: &MaskStack| -> anyhow::Result<Vec<u8>> {
        pipeline.recompute(&queue, &device, &GpuParams::new(&exposure, masks, &source));
        pipeline.read_output_region_blocking(&device, &queue, 0, 0, EDGE, EDGE)
    };
    let baseline = render(&MaskStack::default())?;
    let local_output = render(&local)?;
    let global_output = render(&global)?;
    assert!(local_output != baseline, "Pixelate did not alter the image");
    assert!(
        local_output == global_output,
        "Local and global effects differ"
    );
    let mut combined = local.clone();
    let mut fog = crate::pipeline::EffectComponent::new(MaskEffect::Fog);
    fog.settings.fog.amount = 80.0;
    fog.settings.fog.density = 80.0;
    combined.masks[0].effect_components.push(fog);
    let stacked_output = render(&combined)?;
    assert!(
        stacked_output != local_output,
        "Second effect did not combine"
    );
    combined.masks[0].adjustments.exposure = 1.0;
    assert!(
        render(&combined)? != stacked_output,
        "Local adjustment did not combine with the effect"
    );
    Ok(())
}

#[test]
fn off_frame_light_rays_match_fullscreen_mask() -> anyhow::Result<()> {
    let Some((device, queue)) = request_test_device() else {
        return Ok(());
    };
    const EDGE: u32 = 32;
    let source =
        LoadedRaw::from_scene_linear_rec2020(EDGE, EDGE, vec![0.05; (EDGE * EDGE * 3) as usize])?;
    let exposure = ExposureParams {
        sharpen_amount: 0.0,
        ..Default::default()
    };
    let mut rays = crate::pipeline::EffectComponent::new(MaskEffect::LightRays);
    rays.settings.light_rays.amount = 100.0;
    rays.settings.light_rays.length = 200.0;
    rays.settings.light_rays.source = [-25.0, 50.0];
    rays.settings.light_rays.variation = 0.0;
    let local = MaskStack {
        masks: vec![{
            let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
            mask.effect_components.push(rays.clone());
            mask
        }],
        ..Default::default()
    };
    let global = MaskStack {
        global_effects: vec![rays],
        ..Default::default()
    };
    let pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
        &device,
        &queue,
        &source,
        &GpuParams::new(&exposure, &local, &source),
        ProcessingQuality::Preview,
        64,
    )?;
    pipeline.update_light_rays_mask_layer(
        &queue,
        0,
        &vec![
            half::f16::ONE.to_bits();
            (super::LIGHT_RAYS_MASK_ATLAS_EDGE * super::LIGHT_RAYS_MASK_ATLAS_EDGE) as usize
        ],
    )?;
    let render = |masks: &MaskStack| -> anyhow::Result<Vec<u8>> {
        pipeline.recompute(&queue, &device, &GpuParams::new(&exposure, masks, &source));
        pipeline.read_output_region_blocking(&device, &queue, 0, 0, EDGE, EDGE)
    };
    let baseline = render(&MaskStack::default())?;
    let masked = render(&local)?;
    let unmasked = render(&global)?;
    assert_ne!(masked, baseline, "Light Rays had no visible effect");
    assert_eq!(
        masked, unmasked,
        "off-frame source changed emission at the image edge"
    );
    Ok(())
}

#[test]
fn neon_amount_approaches_the_unmodified_image_smoothly() -> anyhow::Result<()> {
    let Some((device, queue)) = request_test_device() else {
        return Ok(());
    };
    const EDGE: u32 = 32;
    let pixels = (0..EDGE * EDGE)
        .flat_map(|index| {
            let x = index % EDGE;
            let y = index / EDGE;
            [if (8..24).contains(&x) && (8..24).contains(&y) {
                0.8
            } else {
                0.05
            }; 3]
        })
        .collect();
    let source = LoadedRaw::from_scene_linear_rec2020(EDGE, EDGE, pixels)?;
    let exposure = ExposureParams {
        sharpen_amount: 0.0,
        ..Default::default()
    };
    let mut neon = crate::pipeline::EffectComponent::new(MaskEffect::Neon);
    neon.settings.neon.background = 0.0;
    let mut masks = MaskStack {
        global_effects: vec![neon],
        ..Default::default()
    };
    let pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
        &device,
        &queue,
        &source,
        &GpuParams::new(&exposure, &masks, &source),
        ProcessingQuality::Preview,
        64,
    )?;
    let render = |masks: &MaskStack| -> anyhow::Result<Vec<u8>> {
        pipeline.recompute(&queue, &device, &GpuParams::new(&exposure, masks, &source));
        pipeline.read_output_region_blocking(&device, &queue, 0, 0, EDGE, EDGE)
    };
    let baseline = render(&MaskStack::default())?;
    masks.global_effects[0].settings.neon.amount = 0.01;
    let subtle = render(&masks)?;
    masks.global_effects[0].settings.neon.amount = 100.0;
    let strong = render(&masks)?;
    let maximum_subtle_change = baseline
        .iter()
        .zip(&subtle)
        .map(|(before, after)| before.abs_diff(*after))
        .max()
        .unwrap_or(0);
    assert!(
        maximum_subtle_change <= 2,
        "Neon jumps at a nearly zero Amount"
    );
    assert_ne!(strong, baseline, "Neon had no visible effect");
    Ok(())
}

fn tone_percentile_exposure_follow_from_shader() -> f32 {
    let function = SHADER_TONEMAP
        .split_once("fn tone_percentiles()")
        .expect("tone_percentiles shader function exists")
        .1
        .split_once("\n}\n")
        .expect("tone_percentiles shader function has a body")
        .0;
    let exposure_line = function
        .lines()
        .find(|line| line.contains("adaptive_tone_user_exposure_ev()"))
        .expect("tone_percentiles applies user exposure");
    let suffix = exposure_line
        .split_once("adaptive_tone_user_exposure_ev()")
        .expect("exposure call is present")
        .1
        .trim()
        .trim_end_matches(';')
        .trim();

    if suffix.is_empty() {
        return 1.0;
    }
    suffix
        .strip_prefix('*')
        .expect("tone percentile exposure offset is a direct scalar multiple")
        .trim()
        .parse::<f32>()
        .expect("tone percentile exposure multiplier is numeric")
}

fn test_tone_smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let width = (edge1 - edge0).max(1e-4);
    let x = ((value - edge0) / width).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

#[test]
fn tone_percentile_masks_follow_full_user_exposure() {
    let base_percentiles = [-6.0, -3.0, 0.0, 2.5, 4.5];
    let exposure_ev = 3.0;
    let follow = tone_percentile_exposure_follow_from_shader();
    let exposed_percentiles = base_percentiles.map(|value| value + exposure_ev * follow);

    for (base, exposed) in base_percentiles.into_iter().zip(exposed_percentiles) {
        assert!((exposed - base - exposure_ev).abs() < 1e-6);
    }

    let boundaries = |percentiles: [f32; 5]| {
        let [_, p05, p50, p95, _] = percentiles;
        [
            p05 - 0.90,
            p50 + 1.35,
            p50 - 0.35,
            p95 + 0.45,
            p05 - 0.10,
            p50 + 0.50,
        ]
    };
    for (base, exposed) in boundaries(base_percentiles)
        .into_iter()
        .zip(boundaries(exposed_percentiles))
    {
        assert!((exposed - base - exposure_ev).abs() < 1e-6);
    }

    let masks = |percentiles: [f32; 5], exposure: f32| {
        let [_, p05, p50, p95, _] = percentiles;
        let shadow_ev = -1.50 + exposure;
        let highlight_ev = 1.00 + exposure;
        let white_ev = -1.00 + exposure;
        [
            1.0 - test_tone_smoothstep(p05 - 0.90, p50 + 1.35, shadow_ev),
            0.10 + 0.90 * test_tone_smoothstep(p50 - 0.35, p95 + 0.45, highlight_ev),
            test_tone_smoothstep(p05 - 0.10, p50 + 0.50, white_ev),
        ]
    };
    let base_masks = masks(base_percentiles, 0.0);
    let exposed_masks = masks(exposed_percentiles, exposure_ev);
    for (base, exposed) in base_masks.into_iter().zip(exposed_masks) {
        assert!((exposed - base).abs() < 1e-6);
    }
}

fn tone_consistency_test_tile(core: NativeRect, halo: u32) -> ExportTile {
    // Deliberately preserve the requested crop phase here. Production detail/export
    // preparation aligns to the shared tone grid, but this regression also proves
    // that the shader and guide allocation remain globally anchored if an arbitrary
    // crop origin (including a two-pixel phase shift) reaches the GPU.
    let origin_x = i32::try_from(core.x).unwrap() - i32::try_from(halo).unwrap();
    let origin_y = i32::try_from(core.y).unwrap() - i32::try_from(halo).unwrap();
    ExportTile {
        core_x: core.x,
        core_y: core.y,
        core_width: core.width,
        core_height: core.height,
        local_core_x: halo,
        local_core_y: halo,
        padded_width: core.width + 2 * halo,
        padded_height: core.height + 2 * halo,
        global_origin_x: origin_x,
        global_origin_y: origin_y,
    }
}

fn tone_consistency_scene(width: u32, height: u32) -> LoadedRaw {
    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            let u = (x as f32 + 0.5) / width as f32;
            let v = (y as f32 + 0.5) / height as f32;
            let wave = (u * std::f32::consts::TAU * 5.0).sin() * 0.65
                + (v * std::f32::consts::TAU * 3.0).cos() * 0.45;
            let checker = if ((x / 37) + (y / 29)).is_multiple_of(2) {
                -0.85
            } else {
                0.85
            };
            let ev = -6.5 + 10.5 * u + wave + checker;
            let luma = 0.18 * ev.exp2();
            rgb.extend_from_slice(&[
                luma * (0.82 + 0.28 * v),
                luma * (0.90 + 0.18 * u),
                luma * (0.76 + 0.24 * (1.0 - v)),
            ]);
        }
    }
    LoadedRaw::from_scene_linear_rec2020(width, height, rgb).unwrap()
}

fn opposed_highlight_consistency_raw(width: u32, height: u32) -> LoadedRaw {
    let mut colors = Vec::with_capacity((width * height) as usize);
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let physical = match (x % 2, y % 2) {
                (0, 0) => 0,
                (1, 0) => 1,
                (0, 1) => 3,
                _ => 2,
            };
            colors.push(physical);
            let logical = usize::from(if physical == 3 { 1 } else { physical });
            let mut value = [0.82_f32, 0.57, 0.34][logical];
            if (width / 3..2 * width / 3).contains(&x)
                && (height / 3..2 * height / 3).contains(&y)
                && (logical == 0 || logical == 2)
            {
                value = 1.0;
            }
            pixels.push((value * 10_000.0).round() as u16);
        }
    }
    LoadedRaw {
        width,
        height,
        camera_make: "Test".to_owned(),
        camera_model: "Opposed highlights".to_owned(),
        lens_make: String::new(),
        lens_model: String::new(),
        focal_length: 0.0,
        aperture: 0.0,
        focus_distance: 0.0,
        capture_metadata: Default::default(),
        cfa_kind: CfaKind::Bayer,
        raw_pixels: pixels,
        scene_linear_raster: None,
        color_indices: CompactPixelMap::dense(width, height, colors),
        wb_coeffs: [1.45, 1.0, 0.72, 1.0],
        cam_to_srgb: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ],
        black_levels: [0.0; 4],
        black_levels_per_pixel: CompactPixelMap::repeating(width, height, 1, 1, vec![0.0]),
        white_levels: [10_000.0; 4],
        noise_profile: crate::pipeline::NoiseProfile::default(),
        camera_profile: CameraProfile::default(),
        camera_profile_source: None,
        available_camera_profiles: Vec::new(),
        white_balance_model: None,
        lens_geometry: None,
        ai_denoised: std::sync::Arc::new(std::sync::RwLock::new(None)),
        opposed_chroma_cache: Default::default(),
        opposed_chroma_source_identity: Default::default(),
        opposed_chroma_reference_source: true,
    }
}

#[test]
fn gpu_params_pack_the_same_full_source_opposed_reference_for_moved_tiles() {
    let source = opposed_highlight_consistency_raw(160, 128);
    let masks = MaskStack::default();
    let exposure = ExposureParams {
        highlight_method: HighlightReconstructionMethod::InpaintOpposed,
        ..Default::default()
    };
    source.inpaint_opposed_chroma_for_exposure(&exposure);

    let first = tone_consistency_test_tile(
        NativeRect {
            x: 32,
            y: 24,
            width: 80,
            height: 72,
        },
        16,
    );
    let shifted = tone_consistency_test_tile(
        NativeRect {
            x: 38,
            y: 30,
            width: 80,
            height: 72,
        },
        16,
    );
    let first_raw = extract_padded_tile(&source, first);
    let shifted_raw = extract_padded_tile(&source, shifted);
    let first_params = GpuParams::new_for_tile(
        &exposure,
        &masks,
        &first_raw,
        first.global_origin_x,
        first.global_origin_y,
        source.width,
        source.height,
    );
    let shifted_params = GpuParams::new_for_tile(
        &exposure,
        &masks,
        &shifted_raw,
        shifted.global_origin_x,
        shifted.global_origin_y,
        source.width,
        source.height,
    );

    assert_eq!(first_params.camera.wb, shifted_params.camera.wb);
    assert_eq!(
        first_params.camera.highlight_options,
        shifted_params.camera.highlight_options
    );
    assert!(first_params.camera.highlight_options[0] >= 1.5);
    assert!(first_params.camera.highlight_options[1..]
        .iter()
        .any(|value| value.abs() > 1e-5));
}

pub(super) fn request_test_device_with_info(
) -> Option<(wgpu::Device, wgpu::Queue, wgpu::AdapterInfo)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .or_else(|_| {
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
    })
    .ok()?;
    let info = adapter.get_info();
    eprintln!("TEST ADAPTER {info:?}");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("calibraw tone crop consistency test"),
        ..Default::default()
    }))
    .ok()?;
    Some((device, queue, info))
}

pub(super) fn request_test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    request_test_device_with_info().map(|(device, queue, _)| (device, queue))
}

fn render_tone_consistency_crop(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    source: &LoadedRaw,
    exposure: &ExposureParams,
    masks: &MaskStack,
    full_frame: &RawGpuPipeline,
    core: NativeRect,
) -> anyhow::Result<Vec<f32>> {
    const HALO: u32 = 64;
    let tile = tone_consistency_test_tile(core, HALO);
    let tile_raw = extract_padded_tile(source, tile);
    let params = GpuParams::new_for_tile(
        exposure,
        masks,
        &tile_raw,
        tile.global_origin_x,
        tile.global_origin_y,
        source.width,
        source.height,
    );
    let crop_pipeline = RawGpuPipeline::new_headless_reusing_programs_with_mask_edge(
        device,
        queue,
        &tile_raw,
        &params,
        ProcessingQuality::High,
        full_frame,
        64,
    )?;
    crop_pipeline.dispatch_stage(queue, device, &params, ProcessingStage::Raw);
    crop_pipeline.dispatch_tone_guide_with_inherited_statistics(queue, device, &params, full_frame);
    crop_pipeline.dispatch_stage(queue, device, &params, ProcessingStage::Output);
    crop_pipeline.read_display_linear_region_blocking(
        device,
        queue,
        tile.local_core_x,
        tile.local_core_y,
        tile.core_width,
        tile.core_height,
    )
}

#[test]
fn native_overlapping_tone_crops_match_full_frame_away_from_support_boundaries(
) -> anyhow::Result<()> {
    let Some((device, queue)) = request_test_device() else {
        eprintln!("tone crop GPU regression skipped: no headless wgpu adapter");
        return Ok(());
    };

    let source = tone_consistency_scene(640, 480);
    let masks = MaskStack::default();
    // Texture, clarity and dehaze are zeroed explicitly: the tone guide must be
    // compared without any detail contribution.
    let exposure = ExposureParams {
        highlights: -100.0,
        shadows: 100.0,
        sharpen_amount: 0.0,
        texture: 0.0,
        clarity: 0.0,
        dehaze: 0.0,
        ..Default::default()
    };

    let full_params = GpuParams::new(&exposure, &masks, &source);
    let full_frame = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
        &device,
        &queue,
        &source,
        &full_params,
        ProcessingQuality::High,
        64,
    )?;
    full_frame.recompute(&queue, &device, &full_params);

    let cell = TONE_GUIDE_CELL_SIZE;
    let first = NativeRect {
        x: 160 + cell - 2,
        y: 120 + cell - 2,
        width: 280,
        height: 220,
    };
    let shifted = NativeRect {
        x: first.x + 2,
        y: first.y + 2,
        ..first
    };
    let first_tile = tone_consistency_test_tile(first, 64);
    let shifted_tile = tone_consistency_test_tile(shifted, 64);
    assert_eq!(shifted_tile.global_origin_x - first_tile.global_origin_x, 2);
    assert_eq!(shifted_tile.global_origin_y - first_tile.global_origin_y, 2);
    let first_rgb = render_tone_consistency_crop(
        &device,
        &queue,
        &source,
        &exposure,
        &masks,
        &full_frame,
        first,
    )?;
    let shifted_rgb = render_tone_consistency_crop(
        &device,
        &queue,
        &source,
        &exposure,
        &masks,
        &full_frame,
        shifted,
    )?;

    let overlap_x0 = first.x.max(shifted.x);
    let overlap_y0 = first.y.max(shifted.y);
    let overlap_x1 = first.right().min(shifted.right());
    let overlap_y1 = first.bottom().min(shifted.bottom());
    let boundary_margin = (TONE_GUIDE_CELL_SIZE * 8).max(32);
    let interior_x0 = overlap_x0 + boundary_margin;
    let interior_y0 = overlap_y0 + boundary_margin;
    let interior_x1 = overlap_x1 - boundary_margin;
    let interior_y1 = overlap_y1 - boundary_margin;
    assert!(interior_x1 > interior_x0 && interior_y1 > interior_y0);

    let full_rgb = full_frame.read_display_linear_region_blocking(
        &device,
        &queue,
        interior_x0,
        interior_y0,
        interior_x1 - interior_x0,
        interior_y1 - interior_y0,
    )?;

    let mut crop_max = 0.0_f32;
    let mut full_max = 0.0_f32;
    let mut crop_sum_sq = 0.0_f64;
    let mut full_sum_sq = 0.0_f64;
    let mut samples = 0_u64;
    for y in interior_y0..interior_y1 {
        for x in interior_x0..interior_x1 {
            let first_pixel = ((y - first.y) * first.width + (x - first.x)) as usize * 3;
            let shifted_pixel = ((y - shifted.y) * shifted.width + (x - shifted.x)) as usize * 3;
            let full_pixel =
                ((y - interior_y0) * (interior_x1 - interior_x0) + (x - interior_x0)) as usize * 3;
            for channel in 0..3 {
                let crop_delta =
                    (first_rgb[first_pixel + channel] - shifted_rgb[shifted_pixel + channel]).abs();
                let first_full_delta =
                    (first_rgb[first_pixel + channel] - full_rgb[full_pixel + channel]).abs();
                let shifted_full_delta =
                    (shifted_rgb[shifted_pixel + channel] - full_rgb[full_pixel + channel]).abs();
                crop_max = crop_max.max(crop_delta);
                full_max = full_max.max(first_full_delta.max(shifted_full_delta));
                crop_sum_sq += f64::from(crop_delta) * f64::from(crop_delta);
                full_sum_sq += f64::from(first_full_delta) * f64::from(first_full_delta);
                full_sum_sq += f64::from(shifted_full_delta) * f64::from(shifted_full_delta);
                samples += 1;
            }
        }
    }
    let crop_rms = (crop_sum_sq / samples as f64).sqrt();
    let full_rms = (full_sum_sq / (samples * 2) as f64).sqrt();
    eprintln!(
        "tone crop consistency: interior={}x{}, crop-vs-crop max={crop_max:.8e} rms={crop_rms:.8e}, crop-vs-full max={full_max:.8e} rms={full_rms:.8e}",
        interior_x1 - interior_x0,
        interior_y1 - interior_y0,
    );

    assert!(
        crop_max <= 2.0e-5 && crop_rms <= 2.0e-6,
        "shifted native crops diverged: max={crop_max:e}, rms={crop_rms:e}"
    );
    assert!(
        full_max <= 2.0e-5 && full_rms <= 2.0e-6,
        "native crops diverged from full-frame render: max={full_max:e}, rms={full_rms:e}"
    );
    Ok(())
}

#[test]
fn clipped_colored_highlights_match_across_moved_detail_crops_and_wb() -> anyhow::Result<()> {
    let Some((device, queue)) = request_test_device() else {
        eprintln!("opposed highlight GPU regression skipped: no headless wgpu adapter");
        return Ok(());
    };

    let mut source = opposed_highlight_consistency_raw(640, 480);
    let masks = MaskStack::default();
    // Same reasoning as the tone-guide test: the detail controls stay disabled.
    let exposure = ExposureParams {
        highlight_method: HighlightReconstructionMethod::InpaintOpposed,
        sharpen_amount: 0.0,
        texture: 0.0,
        clarity: 0.0,
        dehaze: 0.0,
        ..Default::default()
    };

    let first = NativeRect {
        x: 150,
        y: 100,
        width: 320,
        height: 280,
    };
    let shifted = NativeRect {
        x: first.x + 8,
        y: first.y + 6,
        ..first
    };
    let overlap_x0 = first.x.max(shifted.x);
    let overlap_y0 = first.y.max(shifted.y);
    let overlap_x1 = first.right().min(shifted.right());
    let overlap_y1 = first.bottom().min(shifted.bottom());

    // Restrict the comparison to the clipped colored-highlight patch while staying
    // comfortably away from tile support boundaries.
    let compare_x0 = overlap_x0.max(source.width / 3 + 16);
    let compare_y0 = overlap_y0.max(source.height / 3 + 16);
    let compare_x1 = overlap_x1.min(2 * source.width / 3 - 16);
    let compare_y1 = overlap_y1.min(2 * source.height / 3 - 16);
    assert!(compare_x1 > compare_x0 && compare_y1 > compare_y0);

    let wb_cases = [[1.20, 1.0, 0.88, 1.0], [1.72, 1.0, 0.58, 1.0]];
    let mut prior_reference = None;
    for wb in wb_cases {
        source.wb_coeffs = wb;
        let reference = source.inpaint_opposed_chroma_for_exposure(&exposure);
        if let Some(previous) = prior_reference {
            assert_ne!(
                reference, previous,
                "WB change reused the previous chroma reference"
            );
        }
        prior_reference = Some(reference);

        let full_params = GpuParams::new(&exposure, &masks, &source);
        assert_eq!(full_params.camera.wb, wb);
        assert_eq!(
            &full_params.camera.highlight_options[1..],
            reference.as_slice()
        );
        let full_frame = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
            &device,
            &queue,
            &source,
            &full_params,
            ProcessingQuality::High,
            64,
        )?;
        full_frame.recompute(&queue, &device, &full_params);

        let first_rgb = render_tone_consistency_crop(
            &device,
            &queue,
            &source,
            &exposure,
            &masks,
            &full_frame,
            first,
        )?;
        let shifted_rgb = render_tone_consistency_crop(
            &device,
            &queue,
            &source,
            &exposure,
            &masks,
            &full_frame,
            shifted,
        )?;
        let full_rgb = full_frame.read_display_linear_region_blocking(
            &device,
            &queue,
            compare_x0,
            compare_y0,
            compare_x1 - compare_x0,
            compare_y1 - compare_y0,
        )?;

        let mut crop_max = 0.0_f32;
        let mut full_max = 0.0_f32;
        let mut crop_sum_sq = 0.0_f64;
        let mut full_sum_sq = 0.0_f64;
        let mut samples = 0_u64;
        for y in compare_y0..compare_y1 {
            for x in compare_x0..compare_x1 {
                let first_pixel = ((y - first.y) * first.width + (x - first.x)) as usize * 3;
                let shifted_pixel =
                    ((y - shifted.y) * shifted.width + (x - shifted.x)) as usize * 3;
                let full_pixel =
                    ((y - compare_y0) * (compare_x1 - compare_x0) + (x - compare_x0)) as usize * 3;
                for channel in 0..3 {
                    let crop_delta = (first_rgb[first_pixel + channel]
                        - shifted_rgb[shifted_pixel + channel])
                        .abs();
                    let first_full_delta =
                        (first_rgb[first_pixel + channel] - full_rgb[full_pixel + channel]).abs();
                    let shifted_full_delta = (shifted_rgb[shifted_pixel + channel]
                        - full_rgb[full_pixel + channel])
                        .abs();
                    crop_max = crop_max.max(crop_delta);
                    full_max = full_max.max(first_full_delta.max(shifted_full_delta));
                    crop_sum_sq += f64::from(crop_delta) * f64::from(crop_delta);
                    full_sum_sq += f64::from(first_full_delta) * f64::from(first_full_delta);
                    full_sum_sq += f64::from(shifted_full_delta) * f64::from(shifted_full_delta);
                    samples += 1;
                }
            }
        }
        let crop_rms = (crop_sum_sq / samples as f64).sqrt();
        let full_rms = (full_sum_sq / (samples * 2) as f64).sqrt();
        eprintln!(
            "opposed highlight crop consistency wb={wb:?}: crop-vs-crop max={crop_max:.8e} rms={crop_rms:.8e}, crop-vs-full max={full_max:.8e} rms={full_rms:.8e}"
        );
        assert!(
            crop_max <= 2.0e-5 && crop_rms <= 2.0e-6,
            "moved highlight crops diverged for wb={wb:?}: max={crop_max:e}, rms={crop_rms:e}"
        );
        assert!(
            full_max <= 2.0e-5 && full_rms <= 2.0e-6,
            "highlight crops diverged from full frame for wb={wb:?}: max={full_max:e}, rms={full_rms:e}"
        );
    }

    assert_eq!(
        source.opposed_chroma_cache.read().unwrap().len(),
        wb_cases.len()
    );
    Ok(())
}

#[test]
fn inactive_programs_stay_deferred_across_template_reuse_and_activate_on_edit() -> anyhow::Result<()>
{
    let Some((device, queue)) = request_test_device() else {
        eprintln!("GPU program reuse regression skipped: no headless wgpu adapter");
        return Ok(());
    };
    let raw = opposed_highlight_consistency_raw(48, 48);
    let masks = MaskStack::default();
    let mut exposure = ExposureParams::scene_referred_default();
    let params = GpuParams::new(&exposure, &masks, &raw);
    let pipeline = RawGpuPipeline::new_headless_with_quality(
        &device,
        &queue,
        &raw,
        &params,
        ProcessingQuality::Preview,
    )?;
    let creative = pipeline.adjustment_creative_pass_index;
    assert!(pipeline.passes[creative].pipeline.compiled.get().is_none());
    let template = pipeline.program_template();
    let reused = RawGpuPipeline::new_headless_reusing_program_template(
        &device,
        &queue,
        &raw,
        &params,
        ProcessingQuality::Preview,
        &template,
    )?;
    assert!(reused.passes[creative].pipeline.compiled.get().is_none());
    assert!(std::sync::Arc::ptr_eq(
        &pipeline.passes[creative].pipeline,
        &reused.passes[creative].pipeline
    ));
    reused.recompute(&queue, &device, &params);
    let neutral =
        reused.read_output_region_blocking(&device, &queue, 0, 0, raw.width, raw.height)?;
    exposure.saturation = 35.0;
    let edited = GpuParams::new(&exposure, &masks, &raw);
    reused.recompute(&queue, &device, &edited);
    let colored =
        reused.read_output_region_blocking(&device, &queue, 0, 0, raw.width, raw.height)?;
    assert!(pipeline.passes[creative].pipeline.compiled.get().is_some());
    assert_ne!(neutral, colored);
    // Eagerly compiling all remaining programs must not alter the rendered result.
    for pass in &reused.passes {
        pass.pipeline.get();
    }
    reused.recompute(&queue, &device, &edited);
    assert_eq!(
        colored,
        reused.read_output_region_blocking(&device, &queue, 0, 0, raw.width, raw.height)?
    );
    Ok(())
}

#[test]
fn specialized_bayer_modes_match_the_dynamic_shader_when_switching_modes() -> anyhow::Result<()> {
    use super::ComputeProgram;
    use crate::pipeline::DemosaicMode;
    use std::sync::{Arc, OnceLock};
    let Some((device, queue)) = request_test_device() else {
        eprintln!("Bayer specialization regression skipped: no headless wgpu adapter");
        return Ok(());
    };
    let raw = opposed_highlight_consistency_raw(48, 48);
    let masks = MaskStack::default();
    let mut exposure = ExposureParams::scene_referred_default();
    exposure.luminance_denoise = 15.0;
    exposure.ca_red = 0.5;
    let params = GpuParams::new(&exposure, &masks, &raw);
    let mut pipeline = RawGpuPipeline::new_headless_with_quality(
        &device,
        &queue,
        &raw,
        &params,
        ProcessingQuality::High,
    )?;
    let finish = pipeline.demosaic_finish_index;
    let specialized = Arc::clone(&pipeline.passes[finish].pipeline);
    let dynamic = specialized.compile(&[]);
    let reference = Arc::new(ComputeProgram {
        device: device.clone(),
        shader: specialized.shader.clone(),
        layouts: specialized.layouts.clone(),
        entry: specialized.entry.clone(),
        cache: None,
        compiled: OnceLock::from(dynamic.clone()),
        demosaic_variants: std::array::from_fn(|_| OnceLock::from(dynamic.clone())),
    });
    for (denoise, ca) in [(0.0, 0.0), (15.0, 0.0), (0.0, 0.5), (15.0, 0.5)] {
        exposure.luminance_denoise = denoise;
        exposure.ca_red = ca;
        for mode in [
            DemosaicMode::Reference,
            DemosaicMode::FrequencyDomainChroma,
            DemosaicMode::Dual,
            DemosaicMode::Reference,
        ] {
            exposure.demosaic_mode = mode;
            let params = GpuParams::new(&exposure, &masks, &raw);
            pipeline.passes[finish].pipeline = Arc::clone(&reference);
            pipeline.recompute(&queue, &device, &params);
            let expected = pipeline.read_display_linear_region_blocking(
                &device, &queue, 0, 0, raw.width, raw.height,
            )?;
            pipeline.passes[finish].pipeline = Arc::clone(&specialized);
            pipeline.recompute(&queue, &device, &params);
            let actual = pipeline.read_display_linear_region_blocking(
                &device, &queue, 0, 0, raw.width, raw.height,
            )?;
            let max_error = actual
                .iter()
                .zip(&expected)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(max_error <= 1e-5, "mode={mode:?} error={max_error}");
        }
    }
    Ok(())
}
