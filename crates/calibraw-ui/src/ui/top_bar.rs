use crate::app::{AppAction, AppTab, CalibRawApp};
use crate::ui::theme;
use eframe::egui::{self, Ui};

pub(crate) struct TopBar;

#[cfg(not(target_os = "android"))]
const LIBRARY_SIDEBAR_ALIGNMENT_ID: &str = "library-sidebar-toolbar-alignment-x";
#[cfg(not(target_os = "android"))]
const DEVELOP_DOCK_MIN_WIDTH: f32 = 220.0;
#[cfg(not(target_os = "android"))]
const TOOLBAR_COMPACT_WIDTH: f32 = 620.0;
#[cfg(not(target_os = "android"))]
const TOOLBAR_COMPACT_REVIEW_WIDTH: f32 = 760.0;
#[cfg(not(target_os = "android"))]
const TOOLBAR_COMPACT_ZOOM_WIDTH: f32 = 176.0;

#[cfg(not(target_os = "android"))]
const DEVELOP_ZOOM_FIT_POSITION: f32 = 0.18;

#[cfg(not(target_os = "android"))]
fn develop_zoom_to_slider(zoom: f32) -> f32 {
    let min = crate::ui::preview::MIN_PREVIEW_ZOOM;
    let max = crate::ui::preview::MAX_PREVIEW_ZOOM;
    let zoom = zoom.clamp(min, max);
    if zoom <= 1.0 {
        let span = (1.0 - min).max(f32::EPSILON);
        ((zoom - min) / span * DEVELOP_ZOOM_FIT_POSITION).clamp(0.0, DEVELOP_ZOOM_FIT_POSITION)
    } else {
        let logarithmic = zoom.ln() / max.ln();
        (DEVELOP_ZOOM_FIT_POSITION + logarithmic * (1.0 - DEVELOP_ZOOM_FIT_POSITION))
            .clamp(DEVELOP_ZOOM_FIT_POSITION, 1.0)
    }
}

#[cfg(not(target_os = "android"))]
fn develop_slider_to_zoom(position: f32) -> f32 {
    let min = crate::ui::preview::MIN_PREVIEW_ZOOM;
    let max = crate::ui::preview::MAX_PREVIEW_ZOOM;
    let position = position.clamp(0.0, 1.0);
    if position <= DEVELOP_ZOOM_FIT_POSITION {
        let fraction = position / DEVELOP_ZOOM_FIT_POSITION.max(f32::EPSILON);
        egui::lerp(min..=1.0, fraction)
    } else {
        let fraction = (position - DEVELOP_ZOOM_FIT_POSITION) / (1.0 - DEVELOP_ZOOM_FIT_POSITION);
        (max.ln() * fraction).exp().clamp(1.0, max)
    }
}

#[cfg(not(target_os = "android"))]
fn develop_toolbar_dock_geometry(
    ctx: &egui::Context,
    app: &CalibRawApp,
    toolbar_right: f32,
) -> Option<(f32, f32)> {
    if app.ui.active_tab != AppTab::Develop {
        return None;
    }

    let sidebar_id = egui::Id::new(crate::ui::layout::DEVELOP_SIDEBAR_ID);
    let separator_x = if app.develop_ui.sidebar_open {
        if app.ui.sidebar_tab == crate::app::SidebarTab::Masks {
            egui::PanelState::load(ctx, egui::Id::new(crate::ui::layout::DEVELOP_MASK_STRIP_ID))
                .map(|state| state.outer_rect.left())
                .or_else(|| {
                    egui::PanelState::load(ctx, sidebar_id).map(|state| {
                        state.outer_rect.left()
                            - crate::ui::sidebar::Sidebar::HORIZONTAL_MASK_STRIP_WIDTH
                    })
                })
        } else {
            egui::PanelState::load(ctx, sidebar_id).map(|state| state.outer_rect.left())
        }
    } else {
        egui::PanelState::load(ctx, egui::Id::new(crate::ui::layout::DEVELOP_TOOL_RAIL_ID))
            .map(|state| state.outer_rect.left())
    };

    let separator_x = separator_x.unwrap_or_else(|| {
        // Panel state is available after the first layout pass. Until then use
        // the same sizing rules as the Develop dock as a one-frame fallback.
        let viewport_size = ctx.content_rect().size();
        let mut dock_width = crate::ui::sidebar::Sidebar::DESKTOP_TOOL_RAIL_WIDTH;
        if app.develop_ui.sidebar_open {
            let panel_max =
                crate::ui::layout::ScreenLayout::develop_sidebar_max_width(viewport_size);
            let default_width = crate::ui::layout::ScreenLayout::Horizontal
                .sidebar_default_size(viewport_size)
                .min(panel_max);
            let sidebar_width = crate::ui::layout::develop_sidebar_user_width(
                ctx,
                sidebar_id,
                default_width,
                crate::ui::layout::ScreenLayout::MIN_HORIZONTAL_SIDEBAR_WIDTH,
                panel_max,
            );
            dock_width += sidebar_width;
            if app.ui.sidebar_tab == crate::app::SidebarTab::Masks {
                dock_width += crate::ui::sidebar::Sidebar::HORIZONTAL_MASK_STRIP_WIDTH;
            }
        }
        ctx.content_rect().right() - dock_width
    });

    let width = toolbar_right - separator_x;
    (separator_x.is_finite() && width.is_finite())
        .then_some((separator_x, width.max(crate::ui::theme::SPACE_SM)))
}

