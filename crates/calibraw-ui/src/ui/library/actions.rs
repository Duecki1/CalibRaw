use super::*;

const SELECTION_BAR_COUNT_LABEL_BREAKPOINT: f32 = 360.0;

#[derive(Clone, Debug)]
pub(crate) enum LibraryAction {
    Review(Vec<LibraryAsset>, super::review::ReviewChange),
    Export(Vec<LibraryAsset>),
    Merge(super::MergeKind, Vec<LibraryAsset>),
    CopyAdjustments(LibraryAsset),
    PasteAdjustments(Vec<LibraryAsset>),
    ApplyPreset(Vec<LibraryAsset>, PathBuf),
    Copy(Vec<LibraryAsset>),
    Cut(Vec<LibraryAsset>),
    PasteIntoAssetFolder(LibraryAsset),
    Duplicate(Vec<LibraryAsset>),
    Rename(LibraryAsset),
    RevealFile(LibraryAsset),
    ResetAdjustments(Vec<LibraryAsset>),
    Delete(Vec<LibraryAsset>),
}

fn duplicate_label(count: usize) -> &'static str {
    if count > 1 {
        "Duplicate selected (RAW + sidecars)"
    } else {
        "Duplicate (RAW + sidecar)"
    }
}

fn reset_adjustments_label(count: usize) -> &'static str {
    if count > 1 {
        "Reset adjustments for selected"
    } else {
        "Reset all adjustments"
    }
}

fn delete_label(count: usize) -> &'static str {
    if count > 1 {
        "Delete selected…"
    } else {
        "Delete…"
    }
}

/// Whether photo actions can start: no file action, batch export or AI mask
/// refresh is running.
pub(super) fn library_actions_enabled(app: &CalibRawApp) -> bool {
    !local_action_in_progress(app)
        && app.library_batch_export_progress().is_none()
        && app.library_ai_mask_refresh_status().is_none()
}

