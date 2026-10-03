//! Opt-in visual review of the real app, with a generated scene and no downloads.
//!
//! From the workspace root:
//! ```text
//! CALIBRAW_UI_REVIEW_DIR=/tmp/calibraw-ui-baseline cargo test -p calibraw-ui --lib app::ui_review_tests::gpu_ui_review -- --ignored --exact --nocapture --test-threads=1
//! ```
//! Requires the normal desktop build dependencies and a working wgpu adapter
//! (a software Vulkan adapter also works); no window/display server is needed.
//! CALIBRAW_UI_REVIEW_FILTER optionally selects comma-separated substrings of
//! `theme/size/state.png`, e.g. `obsidian-blue/desktop-1280x800/presets-populated`.
//! With no filter, all 132 PNGs are captured. Reuse a directory to replace the
//! same captures, or separate baseline/after directories for manual comparison.
//! A child test process isolates config/cache without mutating this process's
//! environment. The parent bounds the entire capture, including GPU startup.

use super::*;
use crate::performance_settings::PerformanceSettings;
use eframe::egui_wgpu::{Renderer, RendererOptions, ScreenDescriptor};
use std::process::Command;

const CHILD_ROOT: &str = "CALIBRAW_UI_REVIEW_CHILD_ROOT";
const TEST_NAME: &str = "app::ui_review_tests::gpu_ui_review";
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(600);
const GPU_TIMEOUT: Duration = Duration::from_secs(30);
const SETTLE_FRAMES: usize = 4;

const THEMES: [(UiDesign, &str); 4] = [
    (UiDesign::ObsidianBlue, "obsidian-blue"),
    (UiDesign::ObsidianRed, "obsidian-red"),
    (UiDesign::Porcelain, "porcelain"),
    (UiDesign::DaylightBlue, "daylight-blue"),
];
const SIZES: [(&str, [u32; 2]); 3] = [
    ("desktop-1280x800", [1280, 800]),
    ("compact-900x680", [900, 680]),
    ("portrait-480x800", [480, 800]),
];
const CASES: [(&str, AppTab, SidebarTab); 11] = [
    ("presets-populated", AppTab::Develop, SidebarTab::Presets),
    ("presets-empty", AppTab::Develop, SidebarTab::Presets),
    ("adjustments", AppTab::Develop, SidebarTab::Adjustments),
    ("crop", AppTab::Develop, SidebarTab::Crop),
    ("export", AppTab::Develop, SidebarTab::Export),
    ("masks", AppTab::Develop, SidebarTab::Masks),
    ("inpainting", AppTab::Develop, SidebarTab::Inpainting),
    ("info", AppTab::Develop, SidebarTab::Info),
    ("settings", AppTab::Settings, SidebarTab::Adjustments),
    ("library", AppTab::Library, SidebarTab::Adjustments),
    ("preset-create", AppTab::Develop, SidebarTab::Presets),
];