#[cfg(not(target_os = "android"))]
pub(crate) fn load_toolbar_brand_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let image = image::load_from_memory(include_bytes!(
        "../../../../packaging/icons/CalibRawIconTransHoriz.png"
    ))
    .expect("embedded toolbar brand must be a valid PNG")
    .into_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let pixels = image.into_raw();
    ctx.load_texture(
        "calibraw-toolbar-brand",
        egui::ColorImage::from_rgba_unmultiplied(size, &pixels),
        egui::TextureOptions::LINEAR,
    )
}

impl TopBar {
    #[cfg(not(target_os = "android"))]
    pub(crate) fn library_sidebar_default_width(
        ctx: &egui::Context,
        viewport_left: f32,
    ) -> Option<f32> {
        // A left Panel paints its separator half a stroke-width inside its outer
        // edge. Include that inset so its painted line, rather than its layout
        // rect, aligns with the toolbar separator's painted centerline.
        let panel_separator_inset = ctx
            .style_of(ctx.theme())
            .visuals
            .widgets
            .noninteractive
            .bg_stroke
            .width
            * 0.5;
        ctx.data(|data| {
            data.get_temp::<f32>(egui::Id::new(LIBRARY_SIDEBAR_ALIGNMENT_ID))
                .map(|separator_x| separator_x - viewport_left + panel_separator_inset)
                .filter(|width| width.is_finite() && *width > 0.0)
        })
    }

    pub(crate) fn show(ui: &mut Ui, app: &mut CalibRawApp, frame: &eframe::Frame) {
        #[cfg(target_os = "android")]
        Self::show_portrait(ui, app, frame);
        #[cfg(not(target_os = "android"))]
        Self::show_desktop(ui, app, frame);
    }

    pub(crate) fn back_icon_button(ui: &mut Ui, size: egui::Vec2) -> egui::Response {
        crate::ui::icons::phosphor_icon_button(
            ui,
            egui_phosphor::regular::ARROW_LEFT,
            size,
            "Back to Library",
        )
    }

    fn history_icon_button(
        ui: &mut Ui,
        enabled: bool,
        redo: bool,
        size: egui::Vec2,
        hover_text: &str,
    ) -> egui::Response {
        let icon = if redo {
            egui_phosphor::regular::ARROW_U_UP_RIGHT
        } else {
            egui_phosphor::regular::ARROW_U_UP_LEFT
        };
        crate::ui::icons::phosphor_icon_button_enabled(ui, enabled, icon, size, hover_text)
    }

    fn show_history_controls(ui: &mut Ui, app: &mut CalibRawApp, shortcuts: bool) {
        let undo_tip = if shortcuts {
            "Undo the last edit (Ctrl/Cmd+Z)"
        } else {
            "Undo the last edit"
        };
        let redo_tip = if shortcuts {
            "Redo the last edit (Ctrl/Cmd+Shift+Z or Ctrl+Y)"
        } else {
            "Redo the last edit"
        };
        for (redo, tooltip) in [(false, undo_tip), (true, redo_tip)] {
            if Self::history_icon_button(
                ui,
                app.action_enabled(if redo {
                    AppAction::RedoEdit
                } else {
                    AppAction::UndoEdit
                }),
                redo,
                theme::toolbar_icon_size(),
                tooltip,
            )
            .clicked()
            {
                app.dispatch_action(if redo {
                    AppAction::RedoEdit
                } else {
                    AppAction::UndoEdit
                });
            }
        }
    }

