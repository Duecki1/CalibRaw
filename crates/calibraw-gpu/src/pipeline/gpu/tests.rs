use super::{
    processing_work_format, shader_manager::ShaderManager, shaders, work_shader_source, GpuParams,
    PipelineOptions, ProcessingQuality, RawGpuPipeline, COLOR_DENOISE_ENTRY_POINTS,
    DETAIL_PYRAMID_LEVELS,
};
use crate::pipeline::{
    build_proxy, CameraProfile, CfaKind, CompactPixelMap, ExposureParams, LoadedRaw, MaskStack,
    ProxySpec, SensorSaturation,
};

/// Composes `shader` the way production does for `quality` and validates it.
fn validate_shader(shader: shaders::EntryShader, quality: ProcessingQuality) {
    let format = processing_work_format(quality);
    let mut manager = ShaderManager::new(format, shader.sensor.unwrap_or(CfaKind::Bayer)).unwrap();
    let module_text = shader.source.module_text();
    let text = if shader.work_format {
        work_shader_source(&module_text, format).unwrap()
    } else {
        std::borrow::Cow::Borrowed(module_text.as_ref())
    };
    let module = manager
        .compose_naga_module(text.as_ref(), shader.source.file_name)
        .unwrap_or_else(|error| panic!("{} did not compose: {error:#}", shader.label));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("{} did not validate: {error}", shader.label));
}

#[test]
fn compute_shaders_validate() {
    for shader in shaders::ALL_ENTRY_SHADERS {
        validate_shader(shader, ProcessingQuality::Preview);
    }
}

#[test]
fn high_quality_shaders_validate() {
    for shader in shaders::ALL_ENTRY_SHADERS {
        validate_shader(shader, ProcessingQuality::High);
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
        source_extent: [width, height],
        camera_make: "Test".to_owned(),
        camera_model: "Opposed highlights".to_owned(),
        lens_make: String::new(),
        lens_model: String::new(),
        focal_length: 0.0,
        aperture: 0.0,
        focus_distance: 0.0,
        capture_metadata: Default::default(),
        cfa_kind: CfaKind::Bayer,
        raw_pixels: pixels.into(),
        binned_sensor: None,
        scene_linear_raster: None,
        scene_referred_raster: false,
        color_indices: CompactPixelMap::dense(width, height, colors),
        wb_coeffs: [1.45, 1.0, 0.72, 1.0],
        profile_color: std::sync::Arc::new(crate::pipeline::ProfileColor {
            cam_to_srgb: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
            camera_profile: CameraProfile::default(),
            camera_profile_source: None,
            white_balance_model: None,
        }),
        color_source: None,
        black_levels: [0.0; 4],
        black_levels_per_pixel: CompactPixelMap::repeating(width, height, 1, 1, vec![0.0]),
        white_levels: [10_000.0; 4],
        sensor_saturation: SensorSaturation::AT_WHITE,
        noise_profile: crate::pipeline::NoiseProfile::default(),
        available_camera_profiles: Vec::new(),
        lens_geometry: None,
        lens_shading: None,
        lateral_chromatic_aberration: crate::pipeline::LateralChromaticAberration::NONE,
        ai_denoised: std::sync::Arc::new(std::sync::RwLock::new(None)),
        opposed_chroma_cache: Default::default(),
        opposed_chroma_source_identity: Default::default(),
        opposed_chroma_reference_source: true,
    }
}

