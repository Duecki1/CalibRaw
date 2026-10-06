use super::*;

mod mobile;

const COMPACT_PRIMARY_PANEL_HEIGHT: f32 = 52.0;
const COMPACT_PRIMARY_TAB_HEIGHT: f32 = 48.0;
const COMPACT_CONTEXT_PANEL_HEIGHT: f32 = 48.0;
const COMPACT_CONTEXT_TAB_HEIGHT: f32 = 44.0;

pub(super) fn mobile_tab_text_geometry(height: f32) -> (f32, f32, f32, f32) {
    let icon_size = (height * 0.38).clamp(19.0, 23.0);
    let label_size = if height > 54.0 { 10.5 } else { 9.5 };
    let gap = if height > 54.0 { 4.0 } else { 3.0 };
    let stack_height = icon_size + gap + label_size;
    let stack_top = (height - stack_height) * 0.5;
    let icon_center = stack_top + icon_size * 0.5;
    let label_center = stack_top + icon_size + gap + label_size * 0.5;
    (icon_size, label_size, icon_center, label_center)
}

pub(super) fn mobile_tab_icon_geometry(height: f32, show_label: bool) -> (f32, f32) {
    if show_label {
        let (icon_size, _, icon_center, _) = mobile_tab_text_geometry(height);
        (icon_size, icon_center)
    } else {
        ((height * 0.5).clamp(21.0, 25.0), height * 0.5)
    }
}

impl Sidebar {
    pub(crate) fn show(
        ui: &mut Ui,
        app: &mut CalibRawApp,
        layout: ScreenLayout,
        frame: &eframe::Frame,
    ) {
        Self::requesting_new_content(app, frame, |app| {
            Self::show_contents(ui, app, layout, frame);
        });
    }

    fn show_contents(
        ui: &mut Ui,
        app: &mut CalibRawApp,
        layout: ScreenLayout,
        frame: &eframe::Frame,
    ) {
        ui.take_available_width();
        let vertical_spacing = if moduwu_design::is_compact_portrait(ui) {
            moduwu_design::SPACE_XS
        } else {
            moduwu_design::SPACE_SM
        };
        ui.spacing_mut().item_spacing = egui::vec2(moduwu_design::SPACE_SM, vertical_spacing);

        if layout == ScreenLayout::Vertical {
            Self::show_vertical_mobile_shell(ui, app, frame);
            return;
        }

        Self::show_sidebar_header(ui, app);
        Self::show_histogram(ui, app);
        Self::show_sidebar_content(ui, app, layout, frame);
    }