pub(crate) fn library_image_context_menu(
    ui: &mut Ui,
    app: &CalibRawApp,
    context_asset: &LibraryAsset,
    context_assets: &[LibraryAsset],
    show_review: bool,
) -> Option<LibraryAction> {
    let selected_count = context_assets.len();
    let action_enabled = library_actions_enabled(app) && !context_assets.is_empty();
    let mut action = None;
    if show_review {
        ui.add_enabled_ui(action_enabled, |ui| {
            ui.menu_button("Flag", |ui| {
                for (label, flag) in [
                    ("Pick", crate::sidecar::PhotoFlag::Picked),
                    ("Unflag", crate::sidecar::PhotoFlag::Unflagged),
                    ("Reject", crate::sidecar::PhotoFlag::Rejected),
                ] {
                    if moduwu_design::menu_item(ui, true, label).clicked() {
                        action = Some(LibraryAction::Review(
                            context_assets.to_vec(),
                            super::review::ReviewChange::Flag(flag),
                        ));
                        ui.close();
                    }
                }
            });
            ui.menu_button("Rating", |ui| {
                for rating in 0..=5 {
                    let label = if rating == 0 {
                        "Unrated".to_owned()
                    } else {
                        format!("{rating} stars")
                    };
                    if moduwu_design::menu_item(ui, true, label).clicked() {
                        action = Some(LibraryAction::Review(
                            context_assets.to_vec(),
                            super::review::ReviewChange::Rating(rating),
                        ));
                        ui.close();
                    }
                }
            });
        });
        ui.separator();
    }

    if moduwu_design::context_menu_item(
        ui,
        action_enabled,
        if selected_count > 1 {
            "Export selected…"
        } else {
            "Export…"
        },
    )
    .clicked()
    {
        action = Some(LibraryAction::Export(context_assets.to_vec()));
        ui.close();
    }

    if moduwu_design::context_menu_item(
        ui,
        app.caps().is_desktop() && selected_count == 1 && context_asset.desktop_path().is_some(),
        "Show in File Manager",
    )
    .clicked()
    {
        action = Some(LibraryAction::RevealFile(context_asset.clone()));
        ui.close();
    }

    if selected_count >= 2 {
        for (kind, help) in [
            (
                super::MergeKind::Hdr,
                "Auto-align and merge RAW exposures into an editable 32-bit float TIFF",
            ),
            (
                super::MergeKind::FocusStack,
                "Auto-align RAW frames focused at different distances and combine their sharpest parts into an editable 32-bit float TIFF",
            ),
        ] {
            if moduwu_design::context_menu_item(ui, action_enabled, kind.title())
                .on_hover_text(help)
                .clicked()
            {
                action = Some(LibraryAction::Merge(kind, context_assets.to_vec()));
                ui.close();
            }
        }
    }

    ui.separator();
    if moduwu_design::context_menu_item(
        ui,
        action_enabled && selected_count == 1,
        "Copy adjustments",
    )
    .clicked()
    {
        action = Some(LibraryAction::CopyAdjustments(context_asset.clone()));
        ui.close();
    }
    if moduwu_design::context_menu_item(
        ui,
        action_enabled && app.library.has_copied_adjustments(),
        if selected_count > 1 {
            "Paste adjustments to selected"
        } else {
            "Paste adjustments"
        },
    )
    .on_disabled_hover_text("Copy adjustments from an image first")
    .clicked()
    {
        action = Some(LibraryAction::PasteAdjustments(context_assets.to_vec()));
        ui.close();
    }
    if let Some(preset) = apply_preset_menu(ui, app, action_enabled) {
        action = Some(LibraryAction::ApplyPreset(context_assets.to_vec(), preset));
    }

    ui.separator();
    if moduwu_design::context_menu_item(ui, action_enabled, "Copy").clicked() {
        action = Some(LibraryAction::Copy(context_assets.to_vec()));
        ui.close();
    }
    if moduwu_design::context_menu_item(ui, action_enabled, "Cut").clicked() {
        action = Some(LibraryAction::Cut(context_assets.to_vec()));
        ui.close();
    }
    let paste_label = app
        .library
        .image_clipboard
        .as_ref()
        .map_or_else(|| "Paste here".to_owned(), ImageClipboard::paste_label);
    if moduwu_design::context_menu_item(
        ui,
        action_enabled && app.library.image_clipboard.is_some(),
        paste_label,
    )
    .on_disabled_hover_text("Copy or cut photos first")
    .clicked()
    {
        action = Some(LibraryAction::PasteIntoAssetFolder(context_asset.clone()));
        ui.close();
    }
    if moduwu_design::context_menu_item(ui, action_enabled, duplicate_label(selected_count))
        .clicked()
    {
        action = Some(LibraryAction::Duplicate(context_assets.to_vec()));
        ui.close();
    }
    if moduwu_design::context_menu_item(ui, action_enabled && selected_count == 1, "Rename…")
        .clicked()
    {
        action = Some(LibraryAction::Rename(context_asset.clone()));
        ui.close();
    }
    if moduwu_design::context_menu_item(
        ui,
        action_enabled,
        format!(
            "{}  {}",
            egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
            reset_adjustments_label(selected_count)
        ),
    )
    .clicked()
    {
        action = Some(LibraryAction::ResetAdjustments(context_assets.to_vec()));
        ui.close();
    }
    ui.separator();
    if moduwu_design::context_menu_item(ui, action_enabled, delete_label(selected_count)).clicked()
    {
        action = Some(LibraryAction::Delete(context_assets.to_vec()));
        ui.close();
    }
    action
}

