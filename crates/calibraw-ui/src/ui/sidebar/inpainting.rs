fn inpaint_tool_help(tool: InpaintTool) -> &'static str {
    match tool {
        InpaintTool::Remove => {
            "Paint unwanted content. Big-LaMa repairs a native-resolution local context crop after release."
        }
        #[cfg(not(target_os = "android"))]
        InpaintTool::ComfyRemove => {
            "Paint the area to remove. ComfyUI runs Qwen Image 2.1 on a 1024 px context crop; only the painted mask is blended back."
        }
        InpaintTool::Clone => {
            "Copy pixels from a source. Ctrl-click (Command-click on macOS) or right-click the image to choose it."
        }
        InpaintTool::Heal => {
            "Copy source texture while matching the destination's color and light with GIMP-style perceptual Poisson healing."
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
        crate::ui::theme::section_card_with_help(ui, "Tool", tool_help, |ui| {
            let columns = 2;
            let spacing = ui.spacing().item_spacing.x;
            let tool_width = ((ui.available_width() - spacing) / columns as f32).max(1.0);
            for row in InpaintTool::ALL.chunks(columns) {
                ui.horizontal(|ui| {
                    for &tool in row {
                        if crate::ui::theme::segmented_button(ui, tool.label(), app.inpaint.tool == tool, tool_width)
                            .on_hover_text(inpaint_tool_help(tool)).clicked() {
                            app.dispatch_action(AppAction::SelectInpaintTool(tool));
                        }
                    }
                });
            }

            if app.inpaint.tool.retouch().is_some() {
                let previous_alignment = app.inpaint.alignment;
                let alignment_help = retouch_alignment_help(app.inpaint.alignment);
                crate::ui::theme::form_combo_with_help(
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
                        crate::ui::theme::toggle_button(
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
                    .on_hover_text("Choose the source point used by Clone or Heal strokes.")
                    .clicked()
                {
                    app.inpaint.source_pick_active = !app.inpaint.source_pick_active;
                }
                let source_status = if app.inpaint.source_pick_active {
                    "Click or tap the image to place the source"
                } else if app.inpaint.source_point.is_some() {
                    "Source set · Ctrl/right-click to move it"
                } else {
                    "Source not set · Ctrl/right-click the image"
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

        crate::ui::theme::card_gap(ui);
        crate::ui::theme::section_card(ui, "Brush", |ui| {
            ui.add_enabled_ui(!app.inpaint_processing(), |ui| {
                adjustment_slider_with_reset(
                    ui,
                    "Size",
                    &mut app.inpaint.brush_size,
                    0.0025..=0.25,
                    3,
                    0.0025,
                    Some("Brush stays the same size on screen; zoom in for a smaller, more precise native-image footprint."),
                    0.055,
                );
                if app.inpaint.tool.retouch().is_some() {
                    let mut feather = 1.0 - app.inpaint.brush_hardness;
                    if adjustment_slider_with_reset(
                        ui,
                        "Feather",
                        &mut feather,
                        0.0..=1.0,
                        2,
                        0.01,
                        Some("Width of the soft outer edge as a fraction of the brush radius."),
                        0.5,
                    ) {
                        app.inpaint.brush_hardness = 1.0 - feather;
                    }
                }
                adjustment_slider_with_reset(
                    ui,
                    "Opacity",
                    &mut app.inpaint.brush_opacity,
                    0.01..=1.0,
                    2,
                    0.01,
                    Some("Initial strength of each new Remove, Clone, or Heal stroke."),
                    1.0,
                );
            });
            if let Some(status) = app.inpaint.processing_label.as_deref() {
                ui.add_space(crate::ui::theme::SPACE_SM);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new(status).small());
                });
            }
        });

        crate::ui::theme::card_gap(ui);
        let history_title = format!("{} stroke history", app.inpaint.tool.label());
        crate::ui::theme::section_card(ui, &history_title, |ui| {
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
                        .matches_stroke_tool(stroke)
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
                    let stroke = &app.inpaint.edits.strokes[index];
                    let points = stroke.brush.points.len();
                    let patches = stroke.patches.len();
                    crate::ui::theme::toolbar_row(ui, |ui| {
                        let selected = app.inpaint.selected_stroke == Some(index);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if crate::ui::icons::phosphor_icon_button_enabled(
                                ui,
                                !app.inpaint_processing(),
                                egui_phosphor::regular::TRASH,
                                crate::ui::theme::toolbar_icon_size(),
                                "Delete this inpainting stroke",
                            )
                            .clicked()
                            {
                                delete_stroke = Some(index);
                            }
                            let stroke_response = crate::ui::theme::navigation_row(
                                ui,
                                format!(
                                    "◎  Stroke {}  ·  {} points · {} patch{}",
                                    history_index + 1,
                                    points,
                                    patches,
                                    if patches == 1 { "" } else { "es" }
                                ),
                                selected,
                                egui::Sense::click(),
                            )
                            .on_hover_text(
                                "Hover to show the stored native brush mask. Click to keep it highlighted.",
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
            if let Some((index, history_index, mut opacity, feather)) = selected_settings {
                crate::ui::theme::section_separator(ui);
                ui.strong(format!("Selected stroke {}", history_index + 1));
                if let Some(feather) = feather {
                    ui.label(
                        egui::RichText::new(format!("Recorded feather: {:.0}%", feather * 100.0))
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    );
                }
                let changed = ui
                    .add_enabled_ui(!app.inpaint_processing(), |ui| {
                        adjustment_slider_with_reset(
                            ui,
                            "Opacity",
                            &mut opacity,
                            0.0..=1.0,
                            2,
                            0.01,
                            Some("Non-destructively changes this stored stroke without rerunning its model or heal solver."),
                            1.0,
                        )
                    })
                    .inner;
                if changed {
                    app.set_inpaint_stroke_opacity(index, opacity);
                }
            }
            if let Some(index) = delete_stroke {
                app.delete_inpaint_stroke(index);
            }
            if !ui.input(|input| input.pointer.any_down()) {
                app.finish_inpaint_stroke_opacity_edit();
            }

            if app.preview.gpu_pipeline.is_none() {
                ui.add_space(crate::ui::theme::SPACE_SM);
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Open a RAW image to use inpainting tools.",
                );
            }
        });
    }
}
