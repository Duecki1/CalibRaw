use super::film_negative::FilmNegativeAction;
use super::*;
use crate::app::{AreaPicker, CameraProfileChoice, FILM_NEGATIVE_CARD_TITLE};
use crate::i18n::{tr, tr_fmt};
use crate::ui::components::preview_list::{preview_combo, PreviewListItem};

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
                                if moduwu_design::ghost_icon_button(
                                    ui,
                                    egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                                    moduwu_design::toolbar_icon_size(),
                                    tr("Reset all develop adjustments"),
                                )
                                .clicked()
                                {
                                    app.reset_develop_adjustments();
                                }
                            }
                            SidebarTab::Crop => {
                                if moduwu_design::ghost_icon_button(
                                    ui,
                                    egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                                    moduwu_design::toolbar_icon_size(),
                                    tr("Reset crop and geometry"),
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
                                if ui
                                    .add_enabled_ui(
                                        active_stroke_count != 0 && !app.inpaint_processing(),
                                        |ui| {
                                            moduwu_design::ghost_icon_button(
                                                ui,
                                                egui_phosphor::regular::TRASH,
                                                moduwu_design::toolbar_icon_size(),
                                                &tr_fmt(
                                                    "Clear all {tool} strokes",
                                                    &[("tool", &active_tool.label())],
                                                ),
                                            )
                                        },
                                    )
                                    .inner
                                    .clicked()
                                {
                                    app.clear_inpainting_tool();
                                }
                            }
                            SidebarTab::Masks => {
                                if moduwu_design::ghost_icon_button(
                                    ui,
                                    egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                                    moduwu_design::toolbar_icon_size(),
                                    tr("Reset all masks and clear the subject mask cache"),
                                )
                                .clicked()
                                {
                                    app.reset_masks();
                                }
                            }
                            SidebarTab::Presets => {
                                if app.caps().has_folders() {
                                    crate::ui::presets::show_header_actions(ui, app);
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
            Self::settle_portrait_fill(
                ui.ctx(),
                scroll_output.content_size.y - scroll_output.inner_rect.height(),
            );

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

    /// The tool rail of the docked Develop layout, on desktop and tablets.
    /// The filmstrip is desktop-only, and so is its toggle.
    pub(crate) fn show_docked_tool_rail(ui: &mut Ui, app: &mut CalibRawApp) {
        use crate::ui::icons::{icon_toggle_button, UiIcon};

        ui.set_min_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = moduwu_design::SPACE_XS;
        let show_labels = app.preferences.show_develop_navigation_labels;
        ui.vertical_centered(|ui| {
            ui.add_space(5.0);
            for tab in SidebarTab::ALL {
                let selected = app.ui.sidebar_tab == tab;
                let clicked = ui
                    .scope(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        let mut clicked = moduwu_design::ghost_icon_toggle_button(
                            ui,
                            tab.glyph(),
                            selected,
                            moduwu_design::tool_rail_icon_size(),
                            tab.tooltip(),
                        )
                        .clicked();
                        if show_labels {
                            let color = if selected {
                                ui.visuals().text_color()
                            } else {
                                ui.visuals().weak_text_color()
                            };
                            clicked |= ui
                                .add(
                                    egui::Label::new(
                                        egui::RichText::new(tab.short_label())
                                            .size(10.0)
                                            .color(color),
                                    )
                                    .selectable(false)
                                    .sense(egui::Sense::click()),
                                )
                                .clicked();
                        }
                        clicked
                    })
                    .inner;
                if clicked {
                    app.dispatch_action(AppAction::SelectSidebarTab(tab));
                }
            }
        });

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            ui.add_space(5.0);

            let filmstrip_tooltip = if app.develop_ui.filmstrip_open {
                tr("Hide filmstrip")
            } else {
                tr("Show filmstrip")
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
                tr("Hide editing sidebar")
            } else {
                tr("Show editing sidebar")
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
        let mut changed = false;
        let mut lens_changed = false;
        let mut ai_denoise_request = None;
        let white_balance_raw = app.develop.loaded_raw.clone();
        let area_picker_before = app.develop_ui.area_picker;
        // Matches the GPU pipeline: only a Bayer-mosaic AI result is blended.
        // X-Trans and raster results are demosaiced RGB and always apply fully.
        let ai_strength_available = app.develop.loaded_raw.as_deref().is_some_and(|raw| {
            raw.cfa_kind == crate::pipeline::CfaKind::Bayer
                && !raw.is_pre_demosaiced_raster()
                && raw
                    .ai_denoised_image()
                    .is_some_and(|image| image.bayer_cfa().is_some())
        });
        // Detail resets restore this photo's first-open noise reduction, as
        // resetting all adjustments does, rather than switching it off.
        let mut detail_defaults = ExposureParams::default();
        if let Some(raw) = app.develop.original_raw.as_deref() {
            raw.apply_adaptive_detail_defaults(&mut detail_defaults);
        }
        if layout == ScreenLayout::Vertical {
            match app.develop_ui.adjustment_section {
                AdjustmentSection::Light => {
                    Self::show_camera_profile_selector(ui, app, frame);
                    Self::show_film_negative_card(ui, app, layout);
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
                AdjustmentSection::Lut => Self::show_look_lut_card(ui, app, layout),
                AdjustmentSection::Color => {
                    changed |= Self::show_color_card(ui, app, white_balance_raw.as_deref(), false);
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
                    let (detail_changed, request) = Self::show_detail(
                        ui,
                        &mut app.develop.exposure,
                        &detail_defaults,
                        ai_strength_available,
                        false,
                    );
                    changed |= detail_changed;
                    ai_denoise_request = request;
                }
                AdjustmentSection::Effects => {
                    let mut added = None;
                    if app.develop_ui.effect_component.is_none() {
                        let (presence_changed, presence_added) = Self::show_presence(
                            ui,
                            &mut app.develop.exposure,
                            &app.masks.stack.global_effects,
                            false,
                        );
                        changed |= presence_changed;
                        added = presence_added;
                    }
                    let added = Self::take_film_negative_pick(ui, app, layout, added);
                    Self::show_global_effects(ui, app, frame, layout, added);
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
                    let optics = Self::show_optics(ui, app, false);
                    lens_changed |= optics.lens;
                    changed |= optics.exposure;
                }
            }
        } else {
            let mut added = None;
            // Ahead of the cards the user orders: it changes what they edit.
            Self::show_film_negative_card(ui, app, layout);
            let order = app.preferences.develop_card_order.clone();
            for card in order.cards() {
                match card {
                    DevelopCard::CameraProfile => {
                        Self::show_camera_profile_selector(ui, app, frame);
                    }
                    DevelopCard::Light => {
                        changed |= Self::show_basic(ui, &mut app.develop.exposure, true);
                    }
                    DevelopCard::ToneCurve => {
                        changed |= Self::show_tone_curve(
                            ui,
                            &mut app.develop.exposure,
                            &mut app.develop_ui.tone_curve_tab,
                            true,
                        );
                    }
                    DevelopCard::Lut => Self::show_look_lut_card(ui, app, layout),
                    DevelopCard::Color => {
                        changed |=
                            Self::show_color_card(ui, app, white_balance_raw.as_deref(), true);
                    }
                    DevelopCard::ColorGrading => {
                        changed |= Self::show_color_grading(
                            ui,
                            &mut app.develop.exposure,
                            &mut app.develop_ui.color_grade_tab,
                            true,
                        );
                    }
                    DevelopCard::Detail => {
                        let (detail_changed, request) = Self::show_detail(
                            ui,
                            &mut app.develop.exposure,
                            &detail_defaults,
                            ai_strength_available,
                            true,
                        );
                        changed |= detail_changed;
                        ai_denoise_request = request;
                    }
                    DevelopCard::Effects => {
                        let (presence_changed, presence_added) = Self::show_presence(
                            ui,
                            &mut app.develop.exposure,
                            &app.masks.stack.global_effects,
                            true,
                        );
                        changed |= presence_changed;
                        added = presence_added;
                    }
                    DevelopCard::ColorMixer => {
                        changed |= Self::show_hsl(
                            ui,
                            &mut app.develop.exposure,
                            &mut app.develop_ui.hsl_mixer_color,
                            &mut app.develop_ui.point_color,
                            &mut app.develop_ui.point_color_tab,
                            true,
                        );
                    }
                    DevelopCard::LensCorrections => {
                        let optics = Self::show_optics(ui, app, true);
                        lens_changed |= optics.lens;
                        changed |= optics.exposure;
                    }
                }
            }
            // The effect cards follow every adjustment card, whatever their order.
            let added = Self::take_film_negative_pick(ui, app, layout, added);
            Self::show_global_effects(ui, app, frame, layout, added);
        }

        // One picker at a time: one started this pass ends the other.
        if app.develop_ui.area_picker.is_some() && app.develop_ui.area_picker != area_picker_before
        {
            app.develop_ui.point_color.picker_active = false;
        }
        if app.develop_ui.point_color.picker_active {
            app.develop_ui.cancel_area_picker();
        }
        if changed {
            app.develop.exposure.sanitize_tone_curves();
            app.mark_pipeline_dirty();
        }
        if lens_changed {
            app.lens_correction_edited();
        }
        if let Some(enabled) = ai_denoise_request {
            app.set_ai_denoise_enabled(enabled, frame);
        }
    }

    /// The global effects: adds `added`, then shows the scene depth update card
    /// while an effect needs depth, and the effect cards. A vertical layout
    /// shows only the selected effect's card, in the Effects section; a wide
    /// one draws them all below every other card, ending with an Add effect
    /// button ([`Self::show_effect_components`]).
    fn show_global_effects(
        ui: &mut Ui,
        app: &mut CalibRawApp,
        frame: &eframe::Frame,
        layout: ScreenLayout,
        added: Option<MaskEffect>,
    ) {
        if let Some(effect) = added {
            if app.add_global_effect(effect, layout) {
                moduwu_design::request_scroll_into_view(
                    ui.ctx(),
                    mask_effects::effect_card_scroll_target(effect),
                    None,
                );
            }
        }
        if app.masks.stack.has_scene_depth_effect() {
            Self::show_ai_update_card(ui, app, frame);
        }
        let effect_frame = mask_effects::EffectFrame::of(app);
        let changed = if layout == ScreenLayout::Vertical {
            Self::show_selected_effect_component(
                ui,
                &mut app.masks.stack.global_effects,
                &mut app.develop_ui.effect_component,
                true,
                &effect_frame,
            )
        } else {
            let (changed, film_picked) = Self::show_effect_components(
                ui,
                &mut app.masks.stack.global_effects,
                true,
                mask_effects::FilmNegativeTile::for_film(&app.develop.exposure.film),
                &effect_frame,
            );
            if film_picked {
                Self::start_film_negative(ui.ctx(), app, layout);
            }
            changed
        };
        if changed {
            app.mark_mask_adjustments_dirty();
        }
    }

    /// The Film negative card, while the photo is developed as a negative.
    fn show_film_negative_card(ui: &mut Ui, app: &mut CalibRawApp, layout: ScreenLayout) {
        let Some(negative) = app.develop.exposure.film.negative().copied() else {
            return;
        };
        let picking_base = app.develop_ui.area_picker == Some(AreaPicker::FilmBase);
        let foldable = layout == ScreenLayout::Horizontal;
        match Self::show_film_negative(ui, &negative, picking_base, foldable) {
            Some(FilmNegativeAction::Stock(stock)) => app.set_film_stock(stock),
            Some(FilmNegativeAction::PickBase(active)) => {
                app.develop_ui.set_area_picker(AreaPicker::FilmBase, active);
            }
            Some(FilmNegativeAction::Auto) => app.auto_film_negative(),
            Some(FilmNegativeAction::Remove) => app.remove_film_negative(),
            None => {}
        }
    }

    /// The effect of an Add effect `pick` of the global effects. The film
    /// negative tile instead develops the photo as a negative.
    fn take_film_negative_pick(
        ui: &Ui,
        app: &mut CalibRawApp,
        layout: ScreenLayout,
        pick: Option<mask_effects::EffectPick>,
    ) -> Option<MaskEffect> {
        if pick == Some(mask_effects::EffectPick::FilmNegative) {
            Self::start_film_negative(ui.ctx(), app, layout);
        }
        pick.and_then(mask_effects::EffectPick::effect)
    }

    /// Develops the photo as a film negative and shows its card, which leads
    /// the Light section of the vertical layout.
    fn start_film_negative(ctx: &egui::Context, app: &mut CalibRawApp, layout: ScreenLayout) {
        if !app.enable_film_negative() {
            return;
        }
        if layout == ScreenLayout::Vertical {
            app.develop_ui.adjustment_section = AdjustmentSection::Light;
            app.develop_ui.effect_component = None;
        }
        adjustment_cards::reveal_adjustment_card(ctx, FILM_NEGATIVE_CARD_TITLE);
    }

    /// The Color card, whose eyedropper is the white balance [`AreaPicker`].
    fn show_color_card(
        ui: &mut Ui,
        app: &mut CalibRawApp,
        raw: Option<&LoadedRaw>,
        foldable: bool,
    ) -> bool {
        let mut picking = app.develop_ui.area_picker == Some(AreaPicker::WhiteBalance);
        let changed = Self::show_color(ui, &mut app.develop.exposure, raw, &mut picking, foldable);
        app.develop_ui
            .set_area_picker(AreaPicker::WhiteBalance, picking);
        changed
    }

    /// Whether a photo is open and there is a LUT to show or a library to
    /// choose one from.
    fn look_lut_card_shown(app: &CalibRawApp) -> bool {
        app.develop.loaded_raw.is_some()
            && (app.luts.is_available() || app.develop.look_lut.is_some())
    }

    fn show_look_lut_card(ui: &mut Ui, app: &mut CalibRawApp, layout: ScreenLayout) {
        if !Self::look_lut_card_shown(app) {
            return;
        }
        let response = Self::show_look_lut(
            ui,
            &look_lut::LookLutView {
                look: app.develop.look_lut.as_ref(),
                entries: app.luts.entries(),
                can_import: app.luts.is_available(),
                layout,
            },
        );
        if let Some(choice) = response.hovered {
            app.luts.hover.request(choice);
        }
        match response.action {
            Some(look_lut::LookLutAction::Select(id)) => app.select_look_lut(id),
            Some(look_lut::LookLutAction::Amount(amount)) => app.set_look_lut_amount(amount),
            Some(look_lut::LookLutAction::Import) => app.choose_lut_files_to_import(true),
            None => {}
        }
    }

    /// The Camera profile card: a dropdown of automatic selection, the RAW's
    /// embedded matrix and the matching DCPs. Resting the pointer on one in
    /// the open menu previews it.
    fn show_camera_profile_selector(ui: &mut Ui, app: &mut CalibRawApp, frame: &eframe::Frame) {
        if app.preferences.camera_profile_mode == crate::pipeline::CameraProfileMode::MatrixOnly {
            return;
        }
        let Some(raw) = app.develop.loaded_raw.as_ref() else {
            return;
        };
        if raw.available_camera_profiles.is_empty() {
            return;
        }
        let selected = app.develop.selected_camera_profile.clone();
        let items = camera_profile_items(
            raw,
            &selected,
            app.preferences.camera_profile_folder.as_ref(),
        );
        let mut combo = None;
        moduwu_design::section_card(ui, tr("Camera profile"), |ui| {
            combo = Some(preview_combo(ui, "current-image-camera-profile", items));
        });
        moduwu_design::card_gap(ui);

        let Some(combo) = combo else {
            return;
        };
        if let Some(choice) = combo.hovered {
            app.develop.profile_preview.hover.request(choice);
        }
        if let Some(choice) = combo.clicked.filter(|choice| *choice != selected) {
            app.select_camera_profile_for_current(choice, frame);
        }
    }
}

/// The rows of the Camera profile card for `raw` with `selected` chosen:
/// automatic selection, which names the profile it applies, the embedded
/// matrix, which the profile folder `root` stands for, and each matching DCP.
fn camera_profile_items(
    raw: &LoadedRaw,
    selected: &CameraProfileChoice,
    root: Option<&std::path::PathBuf>,
) -> Vec<PreviewListItem<CameraProfileChoice>> {
    let candidates = &raw.available_camera_profiles;
    let automatic = if selected.is_none() {
        let active_name = raw
            .profile_color
            .camera_profile_source
            .as_ref()
            .and_then(|active| {
                candidates
                    .iter()
                    .find(|candidate| candidate.path == *active)
                    .map(|candidate| candidate.name.clone())
            })
            .or_else(|| raw.profile_color.camera_profile.name.clone())
            .unwrap_or_else(|| tr("Embedded Matrix").to_owned());
        tr_fmt("Automatic — {profile}", &[("profile", &active_name)])
    } else {
        tr("Automatic").to_owned()
    };
    let mut items = vec![PreviewListItem::choice(None, automatic, selected.is_none())];
    if let Some(root) = root {
        items.push(PreviewListItem::choice(
            Some(root.clone()),
            tr("Embedded Matrix"),
            selected.as_ref() == Some(root),
        ));
    }
    items.extend(candidates.iter().map(|candidate| {
        PreviewListItem::choice(
            Some(candidate.path.clone()),
            &candidate.name,
            selected.as_ref() == Some(&candidate.path),
        )
    }));
    items
}