pub(crate) fn apply_library_action(
    ui: &mut Ui,
    app: &mut CalibRawApp,
    frame: &eframe::Frame,
    action: LibraryAction,
) {
    // Every other action reads, writes, moves or deletes the photos' files.
    if !matches!(action, LibraryAction::Review(..)) {
        app.settle_library_review_saves();
    }
    match action {
        LibraryAction::Merge(kind, assets) => {
            super::merge::start(app, frame, kind, assets, ui.ctx())
        }
        LibraryAction::Review(assets, change) => {
            app.review_photos(assets, change);
        }
        LibraryAction::Export(assets) => {
            if !assets.is_empty() {
                app.library.export_dialog = Some(LibraryExportDialog {
                    assets,
                    settings: app.export.settings.clone(),
                    format: app.export.format,
                    overrides: app.export.override_choice.clone(),
                });
            }
        }
        LibraryAction::CopyAdjustments(asset) => {
            app.library.status = match app.copy_library_adjustments(&asset) {
                Ok(()) => format!("Copied adjustments from {}", asset.display_name),
                Err(error) => format!("Could not copy adjustments: {error}"),
            };
        }
        LibraryAction::PasteAdjustments(assets) => {
            let (edited_count, failures) = app.library_adjustment_edit_count(&assets);
            if !failures.is_empty() {
                app.library.status = format!(
                    "Could not inspect selected adjustments. {}",
                    failures.join(" · ")
                );
            } else if edited_count > 0 {
                app.library.adjustment_paste_dialog = Some(LibraryAdjustmentPasteDialog {
                    assets,
                    edited_count,
                });
            } else {
                apply_library_adjustment_paste(
                    app,
                    assets,
                    crate::sidecar::AdjustmentPasteMode::Merge,
                    ui.ctx(),
                    frame,
                );
            }
        }
        LibraryAction::ApplyPreset(assets, preset) => {
            apply_library_preset(app, assets, &preset, ui.ctx(), frame);
        }
        LibraryAction::Copy(assets) => {
            set_library_clipboard(app, ImageClipboardMode::Copy, assets);
        }
        LibraryAction::Cut(assets) => {
            set_library_clipboard(app, ImageClipboardMode::Cut, assets);
        }
        LibraryAction::PasteIntoAssetFolder(asset) => match duplicate_destination(&asset) {
            Ok(destination) => start_image_clipboard_paste(app, destination, ui.ctx()),
            Err(error) => app.library.status = format!("Could not determine paste folder: {error}"),
        },
        LibraryAction::Duplicate(assets) => {
            app.library.clear_selection();
            start_duplicate_assets(app, &assets, ui.ctx());
        }
        LibraryAction::Rename(asset) => {
            app.library.raw_name_dialog = Some(LibraryRawNameDialog {
                name: asset.display_name.clone(),
                asset,
                error: None,
                focus_requested: false,
            });
        }
        LibraryAction::RevealFile(asset) => {
            if let Some(path) = asset.desktop_path() {
                if let Err(error) = app.platform.reveal_file(path) {
                    app.library.status = error;
                }
            }
        }
        LibraryAction::ResetAdjustments(assets) => {
            let current_to_reopen = app.develop.current_path.as_ref().and_then(|current| {
                assets
                    .iter()
                    .find(|asset| asset.desktop_path() == Some(current.as_path()))
                    .and_then(|asset| asset.desktop_path().map(Path::to_path_buf))
            });
            if let Some(path) = current_to_reopen.as_deref() {
                app.detach_current_file_for_library_action(path);
            }

            let total = assets.len();
            let mut changed = 0usize;
            let mut failures = Vec::new();
            for asset in &assets {
                match reset_asset_adjustments(app, asset) {
                    Ok(reset) => {
                        changed += usize::from(reset);
                        app.library.invalidate_adjustment_thumbnail_for_asset(asset);
                    }
                    Err(error) => failures.push(format!("{}: {error}", asset.display_name)),
                }
            }
            app.library.clear_selection();
            app.library.refresh(ui.ctx());
            app.library.status = if failures.is_empty() {
                format!(
                    "Cleared all adjustments for {total} selected {} ({changed} changed)",
                    if total == 1 { "image" } else { "images" }
                )
            } else {
                format!(
                    "Cleared all adjustments for {} of {total} selected images. {}",
                    total.saturating_sub(failures.len()),
                    failures.join(" · ")
                )
            };
            if let Some(path) = current_to_reopen {
                app.reload_desktop_library_document_after_reset(path, frame);
            }
        }
        LibraryAction::Delete(assets) => {
            if !assets.is_empty() {
                app.library.delete_originals_confirmation = Some(assets);
            }
        }
    }
}

