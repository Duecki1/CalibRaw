use super::*;

fn contains(outer: [u32; 4], inner: [u32; 4]) -> bool {
    (0..2).all(|axis| {
        outer[axis] <= inner[axis] && outer[axis] + outer[axis + 2] >= inner[axis] + inner[axis + 2]
    })
}

#[test]
fn mask_slider_sweeps_reuse_the_crop_and_keep_enough_feather_margin() {
    let mut masks = MaskStack::default();
    masks.add_mask(MaskKind::Path);
    for full_size in [[6000, 4000], [4000, 6000]] {
        let origin = [1000, 700];
        let size = [2000, 1800];
        let cached = detail_mask_update_region(&masks, origin, size, full_size, None, true);
        for grow in [-1.0, -0.4, 0.0, 0.4, 1.0] {
            for feather in [0.0, 0.25, 0.75, 1.0] {
                if let MaskGeometry::Path {
                    grow: g,
                    feather: f,
                    ..
                } = &mut masks.masks[0].components[0].geometry
                {
                    *g = grow;
                    *f = feather;
                }
                let required =
                    detail_mask_source_region(&masks, origin, size, full_size[0], full_size[1]);
                let next =
                    detail_mask_update_region(&masks, origin, size, full_size, Some(cached), true);
                assert_eq!(next, cached, "grow={grow}, feather={feather}");
                assert!(contains(next, required));
            }
        }
        // Release refines the same source pixels, keeping the distance contour stable.
        assert_eq!(
            detail_mask_update_region(&masks, origin, size, full_size, Some(cached), false),
            cached
        );
        let drag = detail_mask_texture_extent(cached, 2048, true);
        let settled = detail_mask_texture_extent(cached, 2048, false);
        assert!(drag[0].max(drag[1]) < settled[0].max(settled[1]));
        assert_eq!(settled, mask_region_texture_extent(cached, 2048));
        assert!(
            (drag[0] as f64 / drag[1] as f64 - cached[2] as f64 / cached[3] as f64).abs() < 0.004
        );
    }
}

#[test]
fn mask_crop_reuse_handles_navigation_image_edges_and_large_imported_widths() {
    let mut masks = MaskStack::default();
    masks.add_mask(MaskKind::Path);
    let full = [6000, 4000];
    let cached = detail_mask_update_region(&masks, [0, 0], [1000, 1000], full, None, true);
    assert_eq!(&cached[..2], &[0, 0]);
    let next =
        detail_mask_update_region(&masks, [5200, 3300], [800, 700], full, Some(cached), true);
    assert_eq!(next[0] + next[2], full[0]);
    assert_eq!(next[1] + next[3], full[1]);
    assert!(!contains(cached, next));
    if let MaskGeometry::Path { grow, feather, .. } = &mut masks.masks[0].components[0].geometry {
        *grow = 3.0;
        *feather = 2.0;
    }
    let required = detail_mask_source_region(&masks, [1000, 700], [2000, 1800], full[0], full[1]);
    let updated =
        detail_mask_update_region(&masks, [1000, 700], [2000, 1800], full, Some(cached), true);
    assert!(contains(updated, required));
    assert_eq!(
        detail_mask_texture_extent([0, 0, 16, 12], 64, true),
        [16, 12]
    );
}