/// Every named pass position holds the program it is named after; layouts
/// reused from program templates are taken from these positions.
fn assert_planned_programs(pipeline: &RawGpuPipeline) {
    let indices = &pipeline.indices;
    let entry = |index: usize| pipeline.passes[index].pipeline.entry.as_str();
    for (index, expected) in [
        (indices.highlight_pass_index, "highlight_reconstruct"),
        (indices.demosaic_dual_start_index + 1, "dual_smooth_detail"),
        (
            indices.color_denoise_start_index,
            COLOR_DENOISE_ENTRY_POINTS[0],
        ),
        (indices.tone_prepare_pass_index, "tone_guide_prepare"),
        (indices.tone_blur_pass_index, "tone_guide_horizontal"),
        (indices.tone_reduce_pass_index, "tone_reduce_histogram"),
        (indices.adjustment_prepare_pass_index, "prepare_scene_node"),
        (indices.adjustment_tone_pass_index, "apply_scene_tone_node"),
        (
            indices.adjustment_local_tone_pass_index,
            "apply_local_scene_tone_node",
        ),
        (indices.detail_pyramid_start_index, "build_detail_level_0"),
        (
            indices.detail_pyramid_start_index + DETAIL_PYRAMID_LEVELS - 1,
            "build_detail_level_6",
        ),
        (
            indices.adjustment_effects_pass_index,
            "apply_scene_effects_node",
        ),
        (
            indices.adjustment_effects_copy_pass_index,
            "copy_scene_effects_node",
        ),
        (indices.mask_blur_start_index, "diffuse_mask_blur_0"),
        (indices.glow_prepare_pass_index, "prepare_glow_source"),
        (indices.glow_blur_start_index, "diffuse_glow_0"),
        (
            indices.pixelate_blocks_pass_index,
            "prepare_pixelate_blocks",
        ),
        (
            indices.adjustment_creative_pass_index,
            "apply_creative_effects",
        ),
        (indices.adjustment_render_pass_index, "apply_view_node"),
        (
            indices.image_light_accumulate_pass_index,
            "accumulate_image_lights",
        ),
        (
            indices.image_light_resolve_pass_index,
            "resolve_image_lights",
        ),
        (
            indices.image_light_blur_pass_index,
            "blur_image_lights_horizontal",
        ),
        (
            indices.relight_shadow_map_pass_index,
            "build_relight_shadow_map",
        ),
        (
            indices.scene_depth_guide_pass_index,
            "build_scene_depth_guide",
        ),
        (indices.raster_finish_pass_index, "raster_finish"),
        (indices.haze_accumulate_pass_index, "accumulate_haze"),
        (indices.haze_resolve_pass_index, "resolve_haze_airlight"),
        (indices.haze_end_index - 1, "smooth_haze_coefficients"),
        (
            indices.binned_highlight_pass_index,
            "binned_highlight_reconstruct",
        ),
    ] {
        assert_eq!(entry(index), expected, "pass {index}");
    }
    assert_eq!(pipeline.passes.len(), indices.pass_count);
}

/// Builds and runs the whole graph for each sensor and pixel layout and
/// quality, so wgpu checks every entry shader against the Rust bind group
/// layouts it is given. Naga validation alone checks each shader in
/// isolation.
#[test]
fn pipelines_build_and_render_for_every_sensor_layout_and_quality() -> anyhow::Result<()> {
    const EDGE: u32 = 96;
    // Fujifilm X-Trans: R = 0, G = 1, B = 2.
    const XTRANS: [[u8; 6]; 6] = [
        [1, 1, 0, 1, 1, 2],
        [1, 1, 2, 1, 1, 0],
        [2, 0, 1, 0, 2, 1],
        [1, 1, 2, 1, 1, 0],
        [1, 1, 0, 1, 1, 2],
        [0, 2, 1, 2, 0, 1],
    ];
    let Some((device, queue, _)) = request_test_device_with_info() else {
        eprintln!("Pipeline construction check skipped: no headless wgpu adapter");
        return Ok(());
    };
    let bayer = opposed_highlight_consistency_raw(EDGE, EDGE);
    let mut xtrans = opposed_highlight_consistency_raw(EDGE, EDGE);
    xtrans.cfa_kind = CfaKind::XTrans;
    xtrans.color_indices = CompactPixelMap::dense(
        EDGE,
        EDGE,
        (0..EDGE * EDGE)
            .map(|index| XTRANS[((index / EDGE) % 6) as usize][((index % EDGE) % 6) as usize])
            .collect(),
    );
    let raster = tone_consistency_scene(EDGE, EDGE);
    let binned = ProxySpec { max_edge: EDGE / 2 };
    let binned_bayer = build_proxy(&bayer, binned);
    let binned_xtrans = build_proxy(&xtrans, binned);
    for raw in [&bayer, &xtrans, &raster, &binned_bayer, &binned_xtrans] {
        let source = (raw.cfa_kind, raw.pixel_layout());
        let params = GpuParams::new(&ExposureParams::default(), &MaskStack::default(), raw);
        for quality in [ProcessingQuality::Preview, ProcessingQuality::High] {
            let pipeline =
                RawGpuPipeline::new(&device, &queue, raw, &params, PipelineOptions::new(quality))?;
            assert_planned_programs(&pipeline);
            pipeline.recompute(&queue, &device, &params);
            let rgba = pipeline
                .read_output_region_blocking(&device, &queue, 0, 0, raw.width, raw.height)?;
            assert_eq!(
                rgba.len(),
                (raw.width * raw.height * 4) as usize,
                "{source:?} {quality:?}"
            );
            assert!(
                rgba.chunks_exact(4).any(|pixel| pixel[..3] != [0, 0, 0]),
                "{source:?} {quality:?} rendered black"
            );
        }
    }
    Ok(())
}