/// An "Apply preset" submenu listing every preset. Returns the chosen preset.
fn apply_preset_menu(ui: &mut Ui, app: &CalibRawApp, enabled: bool) -> Option<PathBuf> {
    presets_submenu(ui, &crate::ui::presets::PresetList::from_app(app), enabled)
}

fn presets_submenu(
    ui: &mut Ui,
    presets: &crate::ui::presets::PresetList,
    enabled: bool,
) -> Option<PathBuf> {
    let mut chosen = None;
    ui.add_enabled_ui(enabled && !presets.is_empty(), |ui| {
        let menu = ui.menu_button("Apply preset", |ui| presets.show_menu(ui));
        chosen = menu.inner.flatten();
        menu.response
            .on_disabled_hover_text("Create a preset in the editor's Presets tab first");
    });
    chosen
}

fn delete_confirmed_library_assets(ui: &Ui, app: &mut CalibRawApp, assets: Vec<LibraryAsset>) {
    let total = assets.len();
    let mut deleted = 0usize;
    let mut failures = Vec::new();
    for asset in &assets {
        match delete_library_asset(app, asset) {
            Ok(()) => deleted += 1,
            Err(error) => failures.push(format!("{}: {error}", asset.display_name)),
        }
    }
    app.library.clear_selection();
    app.library.refresh(ui.ctx());
    app.library.status = if failures.is_empty() {
        if app.caps().has_folders() {
            format!(
                "Moved {deleted} selected {} to the {}",
                if deleted == 1 {
                    "original"
                } else {
                    "originals"
                },
                crate::platform::trash_name()
            )
        } else {
            format!(
                "Deleted {deleted} selected {}",
                if deleted == 1 {
                    "original"
                } else {
                    "originals"
                }
            )
        }
    } else {
        format!(
            "Removed {deleted} of {total} selected originals. {}",
            failures.join(" · ")
        )
    };
}

fn set_library_clipboard(
    app: &mut CalibRawApp,
    mode: ImageClipboardMode,
    assets: Vec<LibraryAsset>,
) {
    let count = assets.len();
    app.library.image_clipboard = Some(ImageClipboard { mode, assets });
    app.library.folder_clipboard = None;
    app.library.clear_selection();
    app.library.status = format!(
        "{} {count} photo{}. Choose Paste in a Library folder.",
        if mode == ImageClipboardMode::Copy {
            "Copied"
        } else {
            "Cut"
        },
        if count == 1 { "" } else { "s" }
    );
}

/// A photo action on the selection bar: a button while the bar has room,
/// otherwise an item in its More menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BarAction {
    Export,
    Merge(super::MergeKind),
    CopyAdjustments,
    PasteAdjustments,
    Copy,
}

impl BarAction {
    /// The actions for `count` selected photos, in the order they give up
    /// their buttons last to first.
    fn available(count: usize, caps: PlatformCaps) -> Vec<Self> {
        let mut actions = vec![Self::Export];
        if count >= 2 {
            actions.extend([
                Self::Merge(super::MergeKind::Hdr),
                Self::Merge(super::MergeKind::FocusStack),
            ]);
        }
        if count == 1 {
            actions.push(Self::CopyAdjustments);
        }
        actions.push(Self::PasteAdjustments);
        // Copying and cutting work on photos in folders.
        if caps.has_folders() {
            actions.push(Self::Copy);
        }
        actions
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Export => egui_phosphor::regular::EXPORT,
            Self::Merge(super::MergeKind::Hdr) => egui_phosphor::regular::STACK,
            Self::Merge(super::MergeKind::FocusStack) => egui_phosphor::regular::CROSSHAIR_SIMPLE,
            Self::CopyAdjustments => egui_phosphor::regular::SLIDERS_HORIZONTAL,
            Self::PasteAdjustments => egui_phosphor::regular::CLIPBOARD_TEXT,
            Self::Copy => egui_phosphor::regular::COPY,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Export => "Export",
            Self::Merge(kind) => kind.title(),
            Self::CopyAdjustments => "Copy adjustments",
            Self::PasteAdjustments => "Paste adjustments",
            Self::Copy => "Copy",
        }
    }

    fn labeled_text(self) -> String {
        format!("{}  {}", self.glyph(), self.label())
    }

    fn enabled(self, can_paste_adjustments: bool) -> bool {
        self != Self::PasteAdjustments || can_paste_adjustments
    }

    fn action(self, assets: &[LibraryAsset]) -> Option<LibraryAction> {
        Some(match self {
            Self::Export => LibraryAction::Export(assets.to_vec()),
            Self::Merge(kind) => LibraryAction::Merge(kind, assets.to_vec()),
            Self::CopyAdjustments => LibraryAction::CopyAdjustments(assets.first()?.clone()),
            Self::PasteAdjustments => LibraryAction::PasteAdjustments(assets.to_vec()),
            Self::Copy => LibraryAction::Copy(assets.to_vec()),
        })
    }
}

