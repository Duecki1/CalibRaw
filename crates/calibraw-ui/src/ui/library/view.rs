use super::*;
use crate::app::AppTab;

pub(crate) struct Library;

impl Library {
    pub(crate) fn show_folder_sidebar(ui: &mut Ui, app: &mut CalibRawApp) {
        let action_in_progress = sources::local_action_in_progress(app);
        let folders_available = sources::local_folders_available(app);
        let can_create_folder = sources::can_create_local_folder(app);
        let mut requested_toolbar_action = None;

        moduwu_design::card_header(ui, |ui| {
            moduwu_design::toolbar_row(ui, |ui| {
                moduwu_design::toolbar_title(ui, "Folders");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if moduwu_design::ghost_icon_button(
                        ui,
                        egui_phosphor::regular::X,
                        moduwu_design::toolbar_icon_size(),
                        "Close folder sidebar",
                    )
                    .clicked()
                    {
                        app.set_library_folder_sidebar_open(false);
                    }
                    if ui
                        .add_enabled_ui(folders_available && !action_in_progress, |ui| {
                            moduwu_design::ghost_icon_button(
                                ui,
                                egui_phosphor::regular::ARROW_CLOCKWISE,
                                moduwu_design::toolbar_icon_size(),
                                "Refresh folders",
                            )
                        })
                        .inner
                        .clicked()
                    {
                        requested_toolbar_action = Some(LocalFolderToolbarAction::Refresh);
                    }
                    if ui
                        .add_enabled_ui(can_create_folder && !action_in_progress, |ui| {
                            moduwu_design::ghost_icon_button(
                                ui,
                                egui_phosphor::regular::FOLDER_PLUS,
                                moduwu_design::toolbar_icon_size(),
                                "Create folder here",
                            )
                        })
                        .inner
                        .clicked()
                    {
                        requested_toolbar_action = Some(LocalFolderToolbarAction::New);
                    }
                });
            });
        });
        moduwu_design::card_gap(ui);
        // One card fills the panel below the header, as wide as the header,
        // and the tree scrolls inside it.
        moduwu_design::content_card(ui, |ui| {
            let mut scroll_style = egui::style::ScrollStyle::solid();
            scroll_style.bar_width = 7.0;
            scroll_style.bar_inner_margin = 7.0;
            ui.spacing_mut().scroll = scroll_style;

            // While the scroll bar animates in or out, egui rounds the content
            // width up to whole pixels, so the area can report a fraction of a
            // pixel more than it was given. The resizable panel would persist
            // that as its width and creep wider on every folder toggle, so the
            // area gets a fixed rect and its overflow is not reported upward.
            let rect = ui.available_rect_before_wrap();
            let mut scroll_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            egui::ScrollArea::vertical()
                .id_salt("library-folder-sidebar-content")
                .auto_shrink([false, false])
                .show(&mut scroll_ui, |ui| {
                    sources::show_local_folder_tree(ui, app, action_in_progress);
                });
            ui.advance_cursor_after_rect(rect);
        });

        if let Some(action) = requested_toolbar_action {
            sources::apply_local_toolbar_action(app, action, ui.ctx());
        }
        sources::show_sidebar_dialogs(ui, app);
    }

    pub(crate) fn show(ui: &mut Ui, app: &mut CalibRawApp, frame: &eframe::Frame) {
        app.library.resume_thumbnail_decoding();
        app.library.poll(ui.ctx());

        let caps = app.caps();
        let mut refresh = false;
        let mut toggle_selecting = false;
        let mut import_raw = false;
        let mut open_asset: Option<LibraryAsset> = None;
        let mut library_action = None;

        let compact_header =
            moduwu_design::ResponsiveWidth::from_width(ui.available_width()).is_compact();
        let mut selected_sort = app.library.sort_order();
        let mut selected_size = app.library.thumbnail_size();
        let mut selected_filter = app.library.review_filter;
        let mut stack_raw_companions = app.library.stacks_raw_companions();
        let header_title = library_header_title(app);
        let thumbnail_progress = app.library.thumbnail_background_progress();
        let show_header = |ui: &mut Ui| {
            moduwu_design::toolbar_row(ui, |ui| {
                if compact_header {
                    ui.spacing_mut().item_spacing.x = moduwu_design::SPACE_XS;
                }
                if !app.library.folder_sidebar_open()
                    && moduwu_design::ghost_icon_button(
                        ui,
                        egui_phosphor::regular::SIDEBAR_SIMPLE,
                        moduwu_design::toolbar_icon_size(),
                        "Open folder sidebar",
                    )
                    .clicked()
                {
                    app.set_library_folder_sidebar_open(true);
                }

                if compact_header {
                    moduwu_design::toolbar_title(ui, "Library");
                } else {
                    moduwu_design::toolbar_title(ui, &header_title);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Mobile has no top bar; the library header carries its tasks and Settings.
                    if caps.is_mobile() {
                        app.show_background_tasks(ui);
                    }

                    if caps.is_mobile()
                        && (if compact_header {
                            moduwu_design::ghost_icon_button(
                                ui,
                                egui_phosphor::regular::GEAR,
                                moduwu_design::toolbar_icon_size(),
                                "Settings",
                            )
                        } else {
                            moduwu_design::toolbar_button(ui, "Settings", 82.0)
                        })
                        .clicked()
                    {
                        app.activate_tab(AppTab::Settings);
                    }

                    if ui
                        .add_enabled_ui(
                            app.library.location.is_some() && !app.library.scanning,
                            |ui| {
                                moduwu_design::ghost_icon_button(
                                    ui,
                                    egui_phosphor::regular::ARROW_CLOCKWISE,
                                    moduwu_design::toolbar_icon_size(),
                                    "Refresh library",
                                )
                            },
                        )
                        .inner
                        .clicked()
                    {
                        refresh = true;
                    }

                    let selecting = app.library.is_selecting();
                    if ui
                        .add_enabled_ui(!app.library.entries.is_empty(), |ui| {
                            moduwu_design::ghost_icon_toggle_button(
                                ui,
                                egui_phosphor::regular::CHECK_SQUARE_OFFSET,
                                selecting,
                                moduwu_design::toolbar_icon_size(),
                                "Select photos",
                            )
                        })
                        .inner
                        .clicked()
                    {
                        toggle_selecting = true;
                    }

                    if compact_header {
                        sort_filter_popup(
                            ui,
                            &mut selected_sort,
                            &mut selected_filter,
                            &mut stack_raw_companions,
                            Some(&mut selected_size),
                            64.0,
                        );
                    } else {
                        show_library_view_combos(
                            ui,
                            &mut selected_sort,
                            &mut selected_size,
                            &mut selected_filter,
                            &mut stack_raw_companions,
                        );
                    }
                });
            });
        };
        if let Some(progress) = thumbnail_progress {
            let label = if compact_header {
                format!("{}/{}", progress.completed, progress.total)
            } else {
                format!("Previews {}/{}", progress.completed, progress.total)
            };
            moduwu_design::progress_card_header(
                ui,
                progress.completed as f32 / progress.total.max(1) as f32,
                &label,
                show_header,
            );
        } else {
            moduwu_design::card_header(ui, show_header);
        }
        app.set_library_sort_order(selected_sort);
        app.set_library_thumbnail_size(selected_size);
        app.library.review_filter = selected_filter;
        app.set_library_stack_raw_companions(stack_raw_companions);
        if toggle_selecting {
            if app.library.is_selecting() {
                app.library.clear_selection();
            } else {
                app.library.begin_selection();
            }
        }
        app.library
            .follow_document(super::review::current_document_id(app));
        let visible = app.library.visible_entry_indices();
        app.library.retain_visible_selection(&visible);
        if app.library.review_filter.active() {
            ui.horizontal_wrapped(|ui| {
                ui.weak(format!(
                    "Showing {} of {} · {}",
                    visible.len(),
                    app.library.photo_count(),
                    app.library.review_filter.summary()
                ));
                if ui.small_button("Clear filters").clicked() {
                    app.library.review_filter = LibraryReviewFilter::default();
                }
            });
        }

        moduwu_design::card_gap(ui);

        if app.library.location.is_none() {
            if caps.has_folders() {
                if show_library_empty_state(
                    ui,
                    "Build your photo library",
                    "Choose a top-level photo folder. CalibRaw keeps your hierarchy intact and shows the RAW, JPEG, PNG and HEIC photos in each folder.",
                    Some("Open Folder…"),
                ) {
                    app.open_library_folder_dialog();
                }
            } else {
                show_library_empty_state(
                    ui,
                    "Your library is ready",
                    "Use the folder sidebar to browse your Library, or tap + to import photos.",
                    None,
                );
            }
        } else if app.library.scanning && !app.library.catalog_ready {
            show_library_loading_state(ui);
        } else if app.library.catalog_ready && app.library.entries.is_empty() {
            show_library_empty_state(
                ui,
                "No photos here yet",
                if caps.has_folders() {
                    "Choose another folder in the sidebar or add RAW, JPEG, PNG or HEIC photos to this folder."
                } else {
                    "Tap + to import RAW, JPEG, PNG or HEIC photos."
                },
                None,
            );
        } else if visible.is_empty() {
            if show_library_empty_state(
                ui,
                "No matching photos",
                "Change the filename, rating or flag filters to show more photos.",
                Some("Clear search & filters"),
            ) {
                app.library.clear_search();
                app.library.review_filter = LibraryReviewFilter::default();
            }
        } else {
            show_thumbnail_grid(ui, app, visible, &mut open_asset, &mut library_action);
        }

        show_library_selection_action_bar(ui, app, &mut library_action);

        if let Some(action) = library_action {
            apply_library_action(ui, app, frame, action);
        }

        sources::show_page_dialogs(ui, app);
        show_library_action_overlays(ui, app, frame);

        // Photos are imported into a media library with the floating + button.
        if caps.has_media_library() && !app.library.is_selecting() {
            let rect = moduwu_design::floating_action_rect(ui.max_rect());
            let response = moduwu_design::floating_action_button(
                ui,
                rect,
                egui_phosphor::regular::PLUS,
                "Import RAW files",
            );
            if response.clicked() {
                import_raw = true;
            }
        }

        if refresh {
            app.library.refresh(ui.ctx());
        }
        if import_raw {
            app.open_file_dialog(frame);
        }
        if let Some(asset) = open_asset {
            match &asset.locator {
                LibraryLocator::Desktop(path) => app.open_path(path.clone(), frame),
                LibraryLocator::Android { uri } => {
                    app.library.clear_selection();
                    app.open_android_library_document(uri, &asset.display_name);
                }
            }
        }
    }
}

