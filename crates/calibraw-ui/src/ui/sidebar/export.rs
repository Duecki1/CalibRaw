use super::*;

pub(super) fn show_export_action_panel<R>(
    ui: &mut Ui,
    contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    egui::Panel::bottom("develop-export-action")
        .resizable(false)
        .exact_size(moduwu_design::CONTROL_HEIGHT + 2.0 * moduwu_design::SPACE_SM)
        .frame(
            egui::Frame::new()
                .fill(ui.visuals().panel_fill)
                .inner_margin(egui::Margin::symmetric(0, moduwu_design::SPACE_SM as i8)),
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
        moduwu_design::form_row(ui, label, EXPORT_FIELD_WIDTH, |ui, width| {
            ui.allocate_ui_with_layout(
                egui::vec2(width, moduwu_design::CONTROL_HEIGHT),
                egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                |ui| {
                    ui.add(moduwu_design::NumberField::new(value, range).suffix(suffix))
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
    moduwu_design::section_card(ui, "Format", |ui| {
        ui.horizontal(|ui| {
            let spacing = ui.spacing().item_spacing.x;
            let format_width = ((ui.available_width() - spacing * 3.0) / 4.0).max(1.0);
            for (export_format, label) in [
                (ExportFormat::Jpeg, "JPEG"),
                (ExportFormat::Png, "PNG"),
                (ExportFormat::Tiff, "TIFF"),
                (ExportFormat::JpegXl, "JXL"),
            ] {
                if moduwu_design::segmented_button(
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
        moduwu_design::section_separator(ui);

        if *format == ExportFormat::Jpeg {
            let default_quality = crate::pipeline::ExportSettings::default().jpeg_quality;
            AdjustmentSlider::new("JPEG quality", &mut settings.jpeg_quality, 1..=100)
                .reset_to(default_quality)
                .hover_text(
                    "Quality from 1 to 100. Higher quality keeps more detail and produces a larger JPEG file.",
                )
                .show(ui);
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
                    if moduwu_design::segmented_button(
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
        } else {
            moduwu_design::form_combo_with_help(
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
        }
    });
    moduwu_design::card_gap(ui);

    moduwu_design::section_card_with_help(
        ui,
        "Resize",
        "Choose how the exported image is sized. Original uses the full dimensions after cropping and transforms; edge and dimension modes preserve the aspect ratio.",
        |ui| {
            moduwu_design::form_combo(
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
                ExportResizeMode::Original => {}
                ExportResizeMode::Percentage => {
                    AdjustmentSlider::new("Scale (%)", &mut settings.percentage, 1.0..=400.0)
                        .decimals(1)
                        .step(0.1)
                        .reset_to(crate::pipeline::ExportSettings::default().percentage)
                        .hover_text(
                            "Percentage of the image dimensions after cropping and transforms, from 1 to 400%.",
                        )
                        .show(ui);
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
                moduwu_design::toggle_with_help(
                    ui,
                    &mut settings.allow_upscale,
                    "Allow upscaling",
                    "Allow exports to exceed the original image dimensions when the requested size is larger.",
                );
            }
        },
    );
    moduwu_design::card_gap(ui);

    moduwu_design::section_card(ui, "Metadata", |ui| {
        moduwu_design::toggle_with_help(
            ui,
            &mut settings.keep_metadata,
            "Keep metadata",
            "Embeds available source, camera, lens, exposure, creator, original-size, software, and normalized-orientation metadata in the exported image.",
        );
    });

    *format != previous_format
}

/// A square, glyph-only touch action filling `rect` exactly.
#[cfg(target_os = "android")]
fn square_icon_action(
    ui: &mut Ui,
    rect: egui::Rect,
    enabled: bool,
    glyph: &str,
    label: &str,
) -> egui::Response {
    let response = ui
        .scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            ui.add_enabled_ui(enabled, |ui| {
                // The touch button padding would widen the square past `rect`.
                ui.spacing_mut().button_padding = egui::Vec2::ZERO;
                let response = ui.add(egui::Button::new("").min_size(rect.size()));
                // Painted rather than laid out by the button, which does not
                // centre a lone glyph in a fixed-size button. The disabled
                // painter fades it with the frame.
                let color = ui.style().interact(&response).text_color();
                ui.painter().text(
                    response.rect.center(),
                    egui::Align2::CENTER_CENTER,
                    glyph,
                    egui::FontId::proportional(rect.height() * 0.55),
                    color,
                );
                response
            })
            .inner
        })
        .inner
        .on_hover_text(label);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    response
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

        // Android row: square replay and share actions flank a shortened Export.
        // Each control gets an exact rect so the touch button padding cannot
        // push the row past the panel edge.
        #[cfg(target_os = "android")]
        let response = {
            let edge = moduwu_design::Metrics::of(ui.ctx()).control_height;
            let gap = ui.spacing().item_spacing.x;
            let (row, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width().max(3.0 * edge), edge),
                egui::Sense::hover(),
            );
            let replay_rect = egui::Rect::from_min_size(row.min, egui::vec2(edge, edge));
            let share_rect = egui::Rect::from_min_size(
                egui::pos2(row.right() - edge, row.top()),
                egui::vec2(edge, edge),
            );
            let export_rect = egui::Rect::from_min_max(
                egui::pos2(replay_rect.right() + gap, row.top()),
                egui::pos2(
                    (share_rect.left() - gap).max(replay_rect.right() + gap),
                    row.bottom(),
                ),
            );

            if square_icon_action(
                ui,
                replay_rect,
                app.can_export(),
                egui_phosphor::regular::FILM_STRIP,
                "Create and share edit replay",
            )
            .clicked()
            {
                app.create_edit_replay(frame);
            }
            let export = ui
                .scope_builder(egui::UiBuilder::new().max_rect(export_rect), |ui| {
                    ui.add_enabled(
                        export_enabled,
                        egui::Button::new("Export…").min_size(export_rect.size()),
                    )
                })
                .inner;
            if square_icon_action(
                ui,
                share_rect,
                export_enabled,
                egui_phosphor::regular::SHARE_NETWORK,
                "Export and share",
            )
            .clicked()
            {
                app.export_and_share(frame);
            }
            export
        };
        #[cfg(not(target_os = "android"))]
        let response = ui
            .add_enabled_ui(export_enabled, |ui| {
                moduwu_design::full_width_button(ui, "Export…")
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
                    ui.add_space(moduwu_design::SPACE_SM);
                    ui.add_sized(
                        [ui.available_width(), 18.0],
                        egui::ProgressBar::new(fraction).text(phase),
                    );
                }

                if let Some((completed, total)) = app.export_progress_state() {
                    ui.add_space(moduwu_design::SPACE_SM);
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

                ui.add_space(moduwu_design::SPACE_SM);
                // Android offers the replay as an icon beside Export.
                #[cfg(not(target_os = "android"))]
                {
                    let export_enabled = app.can_export();
                    moduwu_design::section_separator(ui);
                    let replay_response = ui
                        .add_enabled_ui(export_enabled, |ui| {
                            moduwu_design::full_width_button(ui, "Create Edit Replay…")
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