/// How the bar shows its photo actions in the width it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BarLayout {
    /// Every action as a labelled button.
    Labeled,
    /// The first `inline` actions as icons; the others in More.
    Icons { inline: usize },
}

/// The More button's text while the bar shows labels.
fn bar_more_text() -> String {
    format!("{}  More", egui_phosphor::regular::DOTS_THREE)
}
/// egui's default separator width, fixed here so the bar can measure itself.
const BAR_SEPARATOR_SPACING: f32 = 6.0;
/// Keeps the bar this far from the library's edges.
const BAR_OUTER_MARGIN: f32 = 12.0;

/// The width a widget showing `text` in `style` needs, padding included.
fn text_width(ui: &Ui, text: String, style: egui::TextStyle, padding: f32) -> f32 {
    let font = style.resolve(ui.style());
    ui.painter()
        .layout_no_wrap(text, font, Color32::WHITE)
        .size()
        .x
        + 2.0 * padding
}

/// Labels when all of them fit in `room`, otherwise as many icons as fit
/// beside the More button.
fn selection_bar_layout(ui: &Ui, actions: &[BarAction], room: f32) -> BarLayout {
    let spacing = ui.spacing().item_spacing.x;
    let padding = ui.spacing().button_padding.x;
    let labeled_width =
        |text: String| text_width(ui, text, egui::TextStyle::Button, padding) + spacing;
    let more = labeled_width(bar_more_text());
    let labeled = actions
        .iter()
        .map(|action| labeled_width(action.labeled_text()))
        .sum::<f32>();
    if labeled + more <= room {
        return BarLayout::Labeled;
    }
    let icon = moduwu_design::toolbar_icon_size().x + spacing;
    let fitting = ((room - icon) / icon).floor().max(0.0) as usize;
    BarLayout::Icons {
        inline: fitting.min(actions.len()),
    }
}

fn selection_bar_action_button(
    ui: &mut Ui,
    action: BarAction,
    enabled: bool,
    labeled: bool,
) -> egui::Response {
    let response = if labeled {
        moduwu_design::secondary_button_enabled(ui, enabled, action.labeled_text())
            .on_hover_text(action.label())
    } else {
        ui.add_enabled_ui(enabled, |ui| {
            moduwu_design::ghost_icon_button(
                ui,
                action.glyph(),
                moduwu_design::toolbar_icon_size(),
                action.label(),
            )
        })
        .inner
    };
    if action == BarAction::PasteAdjustments {
        response.on_disabled_hover_text("Copy adjustments from an image first")
    } else {
        response
    }
}

