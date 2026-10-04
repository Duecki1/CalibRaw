use super::adjustments::LocalAdjustmentTabs;
use super::properties::MaskPropertiesControls;
use super::*;

fn mask_creation_menu_button<R>(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    size: egui::Vec2,
    icon_size: f32,
    tooltip: &str,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::Response {
    let response = ui
        .push_id(id_salt, |ui| {
            ui.add_sized(
                size,
                egui::Button::new(
                    egui::RichText::new(mask_creation_icon())
                        .size(icon_size)
                        .strong(),
                )
                .corner_radius(6.0),
            )
        })
        .inner
        .on_hover_text(tooltip);
    moduwu_design::dropdown_menu(&response, add_contents);
    response
}

impl Sidebar {
    pub(super) fn create_mask_group_card(
        ui: &mut Ui,
        new_mask: &mut Option<MaskKind>,
        orientation: MaskStripOrientation,
    ) {
        let size = MaskCardSize::Group.create_button_size(orientation);
        ui.allocate_ui_with_layout(
            size,
            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
            |ui| {
                ui.spacing_mut().interact_size = size;
                mask_creation_menu_button(
                    ui,
                    "mask-group-creation",
                    size,
                    20.0,
                    "Create a new mask group",
                    |ui| {
                        *new_mask = Self::mask_kind_menu(
                            ui,
                            "This mask type is planned but not implemented yet.",
                        );
                    },
                );
            },
        );
    }

    pub(super) fn create_submask_card(
        ui: &mut Ui,
        add_component: &mut Option<(MaskKind, MaskCombineMode)>,
        orientation: MaskStripOrientation,
    ) {
        let size = MaskCardSize::Submask.create_button_size(orientation);
        ui.allocate_ui_with_layout(
            size,
            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
            |ui| {
                ui.spacing_mut().interact_size = size;
                mask_creation_menu_button(
                    ui,
                    "mask-submask-creation",
                    size,
                    18.0,
                    "Add a sub-mask to the selected group",
                    |ui| {
                        *add_component = Self::submask_creation_menu(
                            ui,
                            "This sub-mask type is planned but not implemented yet.",
                        );
                    },
                );
            },
        );
    }

    pub(super) fn submask_drop_placeholder(ui: &mut Ui) -> egui::Response {
        use eframe::egui::{Align2, FontId, Stroke, StrokeKind};

        let (rect, response) =
            ui.allocate_exact_size(MaskCardSize::Submask.card_size(), egui::Sense::hover());
        let red = crate::ui::theme::DROP_TARGET;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 5.0, red.gamma_multiply(0.18));
        painter.rect_stroke(rect, 5.0, Stroke::new(2.0, red), StrokeKind::Inside);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "DROP",
            FontId::proportional(9.0),
            red,
        );
        response
    }

    pub(super) fn paint_floating_submask(ui: &Ui, drag: &SubmaskDragState, pointer: egui::Pos2) {
        use eframe::egui::{Align2, Color32, FontId, LayerId, Order, Stroke, StrokeKind};

        let card_size = MaskCardSize::Submask;
        let rect =
            egui::Rect::from_center_size(pointer + egui::vec2(12.0, 12.0), card_size.card_size());
        let painter = ui.ctx().layer_painter(LayerId::new(
            Order::Tooltip,
            egui::Id::new("floating-submask-drag-card"),
        ));
        let visuals = ui.visuals();
        painter.rect_filled(rect, 5.0, visuals.widgets.active.bg_fill);
        painter.rect_stroke(
            rect,
            5.0,
            Stroke::new(2.0, visuals.selection.bg_fill),
            StrokeKind::Inside,
        );

        let image_edge = card_size.image_edge();
        let image_rect = egui::Rect::from_min_size(
            egui::pos2(rect.center().x - image_edge * 0.5, rect.min.y + 5.0),
            egui::vec2(image_edge, image_edge),
        );
        painter.rect_filled(image_rect, 3.0, Color32::BLACK);
        if let Some(texture) = drag.source_texture.as_ref() {
            painter.image(
                texture.id(),
                image_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                if drag.source_enabled {
                    Color32::WHITE
                } else {
                    Color32::from_white_alpha(80)
                },
            );
        }

        let badge_height = 16.0;
        let badge_size = egui::vec2(
            (drag.source_badge.chars().count() as f32 * 9.0 * 0.62 + 8.0).max(badge_height + 2.0),
            badge_height,
        );
        let badge_rect =
            egui::Rect::from_min_size(image_rect.right_bottom() - badge_size, badge_size);
        painter.rect_filled(badge_rect, 3.0, Color32::from_black_alpha(210));
        painter.text(
            badge_rect.center(),
            Align2::CENTER_CENTER,
            &drag.source_badge,
            FontId::proportional(9.0),
            Color32::WHITE,
        );

        let display_label: String = drag.source_name.chars().take(10).collect();
        painter.text(
            egui::pos2(rect.center().x, rect.bottom() - 9.0),
            Align2::CENTER_CENTER,
            display_label,
            FontId::proportional(card_size.label_font_size()),
            if drag.source_enabled {
                visuals.text_color()
            } else {
                visuals.weak_text_color()
            },
        );
    }

    pub(super) fn show_masks_horizontal_details(
        ui: &mut Ui,
        app: &mut CalibRawApp,
        frame: &eframe::Frame,
    ) -> Option<egui::Rect> {
        Self::show_mask_details(ui, app, frame, MaskStripOrientation::Horizontal)
    }

    pub(super) fn show_masks_vertical_details(
        ui: &mut Ui,
        app: &mut CalibRawApp,
        frame: &eframe::Frame,
    ) {
        Self::show_mask_details(ui, app, frame, MaskStripOrientation::Vertical);
    }

    fn render_mask_edit_header(ui: &mut Ui, on_reset: impl FnOnce()) -> egui::InnerResponse<()> {
        moduwu_design::card_header(ui, |ui| {
            let width = ui.available_width().max(1.0);
            ui.allocate_ui_with_layout(
                egui::vec2(width, moduwu_design::TOOLBAR_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    moduwu_design::toolbar_title(ui, "Edit");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if moduwu_design::icon_button(
                            ui,
                            egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                            moduwu_design::toolbar_icon_size(),
                            "Reset local adjustments",
                        )
                        .clicked()
                        {
                            on_reset();
                        }
                    });
                },
            );
        })
    }

    pub(crate) fn show_sticky_mask_edit_header(
        ctx: &egui::Context,
        app: &mut CalibRawApp,
        header_rect: egui::Rect,
        viewport_rect: egui::Rect,
    ) {
        if header_rect.top() >= viewport_rect.top() {
            return;
        }

        egui::Area::new(egui::Id::new("sticky-mask-edit-header"))
            .order(egui::Order::Foreground)
            .fixed_pos(viewport_rect.min)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(ui.visuals().panel_fill)
                    .inner_margin(egui::Margin::ZERO)
                    .show(ui, |ui| {
                        ui.set_width(viewport_rect.width());
                        ui.set_max_width(viewport_rect.width());
                        Self::render_mask_edit_header(ui, || {
                            if let Some(mask) = app.masks.stack.selected_mask_mut() {
                                mask.adjustments.reset();
                            }
                            app.mark_mask_adjustments_dirty();
                        });
                        moduwu_design::card_gap(ui);
                    });
            });
    }

    fn show_mask_details(
        ui: &mut Ui,
        app: &mut CalibRawApp,
        frame: &eframe::Frame,
        orientation: MaskStripOrientation,
    ) -> Option<egui::Rect> {
        let (mask_index, component_index) = app.masks.stack.ensure_selection()?;
        if app.develop_ui.mask_point_color_mask != Some(mask_index) {
            let was_visualizing = app.develop_ui.mask_point_color.visualize_range;
            app.develop_ui.mask_point_color = Default::default();
            app.develop_ui.mask_point_color_mask = Some(mask_index);
            if was_visualizing {
                app.refresh_mask_overlay_preview();
            }
        }
        crate::app::preview_visibility::PreviewVisibility::set_mask_scope(
            ui.ctx(),
            Some(mask_index),
        );

        let vertical_section =
            (orientation == MaskStripOrientation::Vertical).then_some(app.develop_ui.mask_section);

        let mut geometry_changed = false;
        let mut adjustments_changed = false;
        let light_rays_changed;
        let mut edit_header_rect = None;
        let selected_is_subject = app.masks.stack.masks[mask_index]
            .components
            .get(component_index)
            .is_some_and(|component| {
                matches!(component.kind, MaskKind::Subject | MaskKind::Background)
            });
        let refinement = &app.masks.stack.subject_refinement;
        let mut controls = MaskPropertiesControls {
            brush_mode: app.masks.brush_mode,
            birefnet_quality: app.ai.birefnet_quality,
            generation_idle: app.birefnet_quality_change_enabled(),
            refinement_active: app.masks.subject_refinement_active && selected_is_subject,
            refinement_size: refinement.size,
            refinement_feather: refinement.feather,
            refinement_flow: refinement.flow,
            clear_refinement: false,
            request_generation: false,
            request_object: false,
        };
        let point_color_preview = |develop_ui: &crate::app::DevelopUiState| {
            (
                develop_ui.mask_point_color.visualize_range,
                develop_ui.mask_point_color.selected,
                develop_ui.mask_point_color.picker_active,
                develop_ui.mask_point_color_tab,
            )
        };
        let previous_point_color_preview = point_color_preview(&app.develop_ui);

        let effect_frame = mask_effects::EffectFrame::of(app);
        {
            let mask = &mut app.masks.stack.masks[mask_index];
            let light_rays_before = mask.has_light_rays_effect();
            if mask.effect != MaskEffect::Adjustment {
                mask.migrate_legacy_effect();
                adjustments_changed |= mask.effect == MaskEffect::Adjustment;
            }
            match orientation {
                MaskStripOrientation::Horizontal => {
                    let action = Self::mask_properties_card(ui, true, true, true, |ui| {
                        geometry_changed |= Self::show_vertical_mask_properties(
                            ui,
                            mask,
                            component_index,
                            &mut controls,
                        );
                    });
                    geometry_changed |=
                        Self::apply_mask_properties_action(mask, component_index, action);

                    edit_header_rect = Some(
                        Self::render_mask_edit_header(ui, || {
                            mask.adjustments.reset();
                            adjustments_changed = true;
                        })
                        .response
                        .rect,
                    );
                    moduwu_design::card_gap(ui);

                    for (section, label, default_open) in [
                        (MaskSection::Light, "Light", true),
                        (MaskSection::ToneCurve, "Tone Curve", false),
                        (MaskSection::Color, "Color", false),
                        (MaskSection::ColorGrading, "Color Grading", false),
                        (MaskSection::Effects, "Effects", false),
                        (MaskSection::ColorMixer, "Color Mixer", false),
                    ] {
                        let changed = Self::show_local_adjustment_card(
                            ui,
                            &mut mask.adjustments,
                            section,
                            label,
                            default_open,
                            true,
                            LocalAdjustmentTabs::for_masks(&mut app.develop_ui),
                        );
                        if changed {
                            mask.adjustments_enabled = true;
                            adjustments_changed = true;
                        }
                    }
                }
                MaskStripOrientation::Vertical => {
                    let mask_section = vertical_section.expect("vertical details have a section");
                    let section_title = match mask_section {
                        MaskSection::Properties => "Mask Properties",
                        MaskSection::Light => "Light",
                        MaskSection::ToneCurve => "Tone Curve",
                        MaskSection::Color => "Color",
                        MaskSection::ColorGrading => "Color Grading",
                        MaskSection::Effects => "Effects",
                        MaskSection::ColorMixer => "Color Mixer",
                    };
                    match mask_section {
                        MaskSection::Properties => {
                            let action = Self::mask_properties_card(ui, true, false, true, |ui| {
                                geometry_changed |= Self::show_vertical_mask_properties(
                                    ui,
                                    mask,
                                    component_index,
                                    &mut controls,
                                );
                            });
                            geometry_changed |=
                                Self::apply_mask_properties_action(mask, component_index, action);
                        }
                        MaskSection::Effects if app.develop_ui.mask_effect_component.is_some() => {
                            let fullscreen = Self::is_plain_fullscreen_mask(mask);
                            adjustments_changed |= Self::show_selected_effect_component(
                                ui,
                                &mut mask.effect_components,
                                &mut app.develop_ui.mask_effect_component,
                                fullscreen,
                                &effect_frame,
                            );
                        }
                        section => {
                            let changed = Self::show_local_adjustment_card(
                                ui,
                                &mut mask.adjustments,
                                section,
                                section_title,
                                true,
                                false,
                                LocalAdjustmentTabs::for_masks(&mut app.develop_ui),
                            );
                            if changed {
                                mask.adjustments_enabled = true;
                                adjustments_changed = true;
                            }
                        }
                    }
                }
            }
            if orientation == MaskStripOrientation::Horizontal {
                let fullscreen = Self::is_plain_fullscreen_mask(mask);
                adjustments_changed |= Self::show_effect_components(
                    ui,
                    &mut mask.effect_components,
                    fullscreen,
                    &effect_frame,
                );
            }
            light_rays_changed = light_rays_before != mask.has_light_rays_effect();
        }

        if moduwu_design::is_compact_portrait(ui)
            && vertical_section.is_some_and(|section| section != MaskSection::Properties)
        {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(format!(
                        "{}  Reset local adjustments",
                        egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE
                    ))
                    .on_hover_text("Reset local adjustments")
                    .clicked()
                {
                    app.masks.stack.masks[mask_index].adjustments.reset();
                    adjustments_changed = true;
                }
            });
            ui.add_space(moduwu_design::SPACE_XS);
        }

        if previous_point_color_preview != point_color_preview(&app.develop_ui) {
            app.refresh_mask_overlay_preview();
        }
        app.masks.brush_mode = controls.brush_mode;
        app.masks.subject_refinement_active = controls.refinement_active;
        let refinement = &mut app.masks.stack.subject_refinement;
        let refinement_settings_changed = refinement.size != controls.refinement_size
            || refinement.feather != controls.refinement_feather
            || refinement.flow != controls.refinement_flow;
        refinement.size = controls.refinement_size;
        refinement.feather = controls.refinement_feather;
        refinement.flow = controls.refinement_flow;
        if controls.clear_refinement && !refinement.is_empty() {
            refinement.clear();
            app.mark_all_mask_layers_dirty();
        } else if refinement_settings_changed {
            app.note_mask_edit_changed();
        }
        if controls.request_generation {
            match app.masks.stack.masks[mask_index].components[component_index].kind {
                MaskKind::Sky => app.request_sky_mask(frame),
                MaskKind::DepthRange => {
                    // The properties button explicitly regenerates an existing map.
                    if app.masks.stack.masks[mask_index].components[component_index]
                        .geometry
                        .is_initialized()
                    {
                        app.masks.depth_cache = None;
                    }
                    app.request_depth_mask(frame);
                }
                _ => app.request_subject_mask(frame),
            }
        }
        if controls.request_object {
            app.request_object_mask(mask_index, component_index);
        }
        Self::apply_mask_geometry_change(ui, app, mask_index, geometry_changed);
        if light_rays_changed {
            app.mark_mask_geometry_dirty(mask_index);
        } else if adjustments_changed {
            app.mark_mask_adjustments_dirty();
        }
        crate::app::preview_visibility::PreviewVisibility::set_mask_scope(ui.ctx(), None);
        edit_header_rect
    }
}
