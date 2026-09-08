use super::*;
use eframe::egui_wgpu::{Renderer, RendererOptions, ScreenDescriptor};

/// Run explicitly with an isolated XDG_CONFIG_HOME. This exercises the actual
/// GPU image and egui layout without a window or an Android device.
#[test]
#[ignore = "requires a GPU adapter; optional CALIBRAW_PREVIEW_TEST_SCREENSHOT writes a visual fixture"]
fn portrait_gpu_layout_and_input() {
    let mut instance_descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    instance_descriptor.memory_budget_thresholds = crate::memory_budget_thresholds();
    let instance = wgpu::Instance::new(instance_descriptor);
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        force_fallback_adapter: true,
        ..Default::default()
    }))
    .unwrap();
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    let context = egui::Context::default();
    crate::ui::theme::install(&context);
    let mut app = CalibRawApp::empty(&context);
    app.ui.active_tab = AppTab::Develop;
    let raw = Arc::new(
        LoadedRaw::from_scene_linear_rec2020(
            600,
            900,
            (0..600 * 900)
                .flat_map(|pixel| {
                    let x = pixel % 600;
                    let y = pixel / 600;
                    let value = if (x / 30 + y / 30) % 2 == 0 {
                        0.12
                    } else {
                        0.55
                    };
                    [value, x as f32 / 600.0, y as f32 / 900.0]
                })
                .collect(),
        )
        .unwrap(),
    );
    let params = GpuParams::new(&app.develop.exposure, &app.masks.stack, &raw);
    let mut pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
        &device,
        &queue,
        &raw,
        &params,
        ProcessingQuality::Preview,
        256,
    )
    .unwrap();
    pipeline.recompute(&queue, &device, &params);
    let mut renderer = Renderer::new(
        &device,
        wgpu::TextureFormat::Rgba8Unorm,
        RendererOptions::default(),
    );
    pipeline.register_egui_texture(&device, &mut renderer);
    let image_id = pipeline.egui_texture_id.unwrap();
    app.preview.gpu_pipeline = Some(pipeline);
    app.develop.loaded_raw = Some(Arc::clone(&raw));
    app.develop.preview_raw = Some(raw);
    let frame = eframe::Frame::_new_kittest();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 800.0));
    let mut output = None;
    for height in [144.0_f32, 460.0, 272.0] {
        context
            .data_mut(|data| data.insert_temp(egui::Id::new("portrait-tool-sheet-height"), height));
        // Areas need a sizing pass before their widgets can receive input.
        for _ in 0..2 {
            let result = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| {
                            crate::ui::develop_viewport::show(ui, &mut app, &frame);
                        });
                },
            );
            for (id, delta) in &result.textures_delta.set {
                renderer.update_texture(&device, &queue, *id, delta);
            }
            output = Some(result);
        }
        let jobs = context.tessellate(
            output.as_ref().unwrap().shapes.clone(),
            context.pixels_per_point(),
        );
        let image = jobs
            .iter()
            .find_map(|job| match &job.primitive {
                egui::epaint::Primitive::Mesh(mesh) if mesh.texture_id == image_id => Some(mesh),
                _ => None,
            })
            .expect("the full-width GPU image must be painted");
        let bounds = image.calc_bounds();
        assert!(
            (bounds.width() - screen.width()).abs() < 0.01,
            "image width changed with tool sheet: {bounds:?}"
        );
        assert_eq!(app.preview.zoom, 1.0);
        let tool_point = egui::pos2(200.0, screen.bottom() - height + 12.0);
        let image_point = egui::pos2(200.0, screen.bottom() - height - 12.0);
        assert_ne!(
            context.layer_id_at(tool_point),
            context.layer_id_at(image_point)
        );
    }

    // Starting a crop gesture over the sheet must never change image geometry.
    app.ui.sidebar_tab = SidebarTab::Crop;
    let original_crop = app.develop.geometry;
    let _ = context.run_ui(
        egui::RawInput {
            screen_rect: Some(screen),
            events: vec![
                egui::Event::PointerMoved(egui::pos2(200.0, 790.0)),
                egui::Event::PointerButton {
                    pos: egui::pos2(200.0, 790.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    crate::ui::develop_viewport::show(ui, &mut app, &frame);
                });
        },
    );
    assert_eq!(app.develop.geometry, original_crop);
    assert!(app.develop_ui.crop_drag.is_none());

    if let Ok(path) = std::env::var("CALIBRAW_PREVIEW_TEST_SCREENSHOT") {
        save_snapshot(
            &device,
            &queue,
            &mut renderer,
            &context,
            output.unwrap(),
            &path,
        );
    }

    // Real pointer and touch releases must reset zoom on brush canvases too.
    let mut time = 10.0;
    app.inpaint.tool = InpaintTool::Clone;
    for tab in [
        SidebarTab::Adjustments,
        SidebarTab::Masks,
        SidebarTab::Inpainting,
        SidebarTab::Crop,
    ] {
        for touch in [false, true] {
            for zoom in [0.7, 2.0] {
                app.ui.sidebar_tab = tab;
                app.preview.zoom = zoom;
                app.preview.center = [0.5; 2];
                let point = egui::pos2(200.0, 200.0);
                for pressed in [
                    Some(false),
                    None,
                    None,
                    Some(true),
                    Some(false),
                    Some(true),
                    Some(false),
                ] {
                    let mut events = vec![egui::Event::PointerMoved(point)];
                    if let Some(pressed) = pressed {
                        events.push(egui::Event::PointerButton {
                            pos: point,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        });
                        if touch {
                            events.push(egui::Event::Touch {
                                device_id: egui::TouchDeviceId(1),
                                id: egui::TouchId(1),
                                phase: if pressed {
                                    egui::TouchPhase::Start
                                } else {
                                    egui::TouchPhase::End
                                },
                                pos: point,
                                force: None,
                            });
                        }
                    }
                    time += 0.05;
                    let _ = context.run_ui(
                        egui::RawInput {
                            screen_rect: Some(screen),
                            time: Some(time),
                            events,
                            ..Default::default()
                        },
                        |ui| {
                            egui::CentralPanel::default().show(ui, |ui| {
                                crate::ui::preview::Preview::show(ui, &mut app, &frame);
                            });
                        },
                    );
                }
                assert_eq!(
                    app.preview.zoom, 1.0,
                    "tab={tab:?}, touch={touch}, zoom={zoom}"
                );
                assert_eq!(app.preview.center, [0.5; 2]);
                time += 1.0;
            }
        }
    }

    // Zooming out and immediately back must not let the late, coarser worker
    // result replace a native crop that is sufficient again.
    let source = Arc::clone(app.develop.loaded_raw.as_ref().unwrap());
    let full_uv = PreviewUvRect {
        min: [0.0; 2],
        max: [1.0; 2],
    };
    let processing_halo = app.preview_detail_halo();
    app.preview.detail = Some(PreviewDetail {
        processing_halo,
        pipeline: app.preview.gpu_pipeline.take().unwrap(),
        uv_rect: full_uv,
        texture_uv_rect: full_uv,
        revision: app.preview.revision,
        raw: Arc::clone(&source),
        source_origin: [0, 0],
        source_size: [600, 900],
        full_source_size: [600, 900],
        mask_source_region: [0, 0, 600, 900],
        virtual_origin: [0, 0],
        virtual_full_size: [600, 900],
    });
    app.preview.zoom = 4.0;
    app.preview.visible_uv = PreviewUvRect {
        min: [0.4; 2],
        max: [0.6; 2],
    };
    app.preview.motion_at = None;
    assert!(app.preview_detail_is_current());
    let (sender, receiver) = std::sync::mpsc::channel();
    sender
        .send(PreviewDetailRebuildEvent::Finished(Ok(
            PreparedPreviewDetail {
                processing_halo,
                raw: Arc::new(build_proxy(&source, ProxySpec { max_edge: 300 })),
                source_raw: source,
                revision: app.preview.revision,
                quality: app.preview.quality,
                visible: full_uv,
                texture_uv_rect: full_uv,
                source_origin: [0, 0],
                source_size: [600, 900],
            },
        )))
        .unwrap();
    app.preview.detail_rebuild_receiver = Some(receiver);
    app.poll_preview_detail_rebuild_worker(&frame);
    assert!(app.preview.detail_rebuild_receiver.is_none());
    assert!(
        app.preview.motion_at.is_none(),
        "a late result attempted an unnecessary GPU upload"
    );
    assert_eq!(
        app.preview
            .detail
            .as_ref()
            .unwrap()
            .pipeline
            .egui_texture_id,
        Some(image_id)
    );
}