    fn show_save_control(ui: &mut Ui, app: &mut CalibRawApp, shortcut: bool) {
        let tooltip = if app.sidecar_save_in_progress() {
            "Saving non-destructive edits…"
        } else if app.sidecar_save_succeeded_recently() {
            "Edits saved"
        } else if shortcut {
            "Save non-destructive edits beside the RAW (Ctrl/Cmd+S)"
        } else {
            "Save non-destructive edits"
        };
        let icon = if app.sidecar_save_succeeded_recently() {
            egui_phosphor::regular::CHECK
        } else {
            egui_phosphor::regular::FLOPPY_DISK
        };
        if crate::ui::icons::phosphor_icon_button_enabled(
            ui,
            app.action_enabled(AppAction::SaveEdits),
            icon,
            theme::toolbar_icon_size(),
            tooltip,
        )
        .clicked()
        {
            app.dispatch_action(AppAction::SaveEdits);
        }
    }

    pub(crate) fn show_portrait(ui: &mut Ui, app: &mut CalibRawApp, _frame: &eframe::Frame) {
        theme::prepare_toolbar(ui);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            app.show_export_task_indicator(ui);

            Self::show_save_control(ui, app, false);
            crate::ui::library::show_current_photo_review(ui, app, true);

            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                if Self::back_icon_button(ui, theme::toolbar_icon_size()).clicked() {
                    app.activate_tab(AppTab::Library);
                }
                Self::show_history_controls(ui, app, false);
            });
        });
    }

    #[cfg(not(target_os = "android"))]
    fn toolbar_brand_size() -> egui::Vec2 {
        let height = 24.0_f32.min(theme::TOOLBAR_HEIGHT);
        // CalibRawIconTransHoriz.png is 440x160, so preserve its 2.75:1 aspect.
        egui::vec2(height * 2.75, height)
    }

    #[cfg(not(target_os = "android"))]
    fn show_toolbar_brand(ui: &mut Ui, app: &CalibRawApp) {
        ui.add(
            egui::Image::new((app.toolbar_brand_texture.id(), Self::toolbar_brand_size()))
                .sense(egui::Sense::hover()),
        )
        .on_hover_text("CalibRaw");
    }

    #[cfg(not(target_os = "android"))]
    fn toolbar_brand_can_be_centered(tab: AppTab, width: f32) -> bool {
        // Centering is purely decorative, so only do it when both sides have a
        // generous amount of guaranteed room. At smaller/windowed widths the
        // brand becomes a normal right-side layout item instead, which means it
        // can never cover search/Open Folder, edit actions, task indicators, or
        // the Develop review controls.
        let minimum_width = match tab {
            AppTab::Library => 1500.0,
            AppTab::Develop => 1100.0,
            AppTab::Settings => 900.0,
        };
        width >= minimum_width
    }

    #[cfg(not(target_os = "android"))]
    fn paint_centered_toolbar_brand(ui: &Ui, app: &CalibRawApp) {
        let brand_size = Self::toolbar_brand_size();
        let brand_center = egui::pos2(ui.max_rect().center().x, ui.min_rect().center().y);
        let brand_rect = egui::Rect::from_center_size(brand_center, brand_size);
        ui.painter().image(
            app.toolbar_brand_texture.id(),
            brand_rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }

    #[cfg(not(target_os = "android"))]
    fn show_develop_zoom_control(ui: &mut Ui, app: &mut CalibRawApp) {
        let enabled = app.preview.gpu_pipeline.is_some();
        let available_width = ui.available_width().max(1.0);
        let compact = available_width < TOOLBAR_COMPACT_ZOOM_WIDTH;
        let readout_width = if compact { 43.0 } else { 49.0 };
        let icon_width = 16.0;
        let item_spacing = 4.0;
        // Everything except the track has a fixed width. Give the shared slider
        // exactly the rest so it cannot push the toolbar break away from the dock.
        let slider_width =
            (available_width - readout_width - icon_width - item_spacing * 2.0).max(1.0);
        let reset_position = develop_zoom_to_slider(1.0);
        let mut slider_position = develop_zoom_to_slider(app.preview.zoom);
        let mut requested_zoom = None;
        let mut reset_requested = false;

        ui.add_enabled_ui(enabled, |ui| {
            ui.spacing_mut().item_spacing.x = item_spacing;

            let icon_response = ui.add_sized(
                [icon_width, theme::CONTROL_HEIGHT],
                egui::Label::new(
                    egui::RichText::new(egui_phosphor::regular::MAGNIFYING_GLASS).size(15.0),
                )
                .sense(egui::Sense::click()),
            );
            if icon_response.double_clicked() {
                reset_requested = true;
            }

            let slider_interaction =
                crate::ui::components::adjustment_slider::inline_adjustment_slider_with_reset(
                    ui,
                    "develop-preview-zoom",
                    &mut slider_position,
                    0.0..=1.0,
                    slider_width,
                    5,
                    0.01,
                    Some("Preview zoom"),
                    reset_position,
                );
            if slider_interaction.changed {
                requested_zoom = Some(develop_slider_to_zoom(slider_position));
            }
            reset_requested |= slider_interaction.reset_requested;

            let displayed_zoom = if reset_requested {
                1.0
            } else {
                requested_zoom.unwrap_or(app.preview.zoom)
            };
            let zoom_text = format!("{:.0}%", displayed_zoom * 100.0);
            let readout_response = ui.add_sized(
                [readout_width, theme::CONTROL_HEIGHT],
                egui::Label::new(egui::RichText::new(zoom_text).monospace())
                    .sense(egui::Sense::click()),
            );
            if readout_response.double_clicked() {
                reset_requested = true;
            }

            icon_response
                .union(readout_response)
                .on_hover_text("Preview zoom. Double-click to reset to 100%.");
        });

        if reset_requested {
            if (app.preview.zoom - 1.0).abs() > f32::EPSILON || app.preview.center != [0.5, 0.5] {
                app.preview.zoom = 1.0;
                app.preview.center = [0.5, 0.5];
                app.note_preview_motion();
            }
        } else if let Some(zoom) = requested_zoom {
            let zoom = zoom.clamp(
                crate::ui::preview::MIN_PREVIEW_ZOOM,
                crate::ui::preview::MAX_PREVIEW_ZOOM,
            );
            if (app.preview.zoom - zoom).abs() > f32::EPSILON {
                app.preview.zoom = zoom;
                app.note_preview_motion();
            }
        }
    }

    #[cfg(not(target_os = "android"))]
    fn show_develop_review_and_zoom(
        ui: &mut Ui,
        app: &mut CalibRawApp,
        compact_review: bool,
        separator_after_zoom: bool,
    ) {
        if crate::ui::library::show_current_photo_review(ui, app, compact_review) {
            ui.separator();
        }
        if app.develop.current_path.is_some() {
            Self::show_develop_zoom_control(ui, app);
            if separator_after_zoom {
                ui.separator();
            }
        }
    }

    #[cfg(not(target_os = "android"))]
    fn show_desktop(ui: &mut Ui, app: &mut CalibRawApp, _frame: &eframe::Frame) {
        theme::prepare_toolbar(ui);
        let toolbar_width = ui.available_width();
        let compact = toolbar_width < TOOLBAR_COMPACT_WIDTH;
        let compact_review = toolbar_width < TOOLBAR_COMPACT_REVIEW_WIDTH;
        let center_brand = Self::toolbar_brand_can_be_centered(app.ui.active_tab, toolbar_width);
        // The three navigation tabs consume the space previously used by the
        // square app icon and its separator, keeping the Library sidebar alignment
        // essentially unchanged while giving each tab a wider hit target.
        let tab_width = if compact { 88.0 } else { 98.0 };
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if app.ui.active_tab == AppTab::Develop {
                // Keep the toolbar break aligned with the visible Develop dock on
                // the right so the top separator stays in the same vertical line as
                // the sidebar/tool-rail edge below it.
                let dock_geometry =
                    develop_toolbar_dock_geometry(ui.ctx(), app, ui.max_rect().right())
                        .filter(|(_, width)| *width >= DEVELOP_DOCK_MIN_WIDTH);
                if let Some((dock_separator_x, reserved_width)) = dock_geometry {
                    let dock_response = ui.allocate_ui_with_layout(
                        egui::vec2(reserved_width, theme::TOOLBAR_HEIGHT),
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            Self::show_develop_review_and_zoom(ui, app, compact_review, false);
                        },
                    );
                    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
                    // Do not infer this line from the toolbar child's rect: the
                    // toolbar frame has its own inset. Paint at the actual Panel
                    // edge loaded above so the line is pixel-identical below.
                    ui.painter().vline(
                        dock_separator_x,
                        dock_response.response.rect.y_range(),
                        stroke,
                    );
                } else {
                    Self::show_develop_review_and_zoom(ui, app, compact_review, true);
                }
                if !center_brand {
                    Self::show_toolbar_brand(ui, app);
                    ui.separator();
                }
            } else if !center_brand {
                // When there is not enough room to center safely, park the brand on
                // the right and reserve its width in layout. This prevents overlap
                // with Open Folder, search, task indicators, or the navigation tabs.
                Self::show_toolbar_brand(ui, app);
                ui.separator();
            }

            app.show_export_task_indicator(ui);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                for (tab, label) in [
                    (AppTab::Library, "Library"),
                    (AppTab::Develop, "Develop"),
                    (AppTab::Settings, "Settings"),
                ] {
                    if theme::tab_button(ui, label, app.ui.active_tab == tab, tab_width).clicked() {
                        app.activate_tab(tab);
                    }
                }

                let library_search_separator = ui.separator();
                ui.ctx().data_mut(|data| {
                    data.insert_temp(
                        egui::Id::new(LIBRARY_SIDEBAR_ALIGNMENT_ID),
                        library_search_separator.rect.center().x,
                    );
                });
                if app.ui.active_tab == AppTab::Library {
                    let search_width = if compact { 142.0 } else { 210.0 };
                    let focus_search = app.app_shortcuts_allowed(ui.ctx())
                        && ui.input(|input| {
                            input.modifiers.command && input.key_pressed(egui::Key::F)
                        });
                    let search_response = ui
                        .add_sized(
                            [search_width, theme::CONTROL_HEIGHT],
                            theme::singleline_text_edit(app.library.search_query_mut()).hint_text(
                                format!(
                                    "{} Search filenames…",
                                    egui_phosphor::regular::MAGNIFYING_GLASS
                                ),
                            ),
                        )
                        .on_hover_text(
                            "Filter by filename. Separate names with commas and press Enter to select every match (Ctrl/Cmd+F).",
                        );
                    if focus_search {
                        search_response.request_focus();
                    }
                    let select_matches = search_response.has_focus()
                        && ui.input_mut(|input| {
                            input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                        });
                    let clear_search = search_response.has_focus()
                        && ui.input_mut(|input| {
                            input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
                        });
                    if select_matches {
                        app.library.select_search_matches();
                    }
                    if clear_search {
                        app.library.clear_search();
                        search_response.surrender_focus();
                    }

                    let open = if compact {
                        crate::ui::icons::phosphor_icon_button(
                            ui,
                            egui_phosphor::regular::FOLDER_OPEN,
                            theme::toolbar_icon_size(),
                            "Open photo folder",
                        )
                    } else {
                        theme::toolbar_button(ui, "Open Folder…", 108.0)
                            .on_hover_text("Open photo folder")
                    };
                    if open.clicked() {
                        app.open_library_folder_dialog();
                    }
                }

                if app.ui.active_tab == AppTab::Develop {
                    Self::show_history_controls(ui, app, true);
                    Self::show_save_control(ui, app, true);
                    let original_visible = app.preview.original_visible();
                    let preview_icon = if original_visible {
                        egui_phosphor::regular::EYE
                    } else {
                        egui_phosphor::regular::EYE_SLASH
                    };
                    let preview_tooltip = if original_visible {
                        "Show edited preview"
                    } else {
                        "Show original preview"
                    };
                    if crate::ui::icons::phosphor_icon_toggle_button_enabled(
                        ui,
                        app.preview.gpu_pipeline.is_some(),
                        preview_icon,
                        original_visible,
                        theme::toolbar_icon_size(),
                        preview_tooltip,
                    )
                    .clicked()
                    {
                        app.toggle_original_preview();
                    }
                }
            });
        });

        if center_brand {
            Self::paint_centered_toolbar_brand(ui, app);
        }
    }
}