#[cfg(not(target_os = "android"))]
#[test]
fn zoom_drag_mapping_refines_all_layers_and_release_keeps_the_last_edit() {
    let instance = wgpu::Instance::default();
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        force_fallback_adapter: true,
        ..Default::default()
    })) else {
        eprintln!("zoom mask interaction test skipped: no headless GPU adapter");
        return;
    };
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    let raw =
        Arc::new(LoadedRaw::from_scene_linear_rec2020(256, 192, vec![0.2; 256 * 192 * 3]).unwrap());
    let full_raw = Arc::new(
        LoadedRaw::from_scene_linear_rec2020(1200, 900, vec![0.2; 1200 * 900 * 3]).unwrap(),
    );
    let exposure = ExposureParams::default();
    let mut masks = MaskStack {
        scene_depth: MaskImage::new(2, 2, vec![0, 85, 170, 255]),
        ..Default::default()
    };
    for layer in 0..2 {
        masks.add_mask(MaskKind::Subject);
        masks.masks[layer].adjustments.exposure = if layer == 0 { 0.8 } else { -0.5 };
        let mut pixels = vec![0; 128 * 96];
        for y in 24..72 {
            let left = if layer == 0 { 24 } else { 64 };
            pixels[y * 128 + left..y * 128 + left + 32].fill(255);
        }
        if let MaskGeometry::Ai {
            mask,
            grow,
            feather,
        } = &mut masks.masks[layer].components[0].geometry
        {
            *mask = MaskImage::new(128, 96, pixels);
            *grow = 0.4;
            *feather = 0.6;
        }
    }
    let params = GpuParams::new(&exposure, &masks, &raw);
    let pipeline = RawGpuPipeline::new(
        &device,
        &queue,
        &raw,
        &params,
        PipelineOptions::new(ProcessingQuality::Preview).mask_atlas_edge(1024),
    )
    .unwrap();
    let region = [0, 0, full_raw.width, full_raw.height];
    let full_extent = detail_mask_texture_extent(region, pipeline.mask_atlas_edge(), false);
    let drag_extent = detail_mask_texture_extent(region, pipeline.mask_atlas_edge(), true);
    assert_ne!(full_extent, drag_extent);
    let mut reference: Option<Vec<u8>> = None;
    for extent in [full_extent, drag_extent, full_extent] {
        CalibRawApp::upload_detail_masks(
            &pipeline, &queue, &masks, &full_raw, region, extent, None,
        )
        .unwrap();
        pipeline.recompute(
            &queue,
            &device,
            &GpuParams::new(&exposure, &masks, &raw).with_mask_uv_rect_and_extent(
                mask_source_region_uv(region, full_raw.width, full_raw.height),
                extent,
            ),
        );
        let pixels = pipeline
            .read_output_region_blocking(&device, &queue, 0, 0, raw.width, raw.height)
            .unwrap();
        if let Some(reference) = &reference {
            let difference = reference
                .iter()
                .zip(&pixels)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert!(
                difference <= 4,
                "changing mask density changed displayed coverage by {difference}"
            );
        } else {
            reference = Some(pixels);
        }
    }
    CalibRawApp::upload_detail_masks(
        &pipeline,
        &queue,
        &masks,
        &full_raw,
        region,
        drag_extent,
        None,
    )
    .unwrap();

    let context = egui::Context::default();
    let mut app = CalibRawApp::empty(&context);
    app.masks.stack = masks;
    app.develop.loaded_raw = Some(Arc::clone(&full_raw));
    app.develop.preview_raw = Some(Arc::clone(&raw));
    let mut renderer = eframe::egui_wgpu::Renderer::new(
        &device,
        wgpu::TextureFormat::Rgba8Unorm,
        eframe::egui_wgpu::RendererOptions::default(),
    );
    let fitted = RawGpuPipeline::new(
        &device,
        &queue,
        &raw,
        &params,
        PipelineOptions::new(ProcessingQuality::Preview)
            .mask_atlas_edge(64)
            .programs(&pipeline.program_template()),
    )
    .unwrap();
    app.preview.gpu_pipeline = Some(PreviewPipeline::register(
        fitted,
        &device,
        &mut renderer,
        &app.preview.retired_textures,
    ));
    let pipeline = PreviewPipeline::register(
        pipeline,
        &device,
        &mut renderer,
        &app.preview.retired_textures,
    );
    app.preview.zoom = 2.0;
    app.preview.viewport_pixels = [64, 48];
    app.preview.visible_uv = PreviewUvRect {
        min: [0.0; 2],
        max: [1.0; 2],
    };
    app.preview.detail = Some(PreviewDetail {
        pipeline,
        uv_rect: app.preview.visible_uv,
        texture_uv_rect: app.preview.visible_uv,
        revision: app.preview.revision,
        raw,
        source_origin: [0, 0],
        source_size: [1200, 900],
        full_source_size: [1200, 900],
        mask_source_region: region,
        mask_texture_extent: drag_extent,
        virtual_origin: [0, 0],
        virtual_full_size: [256, 192],
        processing_halo: app.preview_detail_halo(),
    });
    let pointer_event = |pressed| egui::Event::PointerButton {
        pos: egui::pos2(20.0, 20.0),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let _ = context.run_ui(
        egui::RawInput {
            events: vec![pointer_event(true)],
            ..Default::default()
        },
        |_| {},
    );
    app.masks.interaction_dirty_layer = Some(0);
    app.preview.pending_stage = Some(ProcessingStage::Output);
    app.preview.navigation_pending_stage = Some(ProcessingStage::Output);
    assert!(app.defer_background_mask_processing());
    app.preview.navigation_pending_stage = Some(ProcessingStage::Tone);
    assert!(!app.defer_background_mask_processing());
    app.preview.navigation_pending_stage = Some(ProcessingStage::Output);
    app.preview.white_balance_refresh_pending = true;
    assert!(!app.defer_background_mask_processing());
    app.preview.white_balance_refresh_pending = false;
    app.preview.detail.as_mut().unwrap().revision = app.preview.revision.wrapping_sub(1);
    assert!(!app.defer_background_mask_processing());
    app.preview.detail.as_mut().unwrap().revision = app.preview.revision;

    // A held pointer also commits the latest value without another movement,
    // using the faster interval only when the sharp crop covers the view.
    app.masks.interaction_has_uncommitted_change = true;
    app.masks.interaction_last_upload = Some(Instant::now() - Duration::from_millis(20));
    app.flush_mask_geometry_interaction();
    assert!(!app.masks.interaction_has_uncommitted_change);
    assert!(app.masks.detail_dirty_layers[0]);
    app.masks.dirty_layers.fill(false);
    app.masks.detail_dirty_layers.fill(false);
    app.preview.detail_pending_stage = None;
    let _ = context.run_ui(
        egui::RawInput {
            events: vec![pointer_event(false)],
            ..Default::default()
        },
        |_| {},
    );
    assert!(!app.defer_background_mask_processing());
    app.finish_mask_geometry_interaction();
    assert!(
        app.masks.detail_dirty_layers[0],
        "release must refine even if the last value was already committed"
    );
    assert_eq!(
        app.preview.detail_pending_stage,
        Some(ProcessingStage::Output)
    );
    assert!(app.masks.dirty_layers[0]);
    assert_eq!(app.masks.interaction_dirty_layer, None);
}