fn library_header_title(app: &CalibRawApp) -> String {
    selected_library_folder_name(app).unwrap_or_else(|| "Library".to_owned())
}

fn selected_library_folder_name(app: &CalibRawApp) -> Option<String> {
    if app.caps().has_media_library() {
        let folder = app.library.media.folder.as_str();
        return Some(if folder.is_empty() {
            "All photos".to_owned()
        } else {
            folder.rsplit('/').next().unwrap_or(folder).to_owned()
        });
    }
    app.library.folder.as_deref().map(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| path.display().to_string())
    })
}

/// How close to the grid's top or bottom edge a selecting drag scrolls it,
/// and how fast at the very edge, in points per second.
const DRAG_SCROLL_EDGE: f32 = 56.0;
const DRAG_SCROLL_SPEED: f32 = 900.0;

/// The justified thumbnail grid of the `visible` entries: opening, selecting,
/// keyboard moves, context menus and long-press drags.
fn show_thumbnail_grid(
    ui: &mut Ui,
    app: &mut CalibRawApp,
    mut visible: Vec<usize>,
    open_asset: &mut Option<LibraryAsset>,
    // Set from thumbnail context menus, which exist on desktop only.
    library_action: &mut Option<LibraryAction>,
) {
    let caps = app.caps();
    let available = ui.available_width().max(1.0);
    let available_height = ui.available_height().max(1.0);
    let gap = moduwu_design::SPACE_SM;
    let target_thumbnail_height = responsive_thumbnail_target_height(
        available,
        available_height,
        ui.ctx().pixels_per_point(),
        caps.is_mobile(),
    ) * app.library.thumbnail_size().scale();
    let layout = |entries: &[LibraryEntry], visible: &[usize]| {
        justified_thumbnail_layout(entries, visible, available, target_thumbnail_height, gap)
    };
    let (mut placements, mut grid_height) = layout(&app.library.entries, &visible);

    if app.navigation_shortcuts_allowed(ui.ctx()) {
        if let Some(key) = take_grid_key(ui.ctx()) {
            apply_grid_key(
                app,
                key,
                &visible,
                &placements,
                available_height,
                open_asset,
                library_action,
            );
            // A rating or flag can re-sort the photos.
            visible = app.library.visible_entry_indices();
            (placements, grid_height) = layout(&app.library.entries, &visible);
        }
    }

    let reveal = app
        .library
        .take_reveal_active(&visible)
        .and_then(|position| placements.get(position).copied());
    let selecting = app.library.is_selecting();
    let active = app
        .library
        .active_asset_id()
        .filter(|_| app.library.shows_active_photo(caps))
        .cloned();
    let drag_selecting = app.library.drag_selecting();
    let touch_hold = moduwu_design::touch_hold(ui.ctx());
    // Touch shows no hover; its press must not grow a check under the finger.
    let pointer_hovers = caps.is_desktop() && !ui.input(|input| input.any_touches());
    let mut scroll_source = egui::scroll_area::ScrollSource::default();
    if drag_selecting {
        scroll_source.drag = egui::scroll_area::DragScroll::Never;
    }
    let mut protected_thumbnail_indices = HashSet::new();
    egui::ScrollArea::vertical()
        .wheel_scroll_multiplier(egui::vec2(
            1.0,
            app.library.thumbnail_size().wheel_scroll_multiplier(),
        ))
        .scroll_source(scroll_source)
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, viewport| {
            let (content_rect, _) =
                ui.allocate_exact_size(egui::vec2(available, grid_height.max(1.0)), Sense::hover());
            if let Some(rect) = reveal {
                ui.scroll_to_rect(rect.translate(content_rect.min.to_vec2()), None);
            }
            let preload_viewport = viewport.expand(600.0);
            let pointer = ui.input(|input| input.pointer.interact_pos());
            let mut drag_target = None;

            for (position, relative_rect) in placements.iter().copied().enumerate() {
                if !relative_rect.intersects(preload_viewport) {
                    continue;
                }
                let index = visible[position];
                protected_thumbnail_indices.insert(index);
                app.library.touch_and_request_thumbnail(index, ui.ctx());
                if !relative_rect.intersects(viewport) {
                    continue;
                }
                let item_rect = relative_rect.translate(content_rect.min.to_vec2());
                let entry = &app.library.entries[index];
                let asset_id = entry.asset.id.clone();
                let selected = app.library.is_selected(&asset_id);
                let tile = thumbnail_tile(ui, entry, item_rect, selecting.then_some(selected));
                let response = tile.response;
                // Browsing with a pointer, hovering shows the details and
                // rating controls; otherwise the review badge stays.
                if pointer_hovers && !selecting {
                    if let Some(action) = thumbnail_hover_overlay(
                        ui,
                        item_rect,
                        entry,
                        app.library.stacked_formats(&asset_id),
                        tile.corner_badges_bottom,
                    ) {
                        *library_action = Some(action);
                        continue;
                    }
                } else {
                    paint_review_badge(ui, item_rect, entry.review());
                }
                paint_thumbnail_selection(
                    ui,
                    item_rect,
                    selected,
                    active.as_ref() == Some(&asset_id),
                );
                let check_clicked = (selecting
                    || (pointer_hovers && ui.rect_contains_pointer(item_rect)))
                    && thumbnail_check_circle(ui, entry, item_rect, selected).clicked();

                if drag_selecting && pointer.is_some_and(|pointer| item_rect.contains(pointer)) {
                    drag_target = Some(asset_id.clone());
                }
                let long_pressed = touch_hold
                    .long_press_at
                    .is_some_and(|origin| item_rect.contains(origin));
                // Lifting a finger after a long press is no tap: the long
                // press already selected.
                let tapped = !touch_hold.suppress_tap;
                if long_pressed {
                    app.library.begin_drag_selection(&asset_id);
                } else if tapped && check_clicked {
                    app.library.toggle_selected(&asset_id);
                } else if tapped && response.clicked() {
                    let modifiers = ui.input(|input| input.modifiers);
                    if modifiers.shift {
                        app.library.select_range_to(&asset_id, &visible);
                    } else if modifiers.command || modifiers.ctrl || selecting {
                        app.library.toggle_selected(&asset_id);
                    } else {
                        *open_asset = Some(app.library.entries[index].asset.clone());
                    }
                }
                // egui opens menus on its own, longer hold; a long press
                // that selected has done its job.
                if caps.is_desktop() && tapped {
                    show_thumbnail_context_menu(app, &response, index, library_action);
                }
            }

            if drag_selecting {
                if let Some(target) = drag_target {
                    app.library.drag_selection_to(&target, &visible);
                }
                scroll_near_edges(ui, pointer);
            }
        });
    if drag_selecting && !ui.input(|input| input.pointer.any_down()) {
        app.library.end_drag_selection();
    }
    app.library.evict_old_textures(&protected_thumbnail_indices);
}