    fn show_sidebar_header(ui: &mut Ui, app: &mut CalibRawApp) {
        let title = app.ui.sidebar_tab.title();
        moduwu_design::card_header(ui, |ui| {
            let width = ui.available_width().max(1.0);
            ui.allocate_ui_with_layout(
                egui::vec2(width, moduwu_design::TOOLBAR_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        match app.ui.sidebar_tab {
                            SidebarTab::Adjustments => {
                                if moduwu_design::icon_button(
                                    ui,
                                    egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                                    moduwu_design::toolbar_icon_size(),
                                    "Reset all develop adjustments",
                                )
                                .clicked()
                                {
                                    app.reset_develop_adjustments();
                                }
                            }
                            SidebarTab::Crop => {
                                if moduwu_design::icon_button(
                                    ui,
                                    egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                                    moduwu_design::toolbar_icon_size(),
                                    "Reset crop and geometry",
                                )
                                .clicked()
                                {
                                    Self::reset_crop(app);
                                }
                            }
                            SidebarTab::Inpainting => {
                                let active_tool = app.inpaint.tool;
                                let active_stroke_count = app
                                    .inpaint
                                    .edits
                                    .strokes
                                    .iter()
                                    .filter(|stroke| {
                                        active_tool.matches_stroke_tool(
                                            stroke.retouch.map(|retouch| retouch.tool),
                                        )
                                    })
                                    .count();
                                if moduwu_design::icon_button_enabled(
                                    ui,
                                    active_stroke_count != 0 && !app.inpaint_processing(),
                                    egui_phosphor::regular::TRASH,
                                    moduwu_design::toolbar_icon_size(),
                                    &format!("Clear all {} strokes", active_tool.label()),
                                )
                                .clicked()
                                {
                                    app.clear_inpainting_tool();
                                }
                            }
                            SidebarTab::Masks => {
                                if moduwu_design::icon_button(
                                    ui,
                                    egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                                    moduwu_design::toolbar_icon_size(),
                                    "Reset all masks and clear the subject mask cache",
                                )
                                .clicked()
                                {
                                    app.reset_masks();
                                }
                            }
                            SidebarTab::Presets => {
                                #[cfg(not(target_os = "android"))]
                                crate::ui::presets::show_header_actions(ui, app);
                            }
                            SidebarTab::Export | SidebarTab::Info => {}
                        }
                        Self::show_histogram_toggle(ui, app);
                        Self::show_clipping_toggles(ui, app);
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(title)
                                        .strong()
                                        .size(moduwu_design::PANEL_TITLE_TEXT_SIZE),
                                )
                                .truncate(),
                            )
                            .on_hover_text(title);
                        });
                    });
                },
            );
        });
        moduwu_design::card_gap(ui);
    }

    fn show_sidebar_content(
        ui: &mut Ui,
        app: &mut CalibRawApp,
        layout: ScreenLayout,
        frame: &eframe::Frame,
    ) {
        let sidebar_scroll_source = if moduwu_design::slider_scroll_locked(ui.ctx()) {
            egui::scroll_area::ScrollSource::NONE
        } else {
            egui::scroll_area::ScrollSource::default()
        };
        ui.scope(|ui| {
            if layout == ScreenLayout::Vertical {
                Self::begin_vertical_card_actions(ui.ctx());
            }

            let mut scroll_style = egui::style::ScrollStyle::solid();
            scroll_style.bar_width = 7.0;
            scroll_style.bar_inner_margin = 7.0;
            ui.spacing_mut().scroll = scroll_style;

            if app.ui.sidebar_tab == SidebarTab::Export {
                super::export::show_export_action_panel(ui, |ui| {
                    Self::show_export_action(ui, app, frame)
                });
            }

            let mut mask_edit_header_rect = None;
            let scroll_output = egui::ScrollArea::vertical()
                .id_salt("develop-sidebar-content")
                .scroll_source(sidebar_scroll_source)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let content_width = ui.available_width().max(1.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(content_width, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(content_width);
                            ui.set_max_width(content_width);
                            if layout == ScreenLayout::Vertical {
                                Self::show_histogram(ui, app);
                            }
                            match app.ui.sidebar_tab {
                                SidebarTab::Adjustments => {
                                    Self::show_adjustments(ui, app, layout, frame)
                                }
                                SidebarTab::Presets => {
                                    crate::ui::presets::show_panel(ui, app, frame)
                                }
                                SidebarTab::Crop => Self::show_crop(ui, app, layout),
                                SidebarTab::Masks => {
                                    mask_edit_header_rect = Self::show_masks(ui, app, layout, frame)
                                }
                                SidebarTab::Inpainting => {
                                    Self::show_inpainting(ui, app, layout, frame)
                                }
                                SidebarTab::Export => Self::show_export(ui, app, frame),
                                SidebarTab::Info => Self::show_info(ui, app),
                            }
                            if layout == ScreenLayout::Vertical {
                                Self::show_mobile_footer_actions(ui, app);
                            }
                            ui.add_space(moduwu_design::SPACE_SM);
                        },
                    );
                });

            if layout == ScreenLayout::Horizontal && app.ui.sidebar_tab == SidebarTab::Masks {
                if let Some(header_rect) = mask_edit_header_rect {
                    Self::show_sticky_mask_edit_header(
                        ui.ctx(),
                        app,
                        header_rect,
                        scroll_output.inner_rect,
                    );
                }
            }
        });
    }

    #[cfg(not(target_os = "android"))]
    pub(crate) fn show_desktop_tool_rail(ui: &mut Ui, app: &mut CalibRawApp) {
        use crate::ui::icons::{glyph_toggle_button, icon_toggle_button, UiIcon};

        ui.set_min_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = moduwu_design::SPACE_XS;
        ui.vertical_centered(|ui| {
            ui.add_space(5.0);
            for tab in SidebarTab::ALL {
                if glyph_toggle_button(
                    ui,
                    tab.glyph(),
                    app.ui.sidebar_tab == tab,
                    moduwu_design::tool_rail_icon_size(),
                    tab.tooltip(),
                )
                .clicked()
                {
                    app.dispatch_action(AppAction::SelectSidebarTab(tab));
                }
            }
        });

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            ui.add_space(5.0);

            let filmstrip_tooltip = if app.develop_ui.filmstrip_open {
                "Hide filmstrip"
            } else {
                "Show filmstrip"
            };
            if icon_toggle_button(
                ui,
                UiIcon::Filmstrip,
                app.develop_ui.filmstrip_open,
                moduwu_design::tool_rail_icon_size(),
                filmstrip_tooltip,
            )
            .clicked()
            {
                app.set_develop_filmstrip_open(!app.develop_ui.filmstrip_open);
            }

            let sidebar_tooltip = if app.develop_ui.sidebar_open {
                "Hide editing sidebar"
            } else {
                "Show editing sidebar"
            };
            if icon_toggle_button(
                ui,
                UiIcon::Sidebar,
                app.develop_ui.sidebar_open,
                moduwu_design::tool_rail_icon_size(),
                sidebar_tooltip,
            )
            .clicked()
            {
                app.develop_ui.sidebar_open = !app.develop_ui.sidebar_open;
            }
        });
    }

    fn show_adjustments(
        ui: &mut Ui,
        app: &mut CalibRawApp,
        layout: ScreenLayout,
        frame: &eframe::Frame,
    ) {
        Self::show_camera_profile_selector(ui, app, frame);

        let mut changed = false;
        let mut lens_changed = false;
        let mut ai_denoise_request = None;
        let white_balance_raw = app.develop.loaded_raw.clone();
        let white_balance_was_active = app.develop_ui.white_balance_picker_active;
        if layout == ScreenLayout::Vertical {
            match app.develop_ui.adjustment_section {
                AdjustmentSection::Light => {
                    changed |= Self::show_basic(ui, &mut app.develop.exposure, false);
                }
                AdjustmentSection::ToneCurve => {
                    changed |= Self::show_tone_curve(
                        ui,
                        &mut app.develop.exposure,
                        &mut app.develop_ui.tone_curve_tab,
                        false,
                    );
                }
                AdjustmentSection::Color => {
                    changed |= Self::show_color(
                        ui,
                        &mut app.develop.exposure,
                        white_balance_raw.as_deref(),
                        &mut app.develop_ui.white_balance_picker_active,
                        false,
                    );
                }
                AdjustmentSection::ColorGrading => {
                    changed |= Self::show_color_grading(
                        ui,
                        &mut app.develop.exposure,
                        &mut app.develop_ui.color_grade_tab,
                        false,
                    );
                }
                AdjustmentSection::Detail => {
                    let (detail_changed, request) =
                        Self::show_detail(ui, &mut app.develop.exposure, false);
                    changed |= detail_changed;
                    ai_denoise_request = request;
                }
                AdjustmentSection::Effects => {
                    if app.develop_ui.effect_component.is_none() {
                        changed |= Self::show_presence(ui, &mut app.develop.exposure, false);
                    }
                }
                AdjustmentSection::ColorMixer => {
                    changed |= Self::show_hsl(
                        ui,
                        &mut app.develop.exposure,
                        &mut app.develop_ui.hsl_mixer_color,
                        &mut app.develop_ui.point_color,
                        &mut app.develop_ui.point_color_tab,
                        false,
                    );
                }
                AdjustmentSection::Optics => {
                    lens_changed |= Self::show_optics(ui, app, false);
                }
            }
        } else {
            changed |= Self::show_basic(ui, &mut app.develop.exposure, true);
            changed |= Self::show_tone_curve(
                ui,
                &mut app.develop.exposure,
                &mut app.develop_ui.tone_curve_tab,
                true,
            );
            changed |= Self::show_color(
                ui,
                &mut app.develop.exposure,
                white_balance_raw.as_deref(),
                &mut app.develop_ui.white_balance_picker_active,
                true,
            );
            changed |= Self::show_color_grading(
                ui,
                &mut app.develop.exposure,
                &mut app.develop_ui.color_grade_tab,
                true,
            );
            let (detail_changed, request) = Self::show_detail(ui, &mut app.develop.exposure, true);
            changed |= detail_changed;
            ai_denoise_request = request;
            changed |= Self::show_presence(ui, &mut app.develop.exposure, true);
            changed |= Self::show_hsl(
                ui,
                &mut app.develop.exposure,
                &mut app.develop_ui.hsl_mixer_color,
                &mut app.develop_ui.point_color,
                &mut app.develop_ui.point_color_tab,
                true,
            );
            lens_changed |= Self::show_optics(ui, app, true);
        }

        let effect_frame = mask_effects::EffectFrame::of(app);
        let effects_shown = layout != ScreenLayout::Vertical
            || app.develop_ui.adjustment_section == AdjustmentSection::Effects;
        if effects_shown && app.masks.stack.has_depth_fog_effect() {
            Self::show_ai_update_card(ui, app, frame);
        }
        if layout == ScreenLayout::Vertical {
            if app.develop_ui.adjustment_section == AdjustmentSection::Effects
                && Self::show_selected_effect_component(
                    ui,
                    &mut app.masks.stack.global_effects,
                    &mut app.develop_ui.effect_component,
                    true,
                    &effect_frame,
                )
            {
                app.mark_mask_adjustments_dirty();
            }
        } else if Self::show_effect_components(
            ui,
            &mut app.masks.stack.global_effects,
            true,
            &effect_frame,
        ) {
            app.mark_mask_adjustments_dirty();
        }

        if !white_balance_was_active && app.develop_ui.white_balance_picker_active {
            app.develop_ui.point_color.picker_active = false;
        }
        if app.develop_ui.point_color.picker_active {
            app.develop_ui.cancel_white_balance_picker();
        }
        if changed {
            app.develop.exposure.sanitize_tone_curves();
            app.mark_pipeline_dirty();
        }
        if lens_changed {
            app.mark_lens_correction_dirty();
        }
        if let Some(enabled) = ai_denoise_request {
            app.set_ai_denoise_enabled(enabled, frame);
        }
    }

    fn show_camera_profile_selector(ui: &mut Ui, app: &mut CalibRawApp, frame: &eframe::Frame) {
        if app.preferences.camera_profile_mode == crate::pipeline::CameraProfileMode::MatrixOnly {
            return;
        }
        let Some(raw) = app.develop.loaded_raw.as_ref() else {
            return;
        };
        let candidates = raw.available_camera_profiles.clone();
        if candidates.is_empty() {
            return;
        }
        let active_source = raw.camera_profile_source.clone();
        let active_name = active_source
            .as_ref()
            .and_then(|active| {
                candidates
                    .iter()
                    .find(|candidate| candidate.path == *active)
                    .map(|candidate| candidate.name.clone())
            })
            .or_else(|| raw.camera_profile.name.clone())
            .unwrap_or_else(|| "Embedded Matrix".to_owned());

        let previous = app.develop.selected_camera_profile.clone();
        let mut selection = previous.clone();
        let embedded_matrix_selected = previous
            .as_ref()
            .zip(app.preferences.camera_profile_folder.as_ref())
            .is_some_and(|(selected, root)| selected == root);
        let selected_text = embedded_matrix_selected
            .then_some("Embedded Matrix".to_owned())
            .or_else(|| {
                previous.as_ref().and_then(|selected| {
                    candidates
                        .iter()
                        .find(|candidate| candidate.path == *selected)
                        .map(|candidate| candidate.name.clone())
                })
            })
            .unwrap_or_else(|| format!("Automatic — {active_name}"));

        moduwu_design::section_card(ui, "Camera profile", |ui| {
            moduwu_design::form_combo(
                ui,
                "Profile",
                "current-image-camera-profile",
                selected_text,
                240.0,
                |ui| {
                    ui.selectable_value(&mut selection, None, "Automatic (recommended)")
                        .on_hover_text("Use the RAW's embedded camera matrix by default.");
                    if let Some(root) = app.preferences.camera_profile_folder.as_ref() {
                        ui.selectable_value(&mut selection, Some(root.clone()), "Embedded Matrix")
                            .on_hover_text(
                                "Use the RAW's embedded camera matrix without a DCP profile.",
                            );
                    }
                    ui.separator();
                    for candidate in &candidates {
                        ui.selectable_value(
                            &mut selection,
                            Some(candidate.path.clone()),
                            &candidate.name,
                        )
                        .on_hover_text(candidate.path.display().to_string());
                    }
                },
            );
        });
        moduwu_design::card_gap(ui);

        if selection != previous {
            app.select_camera_profile_for_current(selection, frame);
        }
    }
}