#[test]
#[ignore = "GPU screenshot review; set CALIBRAW_UI_REVIEW_DIR (optional CALIBRAW_UI_REVIEW_FILTER)"]
fn gpu_ui_review() {
    let output = PathBuf::from(
        std::env::var_os("CALIBRAW_UI_REVIEW_DIR")
            .filter(|value| !value.is_empty())
            .expect("set CALIBRAW_UI_REVIEW_DIR to the screenshot output directory"),
    );
    std::fs::create_dir_all(&output).expect("create screenshot output directory");
    let output = output.canonicalize().unwrap();
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        capture_review(&PathBuf::from(root), &output);
        return;
    }

    let isolated = tempfile::tempdir().expect("create isolated review config/cache");
    let config = isolated.path().join("config");
    let cache = isolated.path().join("cache");
    let data = isolated.path().join("data");
    for folder in [&config, &cache, &data] {
        std::fs::create_dir_all(folder).unwrap();
    }
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            TEST_NAME,
            "--ignored",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ROOT, isolated.path())
        .env("CALIBRAW_UI_REVIEW_DIR", &output)
        .env("XDG_CONFIG_HOME", &config)
        .env("XDG_CACHE_HOME", &cache)
        .env("XDG_DATA_HOME", &data)
        .env("APPDATA", &config)
        .env("LOCALAPPDATA", &cache)
        .spawn()
        .expect("start isolated screenshot test process");
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("poll screenshot test process") {
            assert!(status.success(), "screenshot test process failed: {status}");
            break;
        }
        if started.elapsed() >= CAPTURE_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "GPU UI review exceeded {CAPTURE_TIMEOUT:?}; partial PNGs: {}",
                output.display()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn capture_review(root: &Path, output_dir: &Path) {
    let filters: Vec<String> = std::env::var("CALIBRAW_UI_REVIEW_FILTER")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect();
    let selected =
        |name: &str| filters.is_empty() || filters.iter().any(|part| name.contains(part));
    let mut filenames = Vec::new();
    for (_, theme) in THEMES {
        for (size, _) in SIZES {
            for (case, _, _) in CASES {
                let name = format!("{theme}/{size}/{case}.png");
                if selected(&name) {
                    filenames.push(name);
                }
            }
        }
    }
    assert!(
        !filenames.is_empty(),
        "CALIBRAW_UI_REVIEW_FILTER matched no screenshots"
    );

    let settings_path = root.join("config/calibraw/performance.json");
    let presets = presets::preset_folder_for_settings(&settings_path).unwrap();
    seed_presets(&presets);
    let empty_presets = root.join("empty-presets");
    std::fs::create_dir_all(&empty_presets).unwrap();

    eprintln!(
        "Capturing {} GPU UI screenshots in {}",
        filenames.len(),
        output_dir.display()
    );
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.memory_budget_thresholds = crate::memory_budget_thresholds();
    let instance = wgpu::Instance::new(descriptor);
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .expect("UI review needs a working wgpu adapter (hardware or software Vulkan)");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .expect("create UI review GPU device");
    std::fs::write(
        output_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "source_revision": crate::SOURCE_REVISION,
            "adapter": format!("{:?}", adapter.get_info()),
            "scene": "deterministic generated scene, 768x512, linear Rec.2020",
            "pixels_per_point": 1,
            "settle_frames": SETTLE_FRAMES,
            "screenshots": filenames,
        }))
        .unwrap(),
    )
    .unwrap();

    let raw = Arc::new(review_scene());
    let mut masks = MaskStack::default();
    let (mask, _) = masks.add_mask(MaskKind::Radial).unwrap();
    masks.masks[mask].name = "Foreground light".to_owned();
    let params = GpuParams::new(&ExposureParams::scene_referred_default(), &masks, &raw);
    let mut pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
        &device,
        &queue,
        &raw,
        &params,
        ProcessingQuality::Preview,
        256,
    )
    .expect("create review scene GPU pipeline");
    pipeline.recompute(&queue, &device, &params);

    let mut count = 0;
    for (design, theme_name) in THEMES {
        for (size_name, size) in SIZES {
            let prefix = format!("{theme_name}/{size_name}/");
            if !filenames.iter().any(|name| name.starts_with(&prefix)) {
                continue;
            }
            let context = egui::Context::default();
            crate::ui::theme::install(&context);
            crate::ui::theme::apply(&context, design);
            context.set_pixels_per_point(1.0);
            context.all_styles_mut(|style| style.animation_time = 0.0);
            let settings = PerformanceSettings {
                ui_design: design,
                onboarding_completed: true,
                auto_check_updates: false,
                github_update_check_allowed: Some(false),
                camera_profile_auto_detect: false,
                thumbnail_workers: 1,
                develop_filmstrip_open: false,
                ..PerformanceSettings::default()
            };
            crate::performance_settings::save(Some(&settings_path), settings.clone()).unwrap();
            // Same constructor as empty(), with an explicit isolated settings path.
            let mut app = CalibRawApp::from_performance_settings(
                &context,
                Some(settings_path.clone()),
                settings,
            );
            app.develop.loaded_raw = Some(Arc::clone(&raw));
            app.develop.preview_raw = Some(Arc::clone(&raw));
            app.develop.original_raw = Some(Arc::clone(&raw));
            app.develop.current_label = Some("Generated landscape".to_owned());
            app.develop.image_status = "Generated landscape · 768 × 512".to_owned();
            app.masks.stack = masks.clone();
            app.inpaint.tool = InpaintTool::Clone;
            app.reset_edit_history();
            let mut renderer = Renderer::new(
                &device,
                wgpu::TextureFormat::Rgba8Unorm,
                RendererOptions::default(),
            );
            pipeline.register_egui_texture(&device, &mut renderer);
            let image_id = pipeline.egui_texture_id.unwrap();
            app.preview.gpu_pipeline = Some(pipeline);
            let mut frame = eframe::Frame::_new_kittest();
            let mut frame_number = 0;
            for (case, tab, sidebar) in CASES {
                let name = format!("{prefix}{case}.png");
                if !selected(&name) {
                    continue;
                }
                app.ui.active_tab = tab;
                app.ui.sidebar_tab = sidebar;
                app.presets = PresetState::load(Some(if case == "presets-empty" {
                    empty_presets.clone()
                } else {
                    presets.clone()
                }));
                assert!(app.presets.load_failures.is_empty());
                assert_eq!(
                    app.presets.all().len(),
                    if case == "presets-empty" { 0 } else { 6 }
                );
                if case == "preset-create" {
                    app.open_new_preset_editor();
                    let editor = app.presets.editor.as_mut().expect("create preset dialog");
                    editor.name = "Evening light".to_owned();
                    editor.group = "Landscape".to_owned();
                }
                let mut final_output = None;
                for _ in 0..SETTLE_FRAMES {
                    let output = context.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(size[0] as f32, size[1] as f32),
                            )),
                            time: Some(frame_number as f64 / 10.0),
                            ..Default::default()
                        },
                        |ui| eframe::App::ui(&mut app, ui, &mut frame),
                    );
                    frame_number += 1;
                    for (id, delta) in &output.textures_delta.set {
                        renderer.update_texture(&device, &queue, *id, delta);
                    }
                    // Keep the final frame's frees until after its render submission.
                    if let Some(previous) = final_output.replace(output) {
                        for id in previous.textures_delta.free {
                            renderer.free_texture(&id);
                        }
                    }
                }
                let output = final_output.unwrap();
                let jobs = context.tessellate(output.shapes, context.pixels_per_point());
                if tab == AppTab::Develop {
                    assert!(
                        jobs.iter().any(|job| matches!(
                            &job.primitive, egui::epaint::Primitive::Mesh(mesh)
                                if mesh.texture_id == image_id && !mesh.indices.is_empty()
                        )),
                        "{name}: the actual GPU preview must be painted"
                    );
                }
                let path = output_dir.join(&name);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                save_snapshot(&device, &queue, &mut renderer, &jobs, size, &path);
                for id in output.textures_delta.free {
                    renderer.free_texture(&id);
                }
                count += 1;
                eprintln!("[{count}/{}] {}", filenames.len(), path.display());
            }
            pipeline = app
                .preview
                .gpu_pipeline
                .take()
                .expect("retain reusable scene pipeline");
        }
    }
    assert_eq!(count, filenames.len());
}

