//! Touch navigation: the portrait shell, primary/context/effect tabs and footer actions.

use super::*;

impl Sidebar {
    pub(super) fn show_vertical_mobile_shell(
        ui: &mut Ui,
        app: &mut CalibRawApp,
        frame: &eframe::Frame,
    ) {
        let compact = moduwu_design::is_compact_portrait(ui);
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
            .show(ui, |ui| {
                Self::show_sidebar_content(ui, app, ScreenLayout::Vertical, frame)
            });
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
        let tab_height = if moduwu_design::is_compact_portrait(ui) {
            COMPACT_PRIMARY_TAB_HEIGHT
        } else {
            56.0
        };
        let item_width = (ui.available_width() / SidebarTab::ALL.len() as f32).max(1.0);
        ui.horizontal(|ui| {
            for tab in SidebarTab::ALL {
                if Self::mobile_icon_tab(
                    ui,
                    tab.glyph(),
                    tab.short_label(),
                    show_labels,
                    app.ui.sidebar_tab == tab,
                    egui::vec2(item_width, tab_height),
                    tab.tooltip(),
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
        let compact = moduwu_design::is_compact_portrait(ui);
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
                        app.develop_ui.adjustment_section == section
                            && (section != AdjustmentSection::Effects
                                || app.develop_ui.effect_component.is_none()),
                        egui::vec2(Self::CONTEXT_TAB_WIDTH, tab_height),
                        label,
                    )
                    .clicked()
                    {
                        app.develop_ui.adjustment_section = section;
                        app.develop_ui.effect_component = None;
                        if section != AdjustmentSection::ColorMixer {
                            app.develop_ui.cancel_point_color_preview();
                        }
                        if section != AdjustmentSection::Color {
                            app.develop_ui.cancel_white_balance_picker();
                        }
                    }
                }
                let selected_before = app.develop_ui.effect_component;
                let added = Self::show_mobile_effect_tabs(
                    ui,
                    &mut app.masks.stack.global_effects,
                    &mut app.develop_ui.effect_component,
                    tab_height,
                    show_labels,
                );
                if added || selected_before != app.develop_ui.effect_component {
                    app.develop_ui.cancel_point_color_preview();
                    app.develop_ui.cancel_white_balance_picker();
                }
                if added {
                    app.develop_ui.adjustment_section = AdjustmentSection::Effects;
                    app.mark_mask_adjustments_dirty();
                } else if app.develop_ui.effect_component.is_some() {
                    // The selected component tab shares the Effects content section.
                    app.develop_ui.adjustment_section = AdjustmentSection::Effects;
                }
            }
            SidebarTab::Masks => {
                if app.develop_ui.mask_effect_mask != app.masks.stack.selected_mask {
                    app.develop_ui.mask_effect_mask = app.masks.stack.selected_mask;
                    app.develop_ui.mask_effect_component = None;
                }
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
                        app.develop_ui.mask_section == section
                            && (section != MaskSection::Effects
                                || app.develop_ui.mask_effect_component.is_none()),
                        egui::vec2(Self::CONTEXT_TAB_WIDTH, tab_height),
                        label,
                    )
                    .clicked()
                    {
                        app.develop_ui.mask_section = section;
                        app.develop_ui.mask_effect_component = None;
                        if section != MaskSection::ColorMixer {
                            app.cancel_mask_point_color_preview();
                        }
                    }
                }
                if let Some(mask_index) = app.masks.stack.selected_mask {
                    let mask = &mut app.masks.stack.masks[mask_index];
                    let light_rays_before = mask.has_light_rays_effect();
                    let selected_before = app.develop_ui.mask_effect_component;
                    let added = Self::show_mobile_effect_tabs(
                        ui,
                        &mut mask.effect_components,
                        &mut app.develop_ui.mask_effect_component,
                        tab_height,
                        show_labels,
                    );
                    if added || selected_before != app.develop_ui.mask_effect_component {
                        app.cancel_mask_point_color_preview();
                    }
                    if added {
                        app.develop_ui.mask_section = MaskSection::Effects;
                        let mask = &app.masks.stack.masks[mask_index];
                        if light_rays_before != mask.has_light_rays_effect() {
                            app.mark_mask_geometry_dirty(mask_index);
                        } else {
                            app.mark_mask_adjustments_dirty();
                        }
                    } else if app.develop_ui.mask_effect_component.is_some() {
                        app.develop_ui.mask_section = MaskSection::Effects;
                    }
                }
            }
            SidebarTab::Presets
            | SidebarTab::Crop
            | SidebarTab::Inpainting
            | SidebarTab::Export
            | SidebarTab::Info => {}
        });
    }

    pub(in crate::ui::sidebar) fn show_mobile_effect_tabs(
        ui: &mut Ui,
        components: &mut Vec<crate::pipeline::EffectComponent>,
        selection: &mut Option<MaskEffect>,
        tab_height: f32,
        show_labels: bool,
    ) -> bool {
        use egui_phosphor::regular;

        let mut selected_tab = None;
        for component in components.iter() {
            let effect = component.effect;
            let clicked = ui
                .push_id(("effect-component-tab", effect.label()), |ui| {
                    Self::mobile_icon_tab(
                        ui,
                        regular::SPARKLE,
                        effect.label(),
                        show_labels,
                        *selection == Some(effect),
                        egui::vec2(Self::CONTEXT_TAB_WIDTH, tab_height),
                        effect.label(),
                    )
                    .clicked()
                })
                .inner;
            if clicked {
                selected_tab = Some(effect);
            }
        }
        if let Some(effect) = selected_tab {
            *selection = Some(effect);
        }

        if components.len() >= crate::pipeline::MAX_EFFECT_COMPONENTS {
            return false;
        }
        let response = ui
            .push_id("add-effect-tab", |ui| {
                Self::mobile_icon_tab(
                    ui,
                    regular::PLUS,
                    "Add",
                    show_labels,
                    false,
                    egui::vec2(Self::CONTEXT_TAB_WIDTH, tab_height),
                    "Add effect",
                )
            })
            .inner;
        let mut added = None;
        moduwu_design::dropdown_menu(&response, |ui| {
            added = Self::effect_creation_menu(ui, components);
        });
        if let Some(effect) = added {
            components.push(crate::pipeline::EffectComponent::new(effect));
            *selection = Some(effect);
            return true;
        }
        false
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
        let interaction = moduwu_design::interaction_visuals(ui, &response, selected);
        let tile_width = if size.y > 54.0 { 56.0 } else { 50.0 };
        let tile = egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(size.x.min(tile_width), size.y - 4.0),
        );
        if interaction.state != moduwu_design::InteractionVisualState::Inactive {
            painter.rect_filled(tile, 6.0, interaction.weak_fill);
        }

        let color = if interaction.state == moduwu_design::InteractionVisualState::Inactive {
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

    pub(super) fn show_mobile_footer_actions(ui: &mut Ui, app: &mut CalibRawApp) {
        ui.add_space(moduwu_design::SPACE_XS);
        let width = ui.available_width().max(1.0);
        ui.allocate_ui_with_layout(
            egui::vec2(width, moduwu_design::TOOLBAR_HEIGHT),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                match app.ui.sidebar_tab {
                    // Intentionally no global reset in Edit on mobile.
                    SidebarTab::Adjustments => {}
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
                    SidebarTab::Inpainting => {
                        let active_tool = app.inpaint.tool;
                        let active_stroke_count = app
                            .inpaint
                            .edits
                            .strokes
                            .iter()
                            .filter(|stroke| {
                                active_tool
                                    .matches_stroke_tool(stroke.retouch.map(|retouch| retouch.tool))
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
                    SidebarTab::Presets => {
                        #[cfg(not(target_os = "android"))]
                        crate::ui::presets::show_header_actions(ui, app);
                    }
                    SidebarTab::Export | SidebarTab::Info => {}
                }
                Self::show_vertical_card_footer_actions(ui);
                Self::show_histogram_toggle(ui, app);
                Self::show_clipping_toggles(ui, app);
            },
        );
    }

    #[cfg(target_os = "android")]
    pub(crate) fn show_android_landscape_primary_tabs(ui: &mut Ui, app: &mut CalibRawApp) {
        ui.set_width(Self::ANDROID_LANDSCAPE_TOOL_RAIL_WIDTH);
        ui.spacing_mut().item_spacing.y = 0.0;
        let show_labels = app.preferences.show_develop_navigation_labels;
        ui.vertical_centered(|ui| {
            for tab in SidebarTab::ALL {
                if Self::mobile_icon_tab(
                    ui,
                    tab.glyph(),
                    tab.short_label(),
                    show_labels,
                    app.ui.sidebar_tab == tab,
                    egui::vec2(56.0, 56.0),
                    tab.tooltip(),
                )
                .clicked()
                {
                    app.dispatch_action(AppAction::SelectSidebarTab(tab));
                }
            }
        });
    }
}
