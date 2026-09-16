fn show_export_action_panel<R>(
    ui: &mut Ui,
    contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    egui::Panel::bottom("develop-export-action")
        .resizable(false)
        .exact_size(crate::ui::theme::CONTROL_HEIGHT + 2.0 * crate::ui::theme::SPACE_SM)
        .frame(
            egui::Frame::new()
                .fill(ui.visuals().panel_fill)
                .inner_margin(egui::Margin::symmetric(0, crate::ui::theme::SPACE_SM as i8)),
        )
        .show(ui, contents)
}

fn enforce_export_bit_depth(format: ExportFormat, settings: &mut crate::pipeline::ExportSettings) {
    match format {
        ExportFormat::Jpeg => settings.bit_depth = ExportBitDepth::Eight,
        ExportFormat::Png if settings.bit_depth.is_float() => {
            settings.bit_depth = ExportBitDepth::Sixteen
        }
        _ => {}
    }
}

pub(crate) fn export_settings_controls(
    ui: &mut Ui,
    format: &mut ExportFormat,
    settings: &mut crate::pipeline::ExportSettings,
    _fallback_picker_directory: Option<&std::path::Path>,
) -> bool {
    let previous_format = *format;
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(format, ExportFormat::Jpeg, "JPEG");
        ui.selectable_value(format, ExportFormat::Png, "PNG");
        ui.selectable_value(format, ExportFormat::Tiff, "TIFF");
    });
    enforce_export_bit_depth(*format, settings);
    crate::ui::theme::card_gap(ui);

    crate::ui::theme::section_card_with_help(
        ui,
        "Resize",
        "Choose how the exported image is sized. Edge and dimension modes preserve the aspect ratio.",
        |ui| {
            crate::ui::theme::combo_box(
                "export-resize-mode",
                settings.resize_mode.label(),
                ui.available_width().max(1.0),
            )
            .show_ui(ui, |ui| {
                for mode in [
                    ExportResizeMode::Original,
                    ExportResizeMode::LongEdge,
                    ExportResizeMode::ShortEdge,
                    ExportResizeMode::Width,
                    ExportResizeMode::Height,
                    ExportResizeMode::Percentage,
                ] {
                    ui.selectable_value(&mut settings.resize_mode, mode, mode.label());
                }
            });

            match settings.resize_mode {
                ExportResizeMode::Original => {}
                ExportResizeMode::Percentage => {
                    crate::ui::theme::form_row(ui, "Scale", 112.0, |ui, width| {
                        ui.add_sized(
                            [width, crate::ui::theme::CONTROL_HEIGHT],
                            egui::DragValue::new(&mut settings.percentage)
                                .range(1.0..=400.0)
                                .speed(1.0)
                                .suffix("%")
                                .fixed_decimals(0),
                        )
                        .on_hover_text("Percentage of the original image dimensions.");
                    });
                }
                ExportResizeMode::LongEdge
                | ExportResizeMode::ShortEdge
                | ExportResizeMode::Width
                | ExportResizeMode::Height => {
                    crate::ui::theme::form_row(ui, "Pixels", 112.0, |ui, width| {
                        ui.add_sized(
                            [width, crate::ui::theme::CONTROL_HEIGHT],
                            egui::DragValue::new(&mut settings.edge_or_dimension)
                                .range(1..=crate::pipeline::MAX_EXPORT_EDGE)
                                .speed(10.0)
                                .suffix(" px"),
                        )
                        .on_hover_text("Target size in pixels; the aspect ratio is preserved.");
                    });
                }
            }

            if settings.resize_mode != ExportResizeMode::Original {
                crate::ui::theme::checkbox_with_help(
                    ui,
                    &mut settings.allow_upscale,
                    "Allow upscaling",
                    "Allow exports to exceed the original image dimensions when the requested size is larger.",
                );
            }
        },
    );
    crate::ui::theme::card_gap(ui);

    if *format != ExportFormat::Jpeg {
        crate::ui::theme::section_card_with_help(
            ui,
            "Precision",
            "Choose the channel precision written to the exported file. Higher precision preserves more editing latitude but produces larger files.",
            |ui| {
                crate::ui::theme::combo_box(
                    "export-bit-depth",
                    settings.bit_depth.label(),
                    ui.available_width().max(1.0),
                )
                    .show_ui(ui, |ui| {
                        for depth in [
                            ExportBitDepth::Eight,
                            ExportBitDepth::Sixteen,
                            ExportBitDepth::Float32Linear,
                        ] {
                            let supported = match *format {
                                ExportFormat::Jpeg => depth == ExportBitDepth::Eight,
                                ExportFormat::Png => !depth.is_float(),
                                ExportFormat::Tiff => true,
                            };
                            if supported {
                                ui.selectable_value(
                                    &mut settings.bit_depth,
                                    depth,
                                    depth.label(),
                                );
                            }
                        }
                    })
                    .response
                    .on_hover_text("8-bit is broadly compatible; 16-bit retains more tonal precision; 32-bit float writes a scene-linear TIFF master.");
            },
        );
        crate::ui::theme::card_gap(ui);
    }

    crate::ui::theme::section_card(ui, "Metadata", |ui| {
        crate::ui::theme::checkbox_with_help(
            ui,
            &mut settings.keep_metadata,
            "Keep metadata",
            "Embeds available source, camera, lens, exposure, creator, original-size, software, and normalized-orientation metadata in the exported image.",
        );
    });

    if *format == ExportFormat::Jpeg {
        crate::ui::theme::card_gap(ui);
        crate::ui::theme::section_card(ui, "JPEG", |ui| {
            adjustment_slider_with_reset(
                ui,
                "Quality",
                &mut settings.jpeg_quality,
                1..=100,
                0,
                1.0,
                Some("Higher quality keeps more detail and produces a larger JPEG file."),
                crate::pipeline::ExportSettings::default().jpeg_quality,
            );
        });
    }
    *format != previous_format
}