fn seed_presets(folder: &Path) {
    let selection = EditSelection {
        adjustment_groups: crate::pipeline::AdjustmentGroupSet::ALL,
        ..EditSelection::default()
    };
    for (index, (group, name)) in [
        ("Everyday", "Clean and natural"),
        ("Everyday", "Soft contrast"),
        ("Landscape", "Golden hour"),
        ("Landscape", "Cool mountain shadows"),
        ("Portrait", "Gentle skin tones"),
        ("Portrait", "Window light with lifted shadows"),
    ]
    .into_iter()
    .enumerate()
    {
        let mut edits = crate::sidecar::default_edit_state();
        edits.exposure.exposure = (index as f32 - 2.0) * 0.15;
        let preset = crate::presets::Preset::new(name, group, selection, &edits).unwrap();
        crate::presets::save_new_preset(folder, &preset).expect("seed actual preset files");
    }
}

fn review_scene() -> LoadedRaw {
    let (width, height) = (768, 512);
    let mut rgb = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        for x in 0..width {
            let u = x as f32 / width as f32;
            let v = y as f32 / height as f32;
            let ridge = 0.46 + 0.13 * (u * 13.0).sin() + 0.05 * (u * 31.0).cos();
            let foreground = 0.76 + 0.07 * (u * 9.0).sin();
            let color = if (u - 0.76).powi(2) + (v - 0.23).powi(2) < 0.003 {
                [1.4, 1.0, 0.52]
            } else if v < ridge {
                [0.13 + 0.32 * v, 0.26 + 0.28 * v, 0.57 + 0.18 * v]
            } else if v < foreground {
                [0.07 + 0.07 * u, 0.14 + 0.09 * u, 0.19 + 0.08 * u]
            } else {
                let texture = ((x / 12 + y / 12) % 2) as f32 * 0.015;
                [0.17 + texture, 0.24 + 0.12 * u + texture, 0.08 + texture]
            };
            rgb.extend_from_slice(&color);
        }
    }
    let mut raw = LoadedRaw::from_scene_linear_rec2020(width as u32, height as u32, rgb).unwrap();
    raw.camera_make = "CalibRaw".to_owned();
    raw.camera_model = "Generated review scene".to_owned();
    raw.lens_model = "Synthetic landscape".to_owned();
    raw.focal_length = 35.0;
    raw.aperture = 5.6;
    raw.capture_metadata.iso_speed = 100.0;
    raw.capture_metadata.shutter_seconds = 1.0 / 125.0;
    raw
}

