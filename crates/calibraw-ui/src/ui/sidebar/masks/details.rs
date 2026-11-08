use super::adjustments::{
    apply_local_adjustment_response, LocalAdjustmentTabs, LocalAdjustmentTarget,
};
use super::*;
use crate::app::{apply_mask_property_actions, MaskPropertiesControls, MaskPropertyChanges};
use crate::i18n::tr;

/// Open the kind picker from a creation button of the strip: beside a strip
/// along the right edge, above or below one along the top, and as a sheet on
/// touch layouts. `title` names what the pick creates, and `fill`, when set,
/// gives the picker its button's tint. Returns the picked kind.
fn kind_picker_from(
    response: &egui::Response,
    orientation: MaskStripOrientation,
    title: &str,
    fill: Option<egui::Color32>,
) -> Option<MaskKind> {
    let placement = match orientation {
        MaskStripOrientation::Vertical => moduwu_design::PickerPlacement::Left,
        MaskStripOrientation::Horizontal => moduwu_design::PickerPlacement::BelowOrAbove,
    };
    let mut picker =
        moduwu_design::Picker::new(kind_picker::KIND_PICKER_MIN_WIDTH, placement).title(title);
    if let Some(fill) = fill {
        picker = picker.fill(fill);
    }
    picker.show_menu(response, kind_picker::mask_kind_tiles)
}

/// The opaque fill of a creation tile tinted `amount` of the way toward
/// `accent`; an idle tile and its picker use [`IDLE_TINT`].
fn tinted_fill(visuals: &egui::Visuals, accent: egui::Color32, amount: f32) -> egui::Color32 {
    visuals.panel_fill.lerp_to_gamma(accent, amount)
}

const IDLE_TINT: f32 = 0.11;

/// The icon of a creation tile.
enum TileIcon {
    /// A group of its own: a plus.
    NewGroup,
    /// A sub-mask combined this way: the mode's glyph.
    Combine(MaskCombineMode),
}

/// A creation button of the strip: an icon beside its caption on a row in a
/// strip along the right edge. In a strip along the top, where phones have
/// little width, a combine tile shows only its glyph and the new-group tile
/// puts its short caption under the plus. `label` is also the accessible
/// name. `accent` tints the tile and colours the icon.
fn creation_tile(
    ui: &mut Ui,
    size: egui::Vec2,
    orientation: MaskStripOrientation,
    label: &str,
    icon: TileIcon,
    accent: egui::Color32,
) -> egui::Response {
    use eframe::egui::{Align2, FontId, Stroke, StrokeKind};

    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let enabled = ui.is_enabled();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let visuals = ui.visuals();
    let (accent, text_color) = if enabled {
        (accent, visuals.text_color())
    } else {
        (accent.gamma_multiply(0.4), visuals.weak_text_color())
    };
    let hovered = response.hovered() && enabled;
    let tint = if response.is_pointer_button_down_on() {
        0.30
    } else if hovered {
        0.20
    } else {
        IDLE_TINT
    };
    // The Subtract glyph cuts its square out with the fill, so it is opaque.
    let fill = tinted_fill(visuals, accent, tint);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, fill);
    painter.rect_stroke(
        rect,
        6.0,
        Stroke::new(1.0, accent.gamma_multiply(if hovered { 0.9 } else { 0.45 })),
        StrokeKind::Inside,
    );
    if response.has_focus() {
        painter.rect_stroke(rect, 6.0, visuals.selection.stroke, StrokeKind::Inside);
    }

    let captioned =
        !(orientation == MaskStripOrientation::Horizontal && matches!(icon, TileIcon::Combine(_)));
    let (icon_center, icon_width, text_anchor, text_align, text_width) = match orientation {
        MaskStripOrientation::Horizontal if !captioned => (
            rect.center(),
            20.0,
            rect.center(),
            Align2::CENTER_CENTER,
            0.0,
        ),
        MaskStripOrientation::Horizontal => (
            egui::pos2(rect.center().x, rect.top() + rect.height() * 0.38),
            22.0,
            egui::pos2(rect.center().x, rect.bottom() - 12.0),
            Align2::CENTER_CENTER,
            rect.width() - 6.0,
        ),
        MaskStripOrientation::Vertical => (
            egui::pos2(rect.left() + 13.0, rect.center().y),
            15.0,
            egui::pos2(rect.left() + 25.0, rect.center().y),
            Align2::LEFT_CENTER,
            rect.width() - 28.0,
        ),
    };
    match icon {
        TileIcon::NewGroup => {
            painter.text(
                icon_center,
                Align2::CENTER_CENTER,
                mask_creation_icon(),
                FontId::proportional(icon_width),
                accent,
            );
        }
        TileIcon::Combine(combine) => {
            super::paint_combine_glyph(&painter, icon_center, icon_width, combine, accent, fill);
        }
    }
    if !captioned {
        return response;
    }
    // Long translations shrink to fit the tile instead of spilling over.
    let font_size = 10.5;
    let natural = painter
        .layout_no_wrap(
            label.to_owned(),
            FontId::proportional(font_size),
            text_color,
        )
        .size()
        .x;
    let font_size = if natural > text_width {
        (font_size * text_width / natural).max(7.5)
    } else {
        font_size
    };
    painter.text(
        text_anchor,
        text_align,
        label,
        FontId::proportional(font_size),
        text_color,
    );
    response
}