fn save_snapshot(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut Renderer,
    context: &egui::Context,
    output: egui::FullOutput,
    path: &str,
) {
    let descriptor = ScreenDescriptor {
        size_in_pixels: [400, 800],
        pixels_per_point: context.pixels_per_point(),
    };
    let jobs = context.tessellate(output.shapes, descriptor.pixels_per_point);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("portrait layout fixture"),
        size: wgpu::Extent3d {
            width: 400,
            height: 800,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    let commands = renderer.update_buffers(device, queue, &mut encoder, &jobs, &descriptor);
    {
        let view = texture.create_view(&Default::default());
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        renderer.render(&mut pass.forget_lifetime(), &jobs, &descriptor);
    }
    const STRIDE: u32 = 1792; // 400 RGBA pixels, aligned to 256 bytes.
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(STRIDE) * 800,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(STRIDE),
                rows_per_image: None,
            },
        },
        texture.size(),
    );
    queue.submit(commands.into_iter().chain([encoder.finish()]));
    let (sender, receiver) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            sender.send(result).unwrap();
        });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    receiver.recv().unwrap().unwrap();
    let bytes = buffer.slice(..).get_mapped_range();
    let pixels = bytes
        .chunks(STRIDE as usize)
        .flat_map(|row| row[..1600].iter().copied())
        .collect();
    image::RgbaImage::from_raw(400, 800, pixels)
        .unwrap()
        .save(path)
        .unwrap();
}
