use super::*;

pub(super) fn show_export_action_panel<R>(
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

pub(super) fn enforce_export_bit_depth(
    format: ExportFormat,
    settings: &mut crate::pipeline::ExportSettings,
) {
    match format {
        ExportFormat::Jpeg => settings.bit_depth = ExportBitDepth::Eight,
        ExportFormat::Png | ExportFormat::JpegXl if settings.bit_depth.is_float() => {
            settings.bit_depth = ExportBitDepth::Sixteen
        }
        _ => {}
    }
}

const EXPORT_FIELD_WIDTH: f32 = 220.0;

fn export_number_row<Num>(
    ui: &mut Ui,
    label: &str,
    value: &mut Num,
    range: std::ops::RangeInclusive<Num>,
    suffix: &str,
    help: &str,
) where
    Num: egui::emath::Numeric + Copy,
{
    ui.push_id(label, |ui| {
        crate::ui::theme::form_row(ui, label, EXPORT_FIELD_WIDTH, |ui, width| {
            ui.allocate_ui_with_layout(
                egui::vec2(width, crate::ui::theme::CONTROL_HEIGHT),
                egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                |ui| {
                    let id = ui.next_auto_id();
                    crate::ui::components::adjustment_slider::step_focused_numeric_field(
                        ui,
                        id,
                        value,
                        range.clone(),
                    );
                    let decimals = if !Num::INTEGRAL
                        && (ui.memory(|memory| memory.has_focus(id))
                            || (value.to_f64() - value.to_f64().round()).abs() > 0.0001)
                    {
                        2
                    } else {
                        0
                    };
                    ui.add(
                        egui::DragValue::new(value)
                            .range(range)
                            .speed(1.0)
                            .suffix(suffix)
                            .fixed_decimals(decimals),
                    )
                    .on_hover_text(help);
                },
            );
        });
    });
}

pub(crate) fn export_settings_controls(
    ui: &mut Ui,
    format: &mut ExportFormat,
    settings: &mut crate::pipeline::ExportSettings,
    _fallback_picker_directory: Option<&std::path::Path>,
) -> bool {
    let previous_format = *format;
    crate::ui::theme::section_card(ui, "Format", |ui| {
        ui.horizontal(|ui| {
            let spacing = ui.spacing().item_spacing.x;
            let format_width = ((ui.available_width() - spacing * 3.0) / 4.0).max(1.0);
            for (export_format, label) in [
                (ExportFormat::Jpeg, "JPEG"),
                (ExportFormat::Png, "PNG"),
                (ExportFormat::Tiff, "TIFF"),
                (ExportFormat::JpegXl, "JXL"),
            ] {
                if crate::ui::theme::segmented_button(
                    ui,
                    label,
                    *format == export_format,
                    format_width,
                )
                .clicked()
                {
                    *format = export_format;
                }
            }
        });
        enforce_export_bit_depth(*format, settings);
        crate::ui::theme::section_separator(ui);

        if *format == ExportFormat::Jpeg {
            export_number_row(
                ui,
                "JPEG quality",
                &mut settings.jpeg_quality,
                1..=100,
                " / 100",
                "Enter any quality from 1 to 100. Higher quality keeps more detail and produces a larger JPEG file.",
            );
            let default_quality = crate::pipeline::ExportSettings::default().jpeg_quality;
            ui.horizontal(|ui| {
                let spacing = ui.spacing().item_spacing.x;
                let choice_width = ((ui.available_width() - spacing * 2.0) / 3.0).max(1.0);
                let default_help = format!("Restore the default JPEG quality: {default_quality}.");
                for (quality, label, help) in [
                    (80, "Small", "Quality 80: smaller files."),
                    (default_quality, "Default", default_help.as_str()),
                    (
                        100,
                        "Max",
                        "Quality 100: maximum JPEG quality, with larger files.",
                    ),
                ] {
                    if crate::ui::theme::segmented_button(
                        ui,
                        label,
                        settings.jpeg_quality == quality,
                        choice_width,
                    )
                    .on_hover_text(help)
                    .clicked()
                    {
                        settings.jpeg_quality = quality;
                    }
                }
            });
            ui.small("8-bit output. Higher quality produces larger files.");
        } else {
            crate::ui::theme::form_combo_with_help(
                ui,
                "Precision",
                "export-bit-depth",
                settings.bit_depth.label(),
                EXPORT_FIELD_WIDTH,
                "8-bit is broadly compatible; 16-bit retains more tonal precision; 32-bit float writes a scene-linear TIFF master.",
                |ui| {
                    for depth in [
                        ExportBitDepth::Eight,
                        ExportBitDepth::Sixteen,
                        ExportBitDepth::Float32Linear,
                    ] {
                        let supported = match *format {
                            ExportFormat::Jpeg => depth == ExportBitDepth::Eight,
                            ExportFormat::Png => !depth.is_float(),
                            ExportFormat::Tiff => true,
                            ExportFormat::JpegXl => !depth.is_float(),
                        };
                        if supported {
                            ui.selectable_value(&mut settings.bit_depth, depth, depth.label());
                        }
                    }
                },
            );
            ui.small(if settings.bit_depth.is_float() {
                "Scene-linear TIFF master for further editing."
            } else {
                "Higher precision preserves finer tonal detail for further editing."
            });
        }
    });
    crate::ui::theme::card_gap(ui);

    crate::ui::theme::section_card_with_help(
        ui,
        "Resize",
        "Choose how the exported image is sized. Edge and dimension modes preserve the aspect ratio.",
        |ui| {
            crate::ui::theme::form_combo(
                ui,
                "Size",
                "export-resize-mode",
                settings.resize_mode.label(),
                EXPORT_FIELD_WIDTH,
                |ui| {
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
                },
            );

            match settings.resize_mode {
                ExportResizeMode::Original => {
                    ui.small("Use the full dimensions after cropping and transforms.");
                }
                ExportResizeMode::Percentage => {
                    export_number_row(
                        ui,
                        "Scale",
                        &mut settings.percentage,
                        1.0..=400.0,
                        "%",
                        "Percentage of the image dimensions after cropping and transforms. Accepts decimal values from 1 to 400%.",
                    );
                }
                ExportResizeMode::LongEdge
                | ExportResizeMode::ShortEdge
                | ExportResizeMode::Width
                | ExportResizeMode::Height => {
                    export_number_row(
                        ui,
                        settings.resize_mode.label(),
                        &mut settings.edge_or_dimension,
                        1..=crate::pipeline::MAX_EXPORT_EDGE,
                        " px",
                        "Target size in pixels; the aspect ratio is preserved.",
                    );
                }
            }

            if settings.resize_mode != ExportResizeMode::Original {
                ui.small("Aspect ratio is preserved.");
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

    crate::ui::theme::section_card(ui, "Metadata", |ui| {
        crate::ui::theme::checkbox_with_help(
            ui,
            &mut settings.keep_metadata,
            "Keep metadata",
            "Embeds available source, camera, lens, exposure, creator, original-size, software, and normalized-orientation metadata in the exported image.",
        );
    });

    *format != previous_format
}

impl Sidebar {
    pub(super) fn show_export_action(ui: &mut Ui, app: &mut CalibRawApp, frame: &eframe::Frame) {
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
                ExportFormat::JpegXl => app.export_jxl(frame),
            }
        }
    }

    pub(super) fn show_export(ui: &mut Ui, app: &mut CalibRawApp, _frame: &eframe::Frame) {
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
                let export_enabled = app.can_export();
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