impl Sidebar {
    fn show_export_action(ui: &mut Ui, app: &mut CalibRawApp, frame: &eframe::Frame) {
        let dimensions_valid = app.develop.loaded_raw.as_ref().is_some_and(|raw| {
            let (width, height) = app
                .develop
                .geometry
                .crop_pixel_dimensions(raw.width, raw.height);
            app.export
                .settings
                .checked_output_dimensions(width, height)
                .is_ok()
        });
        let export_enabled = app.can_export() && dimensions_valid;

        let response = ui
            .add_enabled_ui(export_enabled, |ui| {
                crate::ui::theme::full_width_button(ui, "Export…")
            })
            .inner;
        if response.clicked() {
            match app.export.format {
                ExportFormat::Jpeg => app.export_jpeg(frame),
                ExportFormat::Png => app.export_png(frame),
                ExportFormat::Tiff => app.export_tiff(frame),
            }
        }
    }

    fn show_export(ui: &mut Ui, app: &mut CalibRawApp, _frame: &eframe::Frame) {
        let content_width = ui.available_width().max(1.0);
        let column_width = content_width;

        #[cfg(not(target_os = "android"))]
        let export_picker_directory = app
            .develop
            .current_path
            .as_deref()
            .and_then(|path| path.parent())
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(std::path::Path::to_path_buf);
        #[cfg(target_os = "android")]
        let export_picker_directory: Option<std::path::PathBuf> = None;
        ui.allocate_ui_with_layout(
            egui::vec2(column_width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_width(column_width);
                ui.set_max_width(column_width);
                let format_changed = export_settings_controls(
                    ui,
                    &mut app.export.format,
                    &mut app.export.settings,
                    export_picker_directory.as_deref(),
                );
                if format_changed {
                    app.persist_performance_settings();
                }

                #[cfg(not(target_os = "android"))]
                if let Some((fraction, phase)) = app.edit_replay_progress_state() {
                    ui.add_space(crate::ui::theme::SPACE_SM);
                    ui.add_sized(
                        [ui.available_width(), 18.0],
                        egui::ProgressBar::new(fraction).text(phase),
                    );
                }

                if let Some((completed, total)) = app.export_progress_state() {
                    ui.add_space(crate::ui::theme::SPACE_SM);
                    let (fraction, text) = if total == 0 {
                        (0.0, "Preparing export…".to_owned())
                    } else {
                        (
                            (completed as f32 / total as f32).clamp(0.0, 1.0),
                            format!("Exporting — {completed}/{total} tiles"),
                        )
                    };
                    ui.add_sized(
                        [ui.available_width(), 18.0],
                        egui::ProgressBar::new(fraction).text(text),
                    );
                    if let Some((done, batch_total)) = app.library_batch_export_progress() {
                        ui.label(
                            egui::RichText::new(format!(
                                "Batch: {done}/{batch_total} images completed"
                            ))
                            .small()
                            .color(ui.visuals().weak_text_color()),
                        );
                    }
                }

                ui.add_space(crate::ui::theme::SPACE_SM);
                #[cfg(not(target_os = "android"))]
                let export_enabled = app.can_export();
                #[cfg(not(target_os = "android"))]
                {
                    crate::ui::theme::section_separator(ui);
                    let replay_response = ui
                        .add_enabled_ui(export_enabled, |ui| {
                            crate::ui::theme::full_width_button(ui, "Create Edit Replay…")
                        })
                        .inner
                        .on_hover_text(
                            "Create a short 30 FPS MP4 that replays the current edit by category.",
                        );
                    if replay_response.clicked() {
                        app.create_edit_replay(_frame);
                    }
                }
                if app.export_task_active() {
                    ui.label(
                        egui::RichText::new(
                            "An export is already running. Minimize its progress window to keep editing.",
                        )
                        .small()
                        .color(ui.visuals().weak_text_color()),
                    );
                } else if !app.can_export() && app.export_progress_state().is_none() {
                    ui.label(
                        egui::RichText::new(
                            "Export becomes available after a RAW image has finished loading.",
                        )
                        .small()
                        .color(ui.visuals().weak_text_color()),
                    );
                }
            },
        );
    }
}