/// Adapted from preview_tests::save_snapshot, kept here to avoid changing that
/// test's ownership/API. Uses arbitrary dimensions, padded rows and bounded waits.
fn save_snapshot(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut Renderer,
    jobs: &[egui::ClippedPrimitive],
    size: [u32; 2],
    path: &Path,
) {
    let descriptor = ScreenDescriptor {
        size_in_pixels: size,
        pixels_per_point: 1.0,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("UI review screenshot"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
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
    let commands = renderer.update_buffers(device, queue, &mut encoder, jobs, &descriptor);
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
        renderer.render(&mut pass.forget_lifetime(), jobs, &descriptor);
    }
    let row_bytes = size[0] * 4;
    let stride =
        row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("UI review readback"),
        size: u64::from(stride) * u64::from(size[1]),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: None,
            },
        },
        texture.size(),
    );
    let submission = queue.submit(commands.into_iter().chain([encoder.finish()]));
    let (sender, receiver) = mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(GPU_TIMEOUT),
        })
        .expect("GPU screenshot submission must complete within 30 seconds");
    receiver
        .recv_timeout(GPU_TIMEOUT)
        .expect("GPU readback callback timed out")
        .expect("map screenshot pixels");
    let bytes = buffer.slice(..).get_mapped_range();
    let pixels = bytes
        .chunks_exact(stride as usize)
        .flat_map(|row| row[..row_bytes as usize].iter().copied())
        .collect();
    image::RgbaImage::from_raw(size[0], size[1], pixels)
        .unwrap()
        .save(path)
        .expect("save review PNG");
    drop(bytes);
    buffer.unmap();
}