#[cfg(all(test, not(target_os = "android")))]
mod tests {
    use super::*;

    #[test]
    fn library_sidebar_width_aligns_the_painted_panel_separator() {
        let ctx = egui::Context::default();
        let theme = ctx.theme();
        let mut style = (*ctx.style_of(theme)).clone();
        style.visuals.widgets.noninteractive.bg_stroke.width = 2.0;
        ctx.set_style_of(theme, style);
        ctx.data_mut(|data| {
            data.insert_temp(egui::Id::new(LIBRARY_SIDEBAR_ALIGNMENT_ID), 410.0_f32);
        });

        assert_eq!(
            TopBar::library_sidebar_default_width(&ctx, 70.0),
            Some(341.0)
        );
    }

    #[test]
    fn develop_zoom_slider_keeps_fit_visible_and_round_trips_zoom() {
        assert!((develop_zoom_to_slider(1.0) - DEVELOP_ZOOM_FIT_POSITION).abs() < 1e-6);
        assert!((develop_slider_to_zoom(DEVELOP_ZOOM_FIT_POSITION) - 1.0).abs() < 1e-6);

        for zoom in [
            crate::ui::preview::MIN_PREVIEW_ZOOM,
            0.85,
            1.0,
            2.0,
            8.0,
            crate::ui::preview::MAX_PREVIEW_ZOOM,
        ] {
            let round_trip = develop_slider_to_zoom(develop_zoom_to_slider(zoom));
            assert!(
                (round_trip - zoom).abs() < 1e-4,
                "zoom={zoom}, got={round_trip}"
            );
        }
    }
}
