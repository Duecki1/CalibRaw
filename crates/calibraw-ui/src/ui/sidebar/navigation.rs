const COMPACT_PRIMARY_PANEL_HEIGHT: f32 = 52.0;
const COMPACT_PRIMARY_TAB_HEIGHT: f32 = 48.0;
const COMPACT_CONTEXT_PANEL_HEIGHT: f32 = 48.0;
const COMPACT_CONTEXT_TAB_HEIGHT: f32 = 44.0;

fn mobile_tab_text_geometry(height: f32) -> (f32, f32, f32, f32) {
    let icon_size = (height * 0.38).clamp(19.0, 23.0);
    let label_size = if height > 54.0 { 10.5 } else { 9.5 };
    let gap = if height > 54.0 { 4.0 } else { 3.0 };
    let stack_height = icon_size + gap + label_size;
    let stack_top = (height - stack_height) * 0.5;
    let icon_center = stack_top + icon_size * 0.5;
    let label_center = stack_top + icon_size + gap + label_size * 0.5;
    (icon_size, label_size, icon_center, label_center)
}

fn mobile_tab_icon_geometry(height: f32, show_label: bool) -> (f32, f32) {
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
        ui.take_available_width();
        let vertical_spacing = if crate::ui::theme::is_compact_portrait(ui) {
            crate::ui::theme::SPACE_XS
        } else {
            crate::ui::theme::SPACE_SM
        };
        ui.spacing_mut().item_spacing = egui::vec2(crate::ui::theme::SPACE_SM, vertical_spacing);

        if layout == ScreenLayout::Vertical {
            Self::show_vertical_mobile_shell(ui, app, frame);
            return;
        }

        Self::show_sidebar_header(ui, app);
        Self::show_histogram(ui, app);
        Self::show_sidebar_content(ui, app, layout, frame);
    }

    fn show_sidebar_header(ui: &mut Ui, app: &mut CalibRawApp) {
        let title = match app.ui.sidebar_tab {
            SidebarTab::Adjustments => "Edit",
            SidebarTab::Crop => "Crop",
            SidebarTab::Masks => "Masking",
            SidebarTab::Inpainting => "Inpaint",
            SidebarTab::Export => "Export",
            SidebarTab::Info => "Image Info",
        };
        crate::ui::theme::card_header(ui, |ui| {
            let width = ui.available_width().max(1.0);
            ui.allocate_ui_with_layout(
                egui::vec2(width, crate::ui::theme::TOOLBAR_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        match app.ui.sidebar_tab {
                            SidebarTab::Adjustments => {
                                if crate::ui::icons::phosphor_icon_button(
                                    ui,
                                    egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                                    crate::ui::theme::toolbar_icon_size(),
                                    "Reset all develop adjustments",
                                )
                                .clicked()
                                {
                                    app.reset_develop_adjustments();
                                }
                            }
                            SidebarTab::Crop => {
                                if crate::ui::icons::phosphor_icon_button(
                                    ui,
                                    egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                                    crate::ui::theme::toolbar_icon_size(),
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
                                if crate::ui::icons::phosphor_icon_button_enabled(
                                    ui,
                                    active_stroke_count != 0 && !app.inpaint_processing(),
                                    egui_phosphor::regular::TRASH,
                                    crate::ui::theme::toolbar_icon_size(),
                                    &format!("Clear all {} strokes", active_tool.label()),
                                )
                                .clicked()
                                {
                                    app.clear_inpainting_tool();
                                }
                            }
                            SidebarTab::Masks => {
                                if crate::ui::icons::phosphor_icon_button(
                                    ui,
                                    egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                                    crate::ui::theme::toolbar_icon_size(),
                                    "Reset all masks and clear the subject mask cache",
                                )
                                .clicked()
                                {
                                    app.reset_masks();
                                }
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
                                        .size(crate::ui::theme::PANEL_TITLE_TEXT_SIZE),
                                )
                                .truncate(),
                            )
                            .on_hover_text(title);
                        });
                    });
                },
            );
        });
        crate::ui::theme::card_gap(ui);
    }

    fn show_vertical_mobile_shell(ui: &mut Ui, app: &mut CalibRawApp, frame: &eframe::Frame) {
        let compact = crate::ui::theme::is_compact_portrait(ui);
        egui::Panel::bottom("develop_portrait_primary_tabs")
            .resizable(false)
            .show_separator_line(false)
            .exact_size(if compact {
                COMPACT_PRIMARY_PANEL_HEIGHT
            } else {
                62.0
            })
            .frame(Self::mobile_navigation_frame(ui))
            .show(ui, |ui| Self::show_mobile_primary_tabs(ui, app));

        if matches!(
            app.ui.sidebar_tab,
            SidebarTab::Adjustments | SidebarTab::Masks
        ) {
            egui::Panel::bottom("develop_portrait_context_tabs")
                .resizable(false)
                .show_separator_line(false)
                .exact_size(if compact {
                    COMPACT_CONTEXT_PANEL_HEIGHT
                } else {
                    58.0
                })
                .frame(Self::mobile_navigation_frame(ui))
                .show(ui, |ui| Self::show_mobile_context_tabs(ui, app));
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(egui::Margin::same(0)))
            .show(ui, |ui| Self::show_sidebar_content(ui, app, ScreenLayout::Vertical, frame));
    }

    fn mobile_navigation_frame(ui: &Ui) -> egui::Frame {
        egui::Frame::new()
            .fill(ui.visuals().panel_fill)
            .inner_margin(egui::Margin::symmetric(0, 2))
            .stroke(egui::Stroke::NONE)
    }

    fn paint_mobile_navigation_separator(ui: &Ui) {
        let rect = ui.max_rect();
        let stroke = egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color);
        ui.painter().hline(rect.x_range(), rect.top(), stroke);
    }

    fn show_mobile_primary_tabs(ui: &mut Ui, app: &mut CalibRawApp) {
        use egui_phosphor::regular;

        // Treat the two portrait navigation rows as one surface. The context
        // row paints the outer divider when it is present; otherwise the
        // primary row still needs to separate itself from the content.
        if !matches!(
            app.ui.sidebar_tab,
            SidebarTab::Adjustments | SidebarTab::Masks
        ) {
            Self::paint_mobile_navigation_separator(ui);
        }
        ui.spacing_mut().item_spacing.x = 0.0;
        let show_labels = app.preferences.show_develop_navigation_labels;
        let tab_height = if crate::ui::theme::is_compact_portrait(ui) {
            COMPACT_PRIMARY_TAB_HEIGHT
        } else {
            56.0
        };
        let item_width = (ui.available_width() / 6.0).max(1.0);
        ui.horizontal(|ui| {
            for (tab, icon, label, tooltip) in [
                (
                    SidebarTab::Adjustments,
                    regular::SLIDERS_HORIZONTAL,
                    "Edit",
                    "Edit adjustments",
                ),
                (
                    SidebarTab::Crop,
                    regular::CROP,
                    "Crop",
                    "Crop",
                ),
                (SidebarTab::Masks, regular::SELECTION, "Mask", "Masking"),
                (
                    SidebarTab::Inpainting,
                    regular::BANDAIDS,
                    "Remove",
                    "Remove unwanted objects",
                ),
                (SidebarTab::Export, regular::EXPORT, "Export", "Export"),
                (SidebarTab::Info, regular::INFO, "Info", "Image information"),
            ] {
                if Self::mobile_icon_tab(
                    ui,
                    icon,
                    label,
                    show_labels,
                    app.ui.sidebar_tab == tab,
                    egui::vec2(item_width, tab_height),
                    tooltip,
                )
                .clicked()
                {
                    app.dispatch_action(AppAction::SelectSidebarTab(tab));
                }
            }
        });
    }

    fn show_mobile_context_tabs(ui: &mut Ui, app: &mut CalibRawApp) {
        Self::paint_mobile_navigation_separator(ui);
        let compact = crate::ui::theme::is_compact_portrait(ui);
        let tab_height = if compact {
            COMPACT_CONTEXT_TAB_HEIGHT
        } else {
            52.0
        };
        let tabs_width = ui.available_width().max(1.0);

        ui.spacing_mut().item_spacing.x = 0.0;
        ui.allocate_ui_with_layout(
            egui::vec2(tabs_width, tab_height),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_width(tabs_width);
                egui::ScrollArea::horizontal()
                    .id_salt("develop-portrait-context-tabs")
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
                        Self::show_mobile_context_tab_items(ui, app, tab_height)
                    });
            },
        );
    }

    fn show_mobile_context_tab_items(ui: &mut Ui, app: &mut CalibRawApp, tab_height: f32) {
        use egui_phosphor::regular;

        let show_labels = app.preferences.show_develop_navigation_labels;
        ui.spacing_mut().item_spacing.x = 1.0;
        ui.horizontal(|ui| match app.ui.sidebar_tab {
            SidebarTab::Adjustments => {
                for (section, icon, label) in [
                    (AdjustmentSection::Light, regular::SUN, "Light"),
                    (AdjustmentSection::ToneCurve, regular::WAVE_SINE, "Curve"),
                    (AdjustmentSection::Color, regular::DROP, "Color"),
                    (
                        AdjustmentSection::ColorGrading,
                        regular::CIRCLES_THREE,
                        "Grading",
                    ),
                    (AdjustmentSection::Detail, regular::APERTURE, "Detail"),
                    (AdjustmentSection::Effects, regular::SPARKLE, "Effects"),
                    (AdjustmentSection::ColorMixer, regular::SWATCHES, "Mixer"),
                    (AdjustmentSection::Optics, regular::EYE, "Optics"),
                ] {
                    if Self::mobile_icon_tab(
                        ui,
                        icon,
                        label,
                        show_labels,
                        app.develop_ui.adjustment_section == section,
                        egui::vec2(Self::CONTEXT_TAB_WIDTH, tab_height),
                        label,
                    )
                    .clicked()
                    {
                        app.develop_ui.adjustment_section = section;
                        if section != AdjustmentSection::Color {
                            app.develop_ui.white_balance_picker_active = false;
                            app.develop_ui.white_balance_picker_drag = None;
                        }
                    }
                }
            }
            SidebarTab::Masks => {
                let adjustment_mask = app
                    .masks
                    .stack
                    .selected_mask()
                    .is_none_or(|mask| mask.effect.uses_adjustments());
                for (section, icon, label) in [
                    (MaskSection::Properties, regular::SELECTION, "Mask"),
                    (MaskSection::Light, regular::SUN, "Light"),
                    (MaskSection::ToneCurve, regular::WAVE_SINE, "Curve"),
                    (MaskSection::Color, regular::DROP, "Color"),
                    (MaskSection::ColorGrading, regular::CIRCLES_THREE, "Grading"),
                    (MaskSection::Effects, regular::SPARKLE, "Effects"),
                    (MaskSection::ColorMixer, regular::SWATCHES, "Mixer"),
                ] {
                    if section != MaskSection::Properties && !adjustment_mask {
                        continue;
                    }
                    if Self::mobile_icon_tab(
                        ui,
                        icon,
                        label,
                        show_labels,
                        app.develop_ui.mask_section == section,
                        egui::vec2(Self::CONTEXT_TAB_WIDTH, tab_height),
                        label,
                    )
                    .clicked()
                    {
                        app.develop_ui.mask_section = section;
                    }
                }
            }
            SidebarTab::Crop | SidebarTab::Inpainting | SidebarTab::Export | SidebarTab::Info => {}
        });
    }

    fn mobile_icon_tab(
        ui: &mut Ui,
        icon: &str,
        label: &str,
        show_label: bool,
        selected: bool,
        size: egui::Vec2,
        tooltip: &str,
    ) -> egui::Response {
        use egui::{Align2, FontId, Sense};

        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let painter = ui.painter_at(rect);
        let interaction = crate::ui::theme::interaction_visuals(ui, &response, selected);
        let tile_width = if size.y > 54.0 { 56.0 } else { 50.0 };
        let tile = egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(size.x.min(tile_width), size.y - 4.0),
        );
        if interaction.state != crate::ui::theme::InteractionVisualState::Inactive {
            painter.rect_filled(tile, 6.0, interaction.weak_fill);
        }

        let color = if interaction.state == crate::ui::theme::InteractionVisualState::Inactive {
            ui.visuals().weak_text_color()
        } else {
            interaction.foreground
        };
        let (icon_size, icon_center) = mobile_tab_icon_geometry(size.y, show_label);
        painter.text(
            egui::pos2(rect.center().x, rect.top() + icon_center),
            Align2::CENTER_CENTER,
            icon,
            FontId::proportional(icon_size),
            color,
        );
        if show_label {
            let (_, label_size, _, label_center) = mobile_tab_text_geometry(size.y);
            painter.text(
                egui::pos2(rect.center().x, rect.top() + label_center),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(label_size),
                color,
            );
        }
        response.on_hover_text(tooltip)
    }

    fn show_sidebar_content(
        ui: &mut Ui,
        app: &mut CalibRawApp,
        layout: ScreenLayout,
        frame: &eframe::Frame,
    ) {
        let sidebar_scroll_source = if slider_scroll_locked(ui.ctx()) {
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
                show_export_action_panel(ui, |ui| Self::show_export_action(ui, app, frame));
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
                            ui.add_space(crate::ui::theme::SPACE_SM);
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

    fn show_mobile_footer_actions(ui: &mut Ui, app: &mut CalibRawApp) {
        ui.add_space(crate::ui::theme::SPACE_XS);
        let width = ui.available_width().max(1.0);
        ui.allocate_ui_with_layout(
            egui::vec2(width, crate::ui::theme::TOOLBAR_HEIGHT),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                match app.ui.sidebar_tab {
                    // Intentionally no global reset in Edit on mobile.
                    SidebarTab::Adjustments => {}
                    SidebarTab::Crop => {
                        if crate::ui::icons::phosphor_icon_button(
                            ui,
                            egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                            crate::ui::theme::toolbar_icon_size(),
                            "Reset crop and geometry",
                        )
                        .clicked()
                        {
                            Self::reset_crop(app);
                        }
                    }
                    SidebarTab::Masks => {
                        if crate::ui::icons::phosphor_icon_button(
                            ui,
                            egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                            crate::ui::theme::toolbar_icon_size(),
                            "Reset all masks and clear the subject mask cache",
                        )
                        .clicked()
                        {
                            app.reset_masks();
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
                        if crate::ui::icons::phosphor_icon_button_enabled(
                            ui,
                            active_stroke_count != 0 && !app.inpaint_processing(),
                            egui_phosphor::regular::TRASH,
                            crate::ui::theme::toolbar_icon_size(),
                            &format!("Clear all {} strokes", active_tool.label()),
                        )
                        .clicked()
                        {
                            app.clear_inpainting_tool();
                        }
                    }
                    SidebarTab::Export | SidebarTab::Info => {}
                }
                Self::show_vertical_card_footer_actions(ui);
                Self::show_histogram_toggle(ui, app);
                Self::show_clipping_toggles(ui, app);
            },
        );
    }

    #[cfg(not(target_os = "android"))]
    pub(crate) fn show_desktop_tool_rail(ui: &mut Ui, app: &mut CalibRawApp) {
        use crate::ui::icons::{icon_toggle_button, UiIcon};

        ui.set_min_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = crate::ui::theme::SPACE_XS;
        ui.vertical_centered(|ui| {
            ui.add_space(5.0);
            for (tab, icon, tooltip) in [
                (SidebarTab::Adjustments, UiIcon::Adjustments, "Edit"),
                (SidebarTab::Crop, UiIcon::Crop, "Crop"),
                (SidebarTab::Masks, UiIcon::Mask, "Masking"),
                (
                    SidebarTab::Inpainting,
                    UiIcon::Heal,
                    "Remove unwanted objects",
                ),
                (SidebarTab::Export, UiIcon::Export, "Export"),
                (SidebarTab::Info, UiIcon::Info, "Image information"),
            ] {
                if icon_toggle_button(
                    ui,
                    icon,
                    app.ui.sidebar_tab == tab,
                    crate::ui::theme::tool_rail_icon_size(),
                    tooltip,
                )
                .clicked()
                {
                    app.dispatch_action(AppAction::SelectSidebarTab(tab));
                    app.develop_ui.sidebar_open = true;
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
                crate::ui::theme::tool_rail_icon_size(),
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
                crate::ui::theme::tool_rail_icon_size(),
                sidebar_tooltip,
            )
            .clicked()
            {
                app.develop_ui.sidebar_open = !app.develop_ui.sidebar_open;
            }
        });
    }

    #[cfg(target_os = "android")]
    pub(crate) fn show_android_landscape_primary_tabs(ui: &mut Ui, app: &mut CalibRawApp) {
        use egui_phosphor::regular;

        ui.set_width(Self::ANDROID_LANDSCAPE_TOOL_RAIL_WIDTH);
        ui.spacing_mut().item_spacing.y = 0.0;
        let show_labels = app.preferences.show_develop_navigation_labels;
        ui.vertical_centered(|ui| {
            for (tab, icon, label, tooltip) in [
                (
                    SidebarTab::Adjustments,
                    regular::SLIDERS_HORIZONTAL,
                    "Edit",
                    "Edit adjustments",
                ),
                (
                    SidebarTab::Crop,
                    regular::CROP,
                    "Crop",
                    "Crop",
                ),
                (SidebarTab::Masks, regular::SELECTION, "Mask", "Masking"),
                (
                    SidebarTab::Inpainting,
                    regular::BANDAIDS,
                    "Remove",
                    "Remove unwanted objects",
                ),
                (SidebarTab::Export, regular::EXPORT, "Export", "Export"),
                (SidebarTab::Info, regular::INFO, "Info", "Image information"),
            ] {
                if Self::mobile_icon_tab(
                    ui,
                    icon,
                    label,
                    show_labels,
                    app.ui.sidebar_tab == tab,
                    egui::vec2(56.0, 56.0),
                    tooltip,
                )
                .clicked()
                {
                    app.dispatch_action(AppAction::SelectSidebarTab(tab));
                }
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
                    changed |= Self::show_presence(ui, &mut app.develop.exposure, false);
                }
                AdjustmentSection::ColorMixer => {
                    changed |= Self::show_hsl(
                        ui,
                        &mut app.develop.exposure,
                        &mut app.develop_ui.hsl_mixer_color,
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
                true,
            );
            lens_changed |= Self::show_optics(ui, app, true);
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

        crate::ui::theme::section_card(ui, "Camera profile", |ui| {
            crate::ui::theme::form_combo(
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
        crate::ui::theme::card_gap(ui);

        if selection != previous {
            app.select_camera_profile_for_current(selection, frame);
        }
    }
}