/// Right-click on a thumbnail. While selecting, the menu applies to the
/// selection, which a right-clicked photo joins; otherwise to that photo.
fn show_thumbnail_context_menu(
    app: &mut CalibRawApp,
    response: &egui::Response,
    index: usize,
    library_action: &mut Option<LibraryAction>,
) {
    if response.secondary_clicked() && app.library.is_selecting() {
        let id = app.library.entries[index].asset.id.clone();
        if !app.library.is_selected(&id) {
            app.library.add_to_selection(&id);
        }
    }
    moduwu_design::context_menu(response, |ui| {
        let anchor = &app.library.entries[index].asset;
        let assets = if app.library.is_selecting() {
            app.library.selected_assets()
        } else {
            vec![anchor.clone()]
        };
        if let Some(action) = photo_actions_menu(ui, app, &assets, Some(anchor)) {
            *library_action = Some(action);
        }
    });
}

/// Scrolls the grid while a selecting drag holds the finger near its top
/// or bottom edge, faster the closer it is.
fn scroll_near_edges(ui: &mut Ui, pointer: Option<egui::Pos2>) {
    let Some(pointer) = pointer else {
        return;
    };
    let visible = ui.clip_rect();
    let toward_top = (visible.top() + DRAG_SCROLL_EDGE - pointer.y) / DRAG_SCROLL_EDGE;
    let toward_bottom = (pointer.y - (visible.bottom() - DRAG_SCROLL_EDGE)) / DRAG_SCROLL_EDGE;
    // Positive moves the content down, showing what is above.
    let direction = if toward_top > 0.0 {
        toward_top.min(1.0)
    } else if toward_bottom > 0.0 {
        -toward_bottom.min(1.0)
    } else {
        return;
    };
    let step = direction * DRAG_SCROLL_SPEED * ui.input(|input| input.stable_dt);
    ui.scroll_with_delta(egui::vec2(0.0, step));
    ui.ctx().request_repaint();
}