pub(super) fn request_test_device_with_info(
) -> Option<(wgpu::Device, wgpu::Queue, wgpu::AdapterInfo)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: crate::processing_backends(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
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

/// X-Trans demosaicing matches darktable 5.6.1's Markesteijn 3-pass followed
/// by one colour-smoothing pass. The expected values come from a float64
/// transcription of `xtrans_markesteijn_interpolate` and `color_smoothing` on
/// the same mosaic, at two pixels of every photosite class whose interpolation
/// and median decisions are far from ties (float32 and float64 agree to 1e-6
/// over each pixel's 3x3 window). Dual demosaicing with a zero threshold keeps
/// the smoothed Markesteijn result unchanged.
#[test]
fn xtrans_demosaic_matches_darktable_markesteijn() -> anyhow::Result<()> {
    use crate::pipeline::DemosaicMode;
    const EDGE: u32 = 64;
    const XTRANS: [[u8; 6]; 6] = [
        [1, 1, 0, 1, 1, 2],
        [1, 1, 2, 1, 1, 0],
        [2, 0, 1, 0, 2, 1],
        [1, 1, 2, 1, 1, 0],
        [1, 1, 0, 1, 1, 2],
        [0, 2, 1, 2, 0, 1],
    ];
    // Camera RGB at [x, y] for a pattern shifted by one row and two columns.
    const EXPECTED: [([u32; 2], [f32; 3]); 12] = [
        // Greens in 2x2 blocks.
        ([25, 24], [0.422616, 0.503000, 0.558852]),
        ([26, 24], [0.485211, 0.568800, 0.606648]),
        // Blue in horizontal and vertical pairs.
        ([26, 25], [0.630919, 0.720059, 0.747638]),
        ([28, 25], [0.371757, 0.438279, 0.458000]),
        ([24, 24], [0.362937, 0.423748, 0.479600]),
        ([30, 24], [0.478411, 0.543242, 0.540026]),
        // Red in horizontal and vertical pairs.
        ([25, 25], [0.572883, 0.665245, 0.688345]),
        ([29, 25], [0.499989, 0.564743, 0.570395]),
        ([27, 24], [0.602551, 0.686140, 0.722831]),
        ([33, 24], [0.690196, 0.707821, 0.727531]),
        // Solitary greens.
        ([24, 25], [0.456051, 0.547200, 0.573974]),
        ([27, 25], [0.280478, 0.347000, 0.370100]),
    ];
    let Some((device, queue)) = request_test_device() else {
        eprintln!("X-Trans Markesteijn check skipped: no headless wgpu adapter");
        return Ok(());
    };
    let color = |x: u32, y: u32| XTRANS[(y as usize + 1) % 6][(x as usize + 2) % 6];
    let mut raw = opposed_highlight_consistency_raw(EDGE, EDGE);
    raw.cfa_kind = CfaKind::XTrans;
    raw.wb_coeffs = [1.0; 4];
    raw.color_indices = CompactPixelMap::dense(
        EDGE,
        EDGE,
        (0..EDGE * EDGE)
            .map(|index| color(index % EDGE, index / EDGE))
            .collect(),
    );
    // Integer texture that the reference reproduces exactly; white is 10000.
    for y in 0..EDGE {
        for x in 0..EDGE {
            let value = 3000
                + ((x * x * 3 + y * y * 5 + x * y) % 997) * 4
                + (x * 13 % 7) * 50
                + 400 * u32::from(color(x, y));
            raw.raw_pixels.make_mut()[(y * EDGE + x) as usize] = value as u16;
        }
    }
    let exposure = ExposureParams {
        demosaic_mode: DemosaicMode::Dual,
        dual_threshold: 0.0,
        luminance_denoise: 0.0,
        chroma_denoise: 0.0,
        ..Default::default()
    };
    let params = GpuParams::new(&exposure, &MaskStack::default(), &raw);
    let pipeline = RawGpuPipeline::new(
        &device,
        &queue,
        &raw,
        &params,
        PipelineOptions::new(ProcessingQuality::High),
    )?;
    let scene = pipeline.render_camera_scene_blocking(&device, &queue, &params)?;
    for ([x, y], expected) in EXPECTED {
        let index = ((y * EDGE + x) * 3) as usize;
        let actual = &scene[index..index + 3];
        assert!(
            actual
                .iter()
                .zip(expected)
                .all(|(actual, expected)| (actual - expected).abs() < 1e-5),
            "pixel ({x}, {y}) of colour {}: {actual:?}, darktable {expected:?}",
            color(x, y)
        );
    }
    Ok(())
}
