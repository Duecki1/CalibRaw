use super::*;

fn inpaint_tool_help(tool: InpaintTool) -> &'static str {
    match tool {
        InpaintTool::Remove => {
            "Paint over unwanted content, then release to fill the area using its surroundings."
        }
        InpaintTool::Clone => {
            "Choose a source point, then paint to copy that area."
        }
        InpaintTool::Heal => {
            "Choose a source point, then paint to blend its texture with the surrounding color and light."
        }
    }
}

fn retouch_alignment_help(alignment: RetouchAlignment) -> &'static str {
    match alignment {
        RetouchAlignment::None => "Source follows this stroke, then returns to the selected point.",
        RetouchAlignment::Aligned => "Source keeps the same offset between separate strokes.",
        RetouchAlignment::Registered => "Source and destination use the same image coordinates.",
        RetouchAlignment::Fixed => "Every brush dab starts from the selected source point.",
    }
}

impl Sidebar {
    pub(crate) fn show_inpainting(
        ui: &mut Ui,
        app: &mut CalibRawApp,
        _layout: ScreenLayout,
        _frame: &eframe::Frame,
    ) {
        let tool_help = inpaint_tool_help(app.inpaint.tool);
        moduwu_design::section_card_with_help(ui, "Tool", tool_help, |ui| {
            ui.horizontal(|ui| {
                let spacing = ui.spacing().item_spacing.x;
                let tool_width = ((ui.available_width() - spacing * 2.0) / 3.0).max(1.0);
                for tool in InpaintTool::ALL {
                    if moduwu_design::segmented_button(
                        ui,
                        tool.label(),
                        app.inpaint.tool == tool,
                        tool_width,
                    )
                    .on_hover_text(inpaint_tool_help(tool))
                    .clicked()
                    {
                        app.dispatch_action(AppAction::SelectInpaintTool(tool));
                    }
                }
            });

            if app.inpaint.tool.retouch().is_some() {
                let previous_alignment = app.inpaint.alignment;
                let alignment_help = retouch_alignment_help(app.inpaint.alignment);
                moduwu_design::form_combo_with_help(
                    ui,
                    "Source alignment",
                    "retouch-source-alignment",
                    app.inpaint.alignment.label(),
                    180.0,
                    alignment_help,
                    |ui| {
                        for alignment in RetouchAlignment::ALL {
                            ui.selectable_value(
                                &mut app.inpaint.alignment,
                                alignment,
                                alignment.label(),
                            )
                            .on_hover_text(retouch_alignment_help(alignment));
                        }
                    },
                );
                if previous_alignment != app.inpaint.alignment {
                    app.inpaint.aligned_offset = None;
                }
                if ui
                    .add_enabled_ui(!app.inpaint_processing(), |ui| {
                        moduwu_design::toggle_button(
                            ui,
                            if app.inpaint.source_pick_active {
                                "Cancel source placement"
                            } else {
                                "Set source on canvas"
                            },
                            app.inpaint.source_pick_active,
                        )
                    })
                    .inner
                    .on_hover_text("Choose where to copy from. You can also Ctrl-click (Command-click on macOS) or right-click the image.")
                    .clicked()
                {
                    app.inpaint.source_pick_active = !app.inpaint.source_pick_active;
                }
                let source_status = if app.inpaint.source_pick_active {
                    "Click or tap the image to place the source"
                } else if app.inpaint.source_point.is_some() {
                    "Source set · Paint on the image to apply"
                } else {
                    "Choose a source with Set source on canvas"
                };
                ui.label(egui::RichText::new(source_status).small().color(
                    if app.inpaint.source_point.is_some() {
                        ui.visuals().weak_text_color()
                    } else {
                        ui.visuals().warn_fg_color
                    },
                ));
            }
        });

        moduwu_design::card_gap(ui);
        moduwu_design::section_card(ui, "Brush", |ui| {
            ui.add_enabled_ui(!app.inpaint_processing(), |ui| {
                let size_help = if app.preferences.image_relative_brush_size {
                    "Brush covers the same area of the photo as you zoom."
                } else {
                    "Brush stays the same size on screen; zoom in to work on finer details."
                };
                AdjustmentSlider::new("Size", &mut app.inpaint.brush_size, 0.0025..=0.25)
                    .decimals(3)
                    .step(0.0025)
                    .hover_text(size_help)
                    .reset_to(0.055)
                    .show(ui);
                if app.inpaint.tool.retouch().is_some() {
                    let mut feather_percent = (1.0 - app.inpaint.brush_hardness) * 100.0;
                    if AdjustmentSlider::new("Feather (%)", &mut feather_percent, 0.0..=100.0)
                        .decimals(0)
                        .step(1.0)
                        .hover_text("Softness of new strokes: 0% gives a hard edge; 100% softens the whole brush.")
                        .reset_to(50.0)
                        .show(ui) {
                        app.inpaint.brush_hardness = 1.0 - feather_percent / 100.0;
                    }
                }
                let mut opacity_percent = app.inpaint.brush_opacity * 100.0;
                if AdjustmentSlider::new("Opacity (%)", &mut opacity_percent, 1.0..=100.0)
                    .decimals(0)
                    .step(1.0)
                    .hover_text("Strength of new strokes. Lower values blend in more of the original photo.")
                    .reset_to(100.0)
                    .show(ui) {
                    app.inpaint.brush_opacity = opacity_percent / 100.0;
                }
            });
            if let Some(status) = app
                .inpaint
                .processing_progress
                .as_ref()
                .filter(|_| app.inpaint.pending_retouch.is_some())
            {
                ui.add_space(moduwu_design::SPACE_SM);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new(&status.phase).small());
                });
            }
        });

        moduwu_design::card_gap(ui);
        let history_title = format!("{} stroke history", app.inpaint.tool.label());
        moduwu_design::section_card(ui, &history_title, |ui| {
            app.inpaint.hovered_stroke = None;
            let mut delete_stroke = None;
            let visible_strokes = app
                .inpaint
                .edits
                .strokes
                .iter()
                .enumerate()
                .filter_map(|(index, stroke)| {
                    app.inpaint
                        .tool
                        .matches_stroke_tool(stroke.retouch.map(|retouch| retouch.tool))
                        .then_some(index)
                })
                .collect::<Vec<_>>();
            if visible_strokes.is_empty() {
                ui.label(
                    egui::RichText::new(format!("No {} strokes yet.", app.inpaint.tool.label()))
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
            } else {
                for (history_index, index) in visible_strokes.iter().copied().enumerate().rev() {
                    moduwu_design::toolbar_row(ui, |ui| {
                        let selected = app.inpaint.selected_stroke == Some(index);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if moduwu_design::icon_button_enabled(
                                ui,
                                !app.inpaint_processing(),
                                egui_phosphor::regular::TRASH,
                                moduwu_design::toolbar_icon_size(),
                                "Delete this stroke",
                            )
                            .clicked()
                            {
                                delete_stroke = Some(index);
                            }
                            let stroke_response = moduwu_design::navigation_row(
                                ui,
                                format!("◎  Stroke {}", history_index + 1),
                                selected,
                                egui::Sense::click(),
                            )
                            .on_hover_text(
                                "Hover to see the painted area. Select a stroke to highlight it and adjust its opacity.",
                            );
                            if stroke_response.hovered() {
                                app.inpaint.hovered_stroke = Some(index);
                                ui.ctx().request_repaint();
                            }
                            if stroke_response.clicked() {
                                app.inpaint.selected_stroke =
                                    if selected { None } else { Some(index) };
                                ui.ctx().request_repaint();
                            }
                        });
                    });
                }
            }
            let selected_settings = app
                .inpaint
                .selected_stroke
                .and_then(|index| {
                    visible_strokes
                        .iter()
                        .position(|visible| *visible == index)
                        .and_then(|history_index| {
                            app.inpaint
                                .edits
                                .strokes
                                .get(index)
                                .map(|stroke| (index, history_index, stroke))
                        })
                })
                .map(|(index, history_index, stroke)| {
                    (
                        index,
                        history_index,
                        stroke.opacity,
                        stroke
                            .retouch
                            .map(|retouch| 1.0 - retouch.hardness.clamp(0.0, 1.0)),
                    )
                });
            if let Some((index, history_index, opacity, feather)) = selected_settings {
                moduwu_design::section_separator(ui);
                ui.strong(format!("Selected stroke {}", history_index + 1));
                if let Some(feather) = feather {
                    ui.label(
                        egui::RichText::new(format!("Recorded feather: {:.0}%", feather * 100.0))
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    );
                }
                let mut opacity_percent = opacity * 100.0;
                let changed = ui
                    .add_enabled_ui(!app.inpaint_processing(), |ui| {
                        AdjustmentSlider::new("Opacity (%)", &mut opacity_percent, 0.0..=100.0)
                            .decimals(0)
                            .step(1.0)
                            .hover_text("Adjust this stroke's strength. Set to 0% to hide it or 100% for full strength.")
                            .reset_to(100.0)
                            .show(ui)
                    })
                    .inner;
                if changed {
                    app.set_inpaint_stroke_opacity(index, opacity_percent / 100.0);
                }
            }
            if let Some(index) = delete_stroke {
                app.delete_inpaint_stroke(index);
            }
            if !ui.input(|input| input.pointer.any_down()) {
                app.finish_inpaint_stroke_opacity_edit();
            }

            if app.preview.gpu_pipeline.is_none() {
                ui.add_space(moduwu_design::SPACE_SM);
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Open a photo to use Remove, Clone or Heal.",
                );
            }
        });
    }
}