/// The More menu: the actions without a button, then the rest.
fn selection_bar_more_menu(
    ui: &mut Ui,
    assets: &[LibraryAsset],
    overflow: &[BarAction],
    can_paste_adjustments: bool,
    presets: &crate::ui::presets::PresetList,
    caps: PlatformCaps,
) -> Option<LibraryAction> {
    let count = assets.len();
    let mut action = None;
    for bar_action in overflow {
        if moduwu_design::menu_item(
            ui,
            bar_action.enabled(can_paste_adjustments),
            bar_action.labeled_text(),
        )
        .clicked()
        {
            action = bar_action.action(assets);
            ui.close();
        }
    }
    if !overflow.is_empty() {
        ui.separator();
    }
    if let Some(preset) = presets_submenu(ui, presets, true) {
        action = Some(LibraryAction::ApplyPreset(assets.to_vec(), preset));
    }
    if caps.has_folders() && moduwu_design::menu_item(ui, true, "Cut").clicked() {
        action = Some(LibraryAction::Cut(assets.to_vec()));
        ui.close();
    }
    if moduwu_design::menu_item(ui, true, duplicate_label(count)).clicked() {
        action = Some(LibraryAction::Duplicate(assets.to_vec()));
        ui.close();
    }
    if let [asset] = assets {
        if moduwu_design::menu_item(ui, true, "Rename…").clicked() {
            action = Some(LibraryAction::Rename(asset.clone()));
            ui.close();
        }
    }
    if moduwu_design::menu_item(ui, true, reset_adjustments_label(count)).clicked() {
        action = Some(LibraryAction::ResetAdjustments(assets.to_vec()));
        ui.close();
    }
    ui.separator();
    if moduwu_design::menu_item(ui, true, delete_label(count)).clicked() {
        action = Some(LibraryAction::Delete(assets.to_vec()));
        ui.close();
    }
    action
}

/// The review the selected photos share, field by field; a field they
/// differ in shows as unrated or unflagged.
fn shared_review(assets: &[LibraryAsset]) -> crate::sidecar::PhotoReview {
    let mut reviews = assets.iter().map(|asset| asset.metadata.review);
    let Some(first) = reviews.next() else {
        return crate::sidecar::PhotoReview::default();
    };
    reviews.fold(first, |shared, review| crate::sidecar::PhotoReview {
        rating: if shared.rating == review.rating {
            shared.rating
        } else {
            0
        },
        flag: if shared.flag == review.flag {
            shared.flag
        } else {
            crate::sidecar::PhotoFlag::Unflagged
        },
    })
}