impl Sidebar {
    pub(super) fn create_mask_group_card(
        ui: &mut Ui,
        new_mask: &mut Option<MaskKind>,
        orientation: MaskStripOrientation,
    ) {
        let size = MaskCardSize::Group.create_button_size(orientation);
        let accent = ui.visuals().text_color();
        let response = ui
            .push_id("mask-group-creation", |ui| {
                creation_tile(
                    ui,
                    size,
                    orientation,
                    tr("Mask"),
                    TileIcon::NewGroup,
                    accent,
                )
            })
            .inner
            .on_hover_text(tr("Create a new mask group"));
        *new_mask = kind_picker_from(&response, orientation, tr("Add mask"), None);
    }

    /// One button per combine mode, each opening the kind picker for a
    /// sub-mask combined that way: touch-sized tiles side by side in a strip
    /// along the top, rows as wide as a group card in a strip along the
    /// right edge.
    pub(super) fn create_submask_card(
        ui: &mut Ui,
        add_component: &mut Option<(MaskKind, MaskCombineMode)>,
        orientation: MaskStripOrientation,
    ) {
        const MODES: [MaskCombineMode; 3] = [
            MaskCombineMode::Add,
            MaskCombineMode::Subtract,
            MaskCombineMode::Intersect,
        ];
        let (button_size, layout) = match orientation {
            MaskStripOrientation::Horizontal => (
                MaskCardSize::Submask.create_button_size(orientation),
                egui::Layout::left_to_right(egui::Align::Center),
            ),
            MaskStripOrientation::Vertical => (
                MaskCardSize::Group.create_button_size(orientation),
                egui::Layout::top_down(egui::Align::Center),
            ),
        };
        ui.with_layout(layout, |ui| {
            ui.spacing_mut().item_spacing =
                egui::vec2(moduwu_design::SPACE_XXS, moduwu_design::SPACE_XXS);
            for combine in MODES {
                let accent = crate::ui::theme::mask_combine_color(combine, ui.visuals().dark_mode);
                let response = ui
                    .push_id(("mask-submask-creation", combine.label()), |ui| {
                        let response = creation_tile(
                            ui,
                            button_size,
                            orientation,
                            tr(combine.label()),
                            TileIcon::Combine(combine),
                            accent,
                        );
                        // The top strip's tiles have no caption to read.
                        match orientation {
                            MaskStripOrientation::Horizontal => {
                                response.on_hover_text(tr(combine.label()))
                            }
                            MaskStripOrientation::Vertical => response,
                        }
                    })
                    .inner;
                let title = match combine {
                    MaskCombineMode::Add => tr("Add sub-mask"),
                    MaskCombineMode::Subtract => tr("Subtract sub-mask"),
                    MaskCombineMode::Intersect => tr("Intersect sub-mask"),
                };
                let fill = tinted_fill(ui.visuals(), accent, IDLE_TINT);
                if let Some(kind) = kind_picker_from(&response, orientation, title, Some(fill)) {
                    *add_component = Some((kind, combine));
                }
            }
        });
    }