fn show_library_loading_state(ui: &mut Ui) {
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            ui.add(egui::Spinner::new().size(28.0));
            ui.add_space(moduwu_design::SPACE_SM);
            ui.label("Loading photos…");
        });
    });
}

fn show_library_empty_state(
    ui: &mut Ui,
    title: &str,
    description: &str,
    action: Option<&str>,
) -> bool {
    let mut action_clicked = false;
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            let text_width = ui.available_width().clamp(220.0, 420.0);
            ui.label(
                egui::RichText::new(title)
                    .strong()
                    .size(moduwu_design::PANEL_TITLE_TEXT_SIZE),
            );
            ui.add_space(moduwu_design::SPACE_XS);
            ui.add_sized(
                [text_width, 0.0],
                egui::Label::new(
                    egui::RichText::new(description).color(ui.visuals().weak_text_color()),
                )
                .wrap()
                .halign(egui::Align::Center),
            );
            if let Some(action) = action {
                ui.add_space(moduwu_design::SPACE_MD);
                action_clicked = moduwu_design::primary_button(ui, action, 132.0).clicked();
            }
        });
    });
    action_clicked
}

fn show_library_view_combos(
    ui: &mut Ui,
    selected_sort: &mut LibrarySortOrder,
    selected_size: &mut LibraryThumbnailSize,
    selected_filter: &mut LibraryReviewFilter,
    stack_raw_companions: &mut bool,
) {
    const SORT_WIDTH: f32 = 154.0;
    const SIZE_WIDTH: f32 = 118.0;
    let available_width = ui.available_width().max(1.0);
    let minimum_width = SORT_WIDTH + moduwu_design::SPACE_SM + SIZE_WIDTH;
    let (sort_width, size_width) = if available_width < minimum_width {
        let width = ((available_width - moduwu_design::SPACE_SM).max(2.0)) * 0.5;
        (width, width)
    } else {
        (SORT_WIDTH, SIZE_WIDTH)
    };

    sort_filter_popup(
        ui,
        selected_sort,
        selected_filter,
        stack_raw_companions,
        None,
        sort_width,
    );
    moduwu_design::responsive_combo_box(
        ui,
        "library-thumbnail-size",
        format!("Size: {}", selected_size.label()),
        size_width,
        LibraryThumbnailSize::ALL.len(),
        |ui| {
            for thumbnail_size in LibraryThumbnailSize::ALL {
                ui.selectable_value(selected_size, thumbnail_size, thumbnail_size.label());
            }
        },
    );
}