/// The bar shown while selecting: the count, Select all, rating and flags,
/// the photo actions and the way back to browsing. It always fits the
/// library's width: actions give up their labels first, then their buttons.
pub(super) fn show_library_selection_action_bar(
    ui: &Ui,
    app: &mut CalibRawApp,
    library_action: &mut Option<LibraryAction>,
) {
    if !app.library.is_selecting() {
        return;
    }
    let selected = app.library.selected_assets();
    let bounds = ui.max_rect();
    let compact = !moduwu_design::ResponsiveWidth::from_width(bounds.width()).is_wide();
    let count = selected.len();
    let caps = app.caps();
    let actions = BarAction::available(count, caps);
    let action_enabled = library_actions_enabled(app) && count > 0;
    let can_paste_adjustments = app.library.has_copied_adjustments();
    let presets = crate::ui::presets::PresetList::from_app(app);
    let inner_margin = moduwu_design::SPACE_SM;
    let mut select_all = false;
    let mut clear_selection = false;
    egui::Area::new(egui::Id::new("library-selection-action-bar"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::CENTER_BOTTOM)
        .fixed_pos(egui::pos2(
            bounds.center().x,
            bounds.bottom() - BAR_OUTER_MARGIN,
        ))
        .constrain_to(bounds)
        .movable(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::symmetric(inner_margin as i8, 6))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = if compact {
                        moduwu_design::SPACE_XS
                    } else {
                        6.0
                    };
                    ui.spacing_mut().interact_size.y = moduwu_design::CONTROL_HEIGHT;
                    let spacing = ui.spacing().item_spacing.x;
                    let icon = moduwu_design::toolbar_icon_size().x + spacing;
                    let separator = BAR_SEPARATOR_SPACING + spacing;
                    let count_label = if bounds.width() < SELECTION_BAR_COUNT_LABEL_BREAKPOINT {
                        count.to_string()
                    } else {
                        format!("{count} selected")
                    };
                    // The count, Select all, rating, ✕ and the separators
                    // always show; the actions share what is left.
                    let fixed = text_width(ui, count_label.clone(), egui::TextStyle::Body, 0.0)
                        + spacing
                        + icon
                        + super::review::COMPACT_REVIEW_EDITOR_WIDTH
                        + spacing
                        + 2.0 * separator
                        + icon;
                    let room =
                        bounds.width() - 2.0 * (BAR_OUTER_MARGIN + inner_margin + 1.0) - fixed;
                    let layout = selection_bar_layout(ui, &actions, room);
                    let (inline, overflow) = match layout {
                        BarLayout::Labeled => (&actions[..], &[][..]),
                        BarLayout::Icons { inline } => actions.split_at(inline),
                    };
                    let labeled = layout == BarLayout::Labeled;

                    ui.horizontal(|ui| {
                        ui.strong(count_label).on_hover_text(format!(
                            "{count} selected {}",
                            if count == 1 { "photo" } else { "photos" }
                        ));
                        select_all = moduwu_design::ghost_icon_button(
                            ui,
                            egui_phosphor::regular::SELECTION_ALL,
                            moduwu_design::toolbar_icon_size(),
                            "Select all",
                        )
                        .clicked();
                        ui.add(egui::Separator::default().spacing(BAR_SEPARATOR_SPACING));
                        let review = ui
                            .add_enabled_ui(action_enabled, |ui| {
                                super::review::compact_review_editor(
                                    ui,
                                    "library-selection-review",
                                    shared_review(&selected),
                                )
                            })
                            .inner;
                        if let Some(change) = review {
                            *library_action = Some(LibraryAction::Review(selected.clone(), change));
                        }
                        for bar_action in inline {
                            let enabled =
                                action_enabled && bar_action.enabled(can_paste_adjustments);
                            if selection_bar_action_button(ui, *bar_action, enabled, labeled)
                                .clicked()
                            {
                                *library_action = bar_action.action(&selected);
                            }
                        }
                        let more_label = if labeled {
                            egui::RichText::new(bar_more_text())
                        } else {
                            egui::RichText::new(egui_phosphor::regular::DOTS_THREE)
                                .size(moduwu_design::CONTROL_HEIGHT * 0.55)
                        };
                        let more = ui.add_enabled_ui(action_enabled, |ui| {
                            ui.menu_button(more_label, |ui| {
                                selection_bar_more_menu(
                                    ui,
                                    &selected,
                                    overflow,
                                    can_paste_adjustments,
                                    &presets,
                                    caps,
                                )
                            })
                        });
                        more.inner.response.on_hover_text("More selection actions");
                        if let Some(action) = more.inner.inner.flatten() {
                            *library_action = Some(action);
                        }
                        ui.add(egui::Separator::default().spacing(BAR_SEPARATOR_SPACING));
                        clear_selection = moduwu_design::ghost_icon_button(
                            ui,
                            egui_phosphor::regular::X,
                            moduwu_design::toolbar_icon_size(),
                            "Clear selection",
                        )
                        .clicked();
                    });
                });
        });
    if select_all {
        app.library.select_all();
    }
    if clear_selection {
        app.library.clear_selection();
    }
}