    pub(super) fn drop_placeholder(ui: &mut Ui, card_size: MaskCardSize) -> egui::Response {
        use eframe::egui::{Align2, FontId, Stroke, StrokeKind};

        let (rect, response) = ui.allocate_exact_size(card_size.card_size(), egui::Sense::hover());
        let red = crate::ui::theme::DROP_TARGET;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 5.0, red.gamma_multiply(0.18));
        painter.rect_stroke(rect, 5.0, Stroke::new(2.0, red), StrokeKind::Inside);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            tr("DROP"),
            FontId::proportional(9.0),
            red,
        );
        response
    }

    pub(super) fn paint_floating_card(ui: &Ui, drag: &StripDragState, pointer: egui::Pos2) {
        use eframe::egui::{Align2, Color32, FontId, LayerId, Order, Stroke, StrokeKind};

        let card_size = drag.card_size();
        let rect =
            egui::Rect::from_center_size(pointer + egui::vec2(12.0, 12.0), card_size.card_size());
        let painter = ui.ctx().layer_painter(LayerId::new(
            Order::Tooltip,
            egui::Id::new("floating-mask-drag-card"),
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

        paint_card_badge(&painter, visuals, image_rect, drag.source_badge, card_size);

        let display_label: String = drag
            .source_name
            .chars()
            .take(card_size.max_label_chars())
            .collect();
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
                    moduwu_design::toolbar_title(ui, tr("Edit"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if moduwu_design::ghost_icon_button(
                            ui,
                            egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                            moduwu_design::toolbar_icon_size(),
                            tr("Reset local adjustments"),
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

    /// The selected group's card and, below it, the selected sub-mask's,
    /// with their edits and resets applied to `mask`. On portrait screens
    /// the group card's footer reset also resets the sub-mask, whose card
    /// has no actions there.
    fn show_mask_property_cards(
        ui: &mut Ui,
        mask: &mut LocalMask,
        component_index: usize,
        controls: &mut MaskPropertiesControls,
        people: &crate::app::PeopleChoices,
        foldable: bool,
    ) -> MaskPropertyChanges {
        let mut group_actions = Vec::new();
        let group_action = Self::mask_group_card(ui, foldable, |ui| {
            group_actions = Self::show_mask_group_properties(ui, mask);
        });
        let mut submask_actions = Vec::new();
        let submask_action = Self::submask_card(ui, foldable, |ui| {
            submask_actions =
                Self::show_submask_properties(ui, mask, component_index, controls, people);
        });
        group_actions.extend(submask_actions);
        let mut changes =
            apply_mask_property_actions(mask, component_index, controls, group_actions);
        let footer_reset = Self::cards_use_footer(ui.ctx());
        changes.geometry |= Self::reset_mask_group(mask, group_action);
        if footer_reset {
            changes.geometry |= Self::reset_submask(mask, component_index, group_action);
        }
        changes.geometry |= Self::reset_submask(mask, component_index, submask_action);
        changes
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
        let full_image_mask_changed;
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
            generation_idle: app.ai_mask_generation_idle(),
            refinement_active: app.masks.subject_refinement_active && selected_is_subject,
            refinement_size: refinement.size,
            refinement_feather: refinement.feather,
            refinement_flow: refinement.flow,
            clear_refinement: false,
            request_generation: false,
            request_object: false,
            request_people: false,
            people_edit: None,
            separate_person_parts: false,
            hovered_person: None,
        };
        let people = app.people_choices(mask_index, component_index, frame);
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
            let full_image_mask_before = mask.reads_full_image_mask();
            if mask.effect != MaskEffect::Adjustment {
                mask.migrate_legacy_effect();
                adjustments_changed |= mask.effect == MaskEffect::Adjustment;
            }
            match orientation {
                MaskStripOrientation::Horizontal => {
                    let changes = Self::show_mask_property_cards(
                        ui,
                        mask,
                        component_index,
                        &mut controls,
                        &people,
                        true,
                    );
                    geometry_changed |= changes.geometry;
                    adjustments_changed |= changes.adjustments;

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
                        let response = Self::show_local_adjustment_card(
                            ui,
                            LocalAdjustmentTarget::of(mask),
                            section,
                            label,
                            default_open,
                            true,
                            LocalAdjustmentTabs::for_masks(&mut app.develop_ui),
                        );
                        adjustments_changed |=
                            apply_local_adjustment_response(ui.ctx(), mask, response);
                    }
                    let fullscreen = Self::is_plain_fullscreen_mask(mask);
                    adjustments_changed |= Self::show_effect_components(
                        ui,
                        &mut mask.effect_components,
                        crate::app::EffectList::Mask(mask_index),
                        &mut app.develop_ui.effect_instances,
                        fullscreen,
                        mask_effects::FilmNegativeTile::Hidden,
                        &effect_frame,
                    )
                    .0;
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
                            let changes = Self::show_mask_property_cards(
                                ui,
                                mask,
                                component_index,
                                &mut controls,
                                &people,
                                false,
                            );
                            geometry_changed |= changes.geometry;
                            adjustments_changed |= changes.adjustments;
                        }
                        MaskSection::Effects if app.develop_ui.mask_effect_component.is_some() => {
                            let fullscreen = Self::is_plain_fullscreen_mask(mask);
                            adjustments_changed |= Self::show_selected_effect_component(
                                ui,
                                &mut mask.effect_components,
                                crate::app::EffectList::Mask(mask_index),
                                &mut app.develop_ui.effect_instances,
                                &mut app.develop_ui.mask_effect_component,
                                fullscreen,
                                &effect_frame,
                            );
                        }
                        section => {
                            let response = Self::show_local_adjustment_card(
                                ui,
                                LocalAdjustmentTarget::of(mask),
                                section,
                                section_title,
                                true,
                                false,
                                LocalAdjustmentTabs::for_masks(&mut app.develop_ui),
                            );
                            adjustments_changed |=
                                apply_local_adjustment_response(ui.ctx(), mask, response);
                            if let Some(effect) = response.added_effect {
                                // Open the new effect's tab, as adding it from the tab strip does.
                                app.develop_ui.mask_effect_component = Some(effect);
                            }
                        }
                    }
                }
            }
            full_image_mask_changed = full_image_mask_before != mask.reads_full_image_mask();
        }

        if moduwu_design::is_compact_portrait(ui)
            && vertical_section.is_some_and(|section| section != MaskSection::Properties)
        {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(format!(
                        "{}  {}",
                        egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                        tr("Reset local adjustments")
                    ))
                    .on_hover_text(tr("Reset local adjustments"))
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
        app.ai.people.hovered = controls.hovered_person;
        if controls.separate_person_parts {
            app.separate_person_parts(mask_index, component_index, frame);
        } else if let Some(edit) = controls.people_edit {
            app.edit_people_mask(mask_index, component_index, edit, frame);
        } else if controls.request_people {
            app.request_people_masks(vec![(mask_index, component_index)], frame);
        }
        Self::apply_mask_geometry_change(ui, app, mask_index, geometry_changed);
        if full_image_mask_changed {
            app.mark_mask_geometry_dirty(mask_index);
        } else if adjustments_changed {
            app.mark_mask_adjustments_dirty();
        }
        crate::app::preview_visibility::PreviewVisibility::set_mask_scope(ui.ctx(), None);
        edit_header_rect
    }
}
