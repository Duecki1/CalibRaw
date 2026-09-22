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
    crate::ui::theme::dropdown_menu(&response, add_contents);
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
        crate::ui::theme::card_header(ui, |ui| {
            let width = ui.available_width().max(1.0);
            ui.allocate_ui_with_layout(
                egui::vec2(width, crate::ui::theme::TOOLBAR_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    crate::ui::theme::toolbar_title(ui, "Edit");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if crate::ui::icons::phosphor_icon_button(
                            ui,
                            egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                            crate::ui::theme::toolbar_icon_size(),
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
                        crate::ui::theme::card_gap(ui);
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
                crate::app::preview_visibility::PreviewVisibility::invalidate_mask_cache(ui.ctx());
                app.queue_preview_processing(crate::pipeline::ProcessingStage::Output);
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
        let mut request_subject = false;
        let mut request_object = false;
        let mut brush_mode = app.masks.brush_mode;
        let selected_is_subject = app.masks.stack.masks[mask_index]
            .components
            .get(component_index)
            .is_some_and(|component| {
                matches!(component.kind, MaskKind::Subject | MaskKind::Background)
            });
        let mut refinement_active = app.masks.subject_refinement_active && selected_is_subject;
        let mut refinement_size = app.masks.stack.subject_refinement.size;
        let mut refinement_feather = app.masks.stack.subject_refinement.feather;
        let mut refinement_flow = app.masks.stack.subject_refinement.flow;
        let mut clear_refinement = false;
        let mut local_curve_tab = app.develop_ui.tone_curve_tab;
        let mut local_color_grade_tab = app.develop_ui.color_grade_tab;
        let mut local_hsl_mixer_color = app.develop_ui.hsl_mixer_color;
        let mut local_point_color = app.develop_ui.mask_point_color.clone();
        let mut local_point_color_tab = app.develop_ui.mask_point_color_tab;
        let previous_point_color_preview = (
            local_point_color.visualize_range,
            local_point_color.selected,
            local_point_color.picker_active,
            local_point_color_tab,
        );
        let birefnet_quality = app.ai.birefnet_quality;
        let birefnet_quality_change_enabled = app.birefnet_quality_change_enabled();

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
                            &mut brush_mode,
                            (
                                &mut request_subject,
                                birefnet_quality,
                                birefnet_quality_change_enabled,
                            ),
                            (
                                &mut refinement_active,
                                &mut refinement_size,
                                &mut refinement_feather,
                                &mut refinement_flow,
                                &mut clear_refinement,
                            ),
                            &mut request_object,
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
                    crate::ui::theme::card_gap(ui);

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
                            (
                                &mut local_curve_tab,
                                &mut local_color_grade_tab,
                                &mut local_hsl_mixer_color,
                                &mut local_point_color,
                                &mut local_point_color_tab,
                            ),
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
                                    &mut brush_mode,
                                    (
                                        &mut request_subject,
                                        birefnet_quality,
                                        birefnet_quality_change_enabled,
                                    ),
                                    (
                                        &mut refinement_active,
                                        &mut refinement_size,
                                        &mut refinement_feather,
                                        &mut refinement_flow,
                                        &mut clear_refinement,
                                    ),
                                    &mut request_object,
                                );
                            });
                            geometry_changed |=
                                Self::apply_mask_properties_action(mask, component_index, action);
                        }
                        section => {
                            let changed = Self::show_local_adjustment_card(
                                ui,
                                &mut mask.adjustments,
                                section,
                                section_title,
                                true,
                                false,
                                (
                                    &mut local_curve_tab,
                                    &mut local_color_grade_tab,
                                    &mut local_hsl_mixer_color,
                                    &mut local_point_color,
                                    &mut local_point_color_tab,
                                ),
                            );
                            if changed {
                                mask.adjustments_enabled = true;
                                adjustments_changed = true;
                            }
                        }
                    }
                }
            }
            if orientation == MaskStripOrientation::Horizontal
                || vertical_section == Some(MaskSection::Properties)
            {
                let fullscreen = Self::is_plain_fullscreen_mask(mask);
                adjustments_changed |=
                    Self::show_effect_components(ui, &mut mask.effect_components, fullscreen);
            }
            light_rays_changed = light_rays_before != mask.has_light_rays_effect();
        }

        if crate::ui::theme::is_compact_portrait(ui)
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
            ui.add_space(crate::ui::theme::SPACE_XS);
        }

        app.develop_ui.tone_curve_tab = local_curve_tab;
        app.develop_ui.color_grade_tab = local_color_grade_tab;
        app.develop_ui.hsl_mixer_color = local_hsl_mixer_color;
        app.develop_ui.mask_point_color = local_point_color;
        app.develop_ui.mask_point_color_tab = local_point_color_tab;
        if previous_point_color_preview
            != (
                app.develop_ui.mask_point_color.visualize_range,
                app.develop_ui.mask_point_color.selected,
                app.develop_ui.mask_point_color.picker_active,
                app.develop_ui.mask_point_color_tab,
            )
        {
            crate::app::preview_visibility::PreviewVisibility::invalidate_mask_cache(ui.ctx());
            app.queue_preview_processing(crate::pipeline::ProcessingStage::Output);
        }
        app.masks.brush_mode = brush_mode;
        app.masks.subject_refinement_active = refinement_active;
        let refinement_settings_changed = app.masks.stack.subject_refinement.size
            != refinement_size
            || app.masks.stack.subject_refinement.feather != refinement_feather
            || app.masks.stack.subject_refinement.flow != refinement_flow;
        app.masks.stack.subject_refinement.size = refinement_size;
        app.masks.stack.subject_refinement.feather = refinement_feather;
        app.masks.stack.subject_refinement.flow = refinement_flow;
        if clear_refinement && !app.masks.stack.subject_refinement.is_empty() {
            app.masks.stack.subject_refinement.clear();
            app.mark_all_mask_layers_dirty();
        } else if refinement_settings_changed {
            app.note_mask_edit_changed();
        }
        if request_subject {
            app.request_subject_mask(frame);
        }
        if request_object {
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