pub(crate) fn show_library_action_overlays(
    ui: &mut Ui,
    app: &mut CalibRawApp,
    frame: &eframe::Frame,
) {
    let delete_choice = app
        .library
        .delete_originals_confirmation
        .as_ref()
        .and_then(|assets| show_delete_originals_confirmation(ui, assets, app.caps()));
    if let Some(confirmed) = delete_choice {
        if let Some(assets) = app.library.delete_originals_confirmation.take() {
            if confirmed {
                delete_confirmed_library_assets(ui, app, assets);
            }
        }
    }

    show_library_raw_name_dialog(ui, app, frame);

    let paste_choice = app
        .library
        .adjustment_paste_dialog
        .as_ref()
        .and_then(|dialog| {
            show_adjustment_paste_choice(
                ui,
                "library-adjustment-paste-conflict-dialog",
                dialog.edited_count,
                dialog.assets.len(),
            )
        });
    if let Some(choice) = paste_choice {
        if let Some(dialog) = app.library.adjustment_paste_dialog.take() {
            let mode = match choice {
                AdjustmentPasteChoice::Merge => Some(crate::sidecar::AdjustmentPasteMode::Merge),
                AdjustmentPasteChoice::Replace => {
                    Some(crate::sidecar::AdjustmentPasteMode::Replace)
                }
                AdjustmentPasteChoice::Cancel => None,
            };
            if let Some(mode) = mode {
                apply_library_adjustment_paste(app, dialog.assets, mode, ui.ctx(), frame);
            }
        }
    }

    let can_regenerate = app.can_start_library_ai_mask_refresh();
    let refresh_choice = app
        .library
        .ai_mask_refresh_prompt
        .as_ref()
        .and_then(|prompt| {
            show_ai_mask_refresh_choice(
                ui,
                "library-ai-mask-refresh-prompt",
                prompt.assets.len(),
                can_regenerate,
            )
        });
    if let Some(choice) = refresh_choice {
        if let Some(prompt) = app.library.ai_mask_refresh_prompt.take() {
            if choice == AiMaskRefreshChoice::Regenerate {
                start_library_ai_mask_refresh_for_assets(app, prompt.assets, frame);
            }
        }
    }

    if let Some((completed, total, failed, current_name)) = app.library_ai_mask_refresh_status() {
        if show_ai_mask_refresh_progress(ui, completed, total, failed, current_name.as_deref()) {
            app.cancel_library_ai_mask_refresh();
        }
    }

    let mut close_export_dialog = false;
    let mut confirm_export = false;
    let mut selected_export_format = None;
    // Folder exports ask where to save; media-library exports are published.
    let publishes = app.caps().has_media_library();
    let presets = app.presets.all();
    let luts = app.luts.entries();
    if let Some(dialog) = app.library.export_dialog.as_mut() {
        let count = dialog.assets.len();
        let export_picker_directory = dialog
            .assets
            .first()
            .and_then(LibraryAsset::desktop_path)
            .and_then(Path::parent)
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf);
        let title = if count == 1 {
            "Export image".to_owned()
        } else {
            format!("Export {count} images")
        };
        let help = if publishes {
            "Exports are saved to Pictures/CalibRaw. File names are generated from each photo's name."
        } else if count > 1 {
            "A destination folder will be selected for the batch. File names are generated from each photo's name."
        } else {
            "Choose the output file after pressing Export."
        };
        let label = match (count, publishes) {
            (1, false) => "Export 1 image…".to_owned(),
            (1, true) => "Export 1 image".to_owned(),
            (_, false) => format!("Export {count} images…"),
            (_, true) => format!("Export {count} images"),
        };
        moduwu_design::Dialog::new("library-export-dialog", title)
            .size(moduwu_design::DialogSize::Wide)
            .show_with_footer(
                ui.ctx(),
                |ui| {
                    let format_changed = show_library_export_settings_controls(
                        ui,
                        &mut dialog.format,
                        &mut dialog.settings,
                        export_picker_directory.as_deref(),
                    );
                    if format_changed {
                        selected_export_format = Some(dialog.format);
                    }
                    show_export_override_controls(ui, &mut dialog.overrides, presets, luts);
                },
                |ui| {
                    if moduwu_design::primary_action_button(ui, label)
                        .on_hover_text(help)
                        .clicked()
                    {
                        confirm_export = true;
                    }
                    if moduwu_design::secondary_button(ui, "Cancel").clicked() {
                        close_export_dialog = true;
                    }
                    if !close_export_dialog
                        && !confirm_export
                        && moduwu_design::dialog_keyboard_action(
                            ui,
                            moduwu_design::DialogKeyboard::CLOSE_ONLY,
                            false,
                        ) == moduwu_design::DialogAction::Cancel
                    {
                        close_export_dialog = true;
                    }
                },
            );
    }

    if let Some(format) = selected_export_format {
        app.export.format = format;
        app.persist_performance_settings();
    }

    if confirm_export {
        if let Some(dialog) = app.library.export_dialog.clone() {
            let export_override = app.export_override(&dialog.overrides);
            app.export.override_choice = dialog.overrides;
            if start_local_library_export(
                app,
                &dialog.assets,
                dialog.settings,
                dialog.format,
                export_override,
                frame,
            ) {
                app.library.clear_selection();
                app.library.export_dialog = None;
            }
        }
    } else if close_export_dialog {
        app.library.export_dialog = None;
    }
}
