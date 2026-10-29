use super::*;
use crate::i18n::{tr, tr_fmt};
use crate::pipeline::{FilmSource, FilmStock};
use egui_phosphor::regular;
use moduwu_design::MenuItem;

const SELECTION_BAR_COUNT_LABEL_BREAKPOINT: f32 = 360.0;

#[derive(Clone, Debug)]
pub(crate) enum LibraryAction {
    Review(Vec<LibraryAsset>, super::review::ReviewChange),
    Export(Vec<LibraryAsset>),
    Merge(super::MergeKind, Vec<LibraryAsset>),
    CopyAdjustments(LibraryAsset),
    PasteAdjustments(Vec<LibraryAsset>),
    ApplyPreset(Vec<LibraryAsset>, PathBuf),
    SetFilm(Vec<LibraryAsset>, FilmSource),
    Copy(Vec<LibraryAsset>),
    Cut(Vec<LibraryAsset>),
    PasteIntoAssetFolder(LibraryAsset),
    Duplicate(Vec<LibraryAsset>),
    Rename(LibraryAsset),
    RevealFile(LibraryAsset),
    ResetAdjustments(Vec<LibraryAsset>),
    Delete(Vec<LibraryAsset>),
}

/// Whether photo actions can start: no file action, batch export or AI mask
/// refresh is running.
pub(super) fn library_actions_enabled(app: &CalibRawApp) -> bool {
    !local_action_in_progress(app)
        && app.library_batch_export_progress().is_none()
        && app.library_ai_mask_refresh_status().is_none()
}

/// The photo menu of the grid, the filmstrip and the selection bar: the
/// rating and flag row, then the actions for `assets`. `anchor` is the photo
/// the menu was opened on, whose folder Paste here pastes into; the selection
/// bar's menu has none.
pub(crate) fn photo_actions_menu(
    ui: &mut Ui,
    app: &CalibRawApp,
    assets: &[LibraryAsset],
    anchor: Option<&LibraryAsset>,
) -> Option<LibraryAction> {
    let enabled = library_actions_enabled(app) && !assets.is_empty();
    let single = match assets {
        [asset] => Some(asset),
        _ => None,
    };
    let mut action = None;
    moduwu_design::menu_title(
        ui,
        &single.map_or_else(
            || tr_fmt("{count} photos", &[("count", &assets.len())]),
            |asset| asset.display_name.clone(),
        ),
    );
    let review = ui
        .add_enabled_ui(enabled, |ui| {
            super::review::menu_review_row(ui, "photo-menu-review", shared_review(assets))
        })
        .inner;
    if let Some(change) = review {
        action = Some(LibraryAction::Review(assets.to_vec(), change));
        ui.close();
    }

    ui.separator();
    if menu_row(
        ui,
        enabled,
        MenuItem::new(tr("Export…")).icon(BarAction::Export.glyph()),
    )
    .clicked()
    {
        action = Some(LibraryAction::Export(assets.to_vec()));
    }
    if assets.len() >= 2 {
        let merge = ui.add_enabled_ui(enabled, |ui| {
            moduwu_design::submenu(
                ui,
                MenuItem::new(tr("Merge")).icon(regular::STACK),
                merge_menu,
            )
        });
        if let Some(Some(kind)) = merge.inner.inner {
            action = Some(LibraryAction::Merge(kind, assets.to_vec()));
        }
    }

    ui.separator();
    if menu_row(
        ui,
        enabled && single.is_some(),
        MenuItem::new(tr("Copy adjustments")).icon(BarAction::CopyAdjustments.glyph()),
    )
    .clicked()
    {
        action = single.cloned().map(LibraryAction::CopyAdjustments);
    }
    if menu_row(
        ui,
        enabled && app.library.has_copied_adjustments(),
        MenuItem::new(tr("Paste adjustments")).icon(BarAction::PasteAdjustments.glyph()),
    )
    .on_disabled_hover_text(tr("Copy adjustments from an image first"))
    .clicked()
    {
        action = Some(LibraryAction::PasteAdjustments(assets.to_vec()));
    }
    if let Some(preset) = apply_preset_menu(ui, app, enabled) {
        action = Some(LibraryAction::ApplyPreset(assets.to_vec(), preset));
    }
    let film = ui.add_enabled_ui(enabled, |ui| {
        moduwu_design::submenu(
            ui,
            MenuItem::new(tr("Film negative")).icon(regular::FILM_STRIP),
            film_menu,
        )
    });
    if let Some(Some(film)) = film.inner.inner {
        action = Some(LibraryAction::SetFilm(assets.to_vec(), film));
    }
    if menu_row(
        ui,
        enabled,
        MenuItem::new(tr("Reset adjustments")).icon(regular::ARROW_COUNTER_CLOCKWISE),
    )
    .clicked()
    {
        action = Some(LibraryAction::ResetAdjustments(assets.to_vec()));
    }

    ui.separator();
    // Only Duplicate works on photos outside folders, so it needs no submenu.
    if app.caps().has_folders() {
        let file = ui.add_enabled_ui(enabled, |ui| {
            moduwu_design::submenu(
                ui,
                MenuItem::new(tr("File")).icon(regular::FOLDER_SIMPLE),
                |ui| file_menu(ui, app, assets, anchor, enabled),
            )
        });
        if let Some(Some(chosen)) = file.inner.inner {
            action = Some(chosen);
        }
    } else if menu_row(ui, enabled, duplicate_item()).clicked() {
        action = Some(LibraryAction::Duplicate(assets.to_vec()));
    }
    if menu_row(
        ui,
        enabled && single.is_some(),
        MenuItem::new(tr("Rename…")).icon(regular::PENCIL_SIMPLE),
    )
    .clicked()
    {
        action = single.cloned().map(LibraryAction::Rename);
    }

    ui.separator();
    if menu_row(
        ui,
        enabled,
        MenuItem::new(tr("Delete…"))
            .icon(regular::TRASH)
            .destructive(),
    )
    .clicked()
    {
        action = Some(LibraryAction::Delete(assets.to_vec()));
    }
    action
}

/// Adds a menu row, closing the menu when it is chosen.
fn menu_row(ui: &mut Ui, enabled: bool, item: MenuItem<'_>) -> egui::Response {
    let response = ui.add_enabled(enabled, item);
    if response.clicked() {
        ui.close();
    }
    response
}

fn duplicate_item() -> MenuItem<'static> {
    MenuItem::new(tr("Duplicate")).icon(regular::COPY_SIMPLE)
}

/// How the photos are read: as colour or black-and-white negatives, each
/// estimated when it is next decoded, or as they are.
fn film_menu(ui: &mut Ui) -> Option<FilmSource> {
    let mut chosen = None;
    for (film, label, icon) in [
        (
            FilmSource::NegativeToEstimate(FilmStock::Color),
            tr("Color negative"),
            regular::DROP_HALF_BOTTOM,
        ),
        (
            FilmSource::NegativeToEstimate(FilmStock::BlackAndWhite),
            tr("Black & white negative"),
            regular::CIRCLE_HALF,
        ),
        (FilmSource::Positive, tr("Not a negative"), regular::IMAGE),
    ] {
        if menu_row(ui, true, MenuItem::new(label).icon(icon)).clicked() {
            chosen = Some(film);
        }
    }
    chosen
}

/// HDR merge and focus stacking, for two or more photos.
fn merge_menu(ui: &mut Ui) -> Option<super::MergeKind> {
    let mut chosen = None;
    for (kind, help) in [
        (
            super::MergeKind::Hdr,
            tr("Auto-align and merge RAW exposures into an editable 32-bit float TIFF"),
        ),
        (
            super::MergeKind::FocusStack,
            tr("Auto-align RAW frames focused at different distances and combine their sharpest parts into an editable 32-bit float TIFF"),
        ),
    ] {
        let item = MenuItem::new(kind.title()).icon(BarAction::Merge(kind).glyph());
        if menu_row(ui, true, item).on_hover_text(help).clicked() {
            chosen = Some(kind);
        }
    }
    chosen
}

/// The photos' files: the clipboard, Duplicate and the file manager.
fn file_menu(
    ui: &mut Ui,
    app: &CalibRawApp,
    assets: &[LibraryAsset],
    anchor: Option<&LibraryAsset>,
    enabled: bool,
) -> Option<LibraryAction> {
    let mut action = None;
    if menu_row(
        ui,
        enabled,
        MenuItem::new(tr("Copy")).icon(BarAction::Copy.glyph()),
    )
    .clicked()
    {
        action = Some(LibraryAction::Copy(assets.to_vec()));
    }
    if menu_row(
        ui,
        enabled,
        MenuItem::new(tr("Cut")).icon(regular::SCISSORS),
    )
    .clicked()
    {
        action = Some(LibraryAction::Cut(assets.to_vec()));
    }
    if let Some(anchor) = anchor {
        let clipboard = app.library.image_clipboard.as_ref();
        let label =
            clipboard.map_or_else(|| tr("Paste here").to_owned(), ImageClipboard::paste_label);
        let item = MenuItem::new(label).icon(regular::CLIPBOARD);
        if menu_row(ui, enabled && clipboard.is_some(), item)
            .on_disabled_hover_text(tr("Copy or cut photos first"))
            .clicked()
        {
            action = Some(LibraryAction::PasteIntoAssetFolder(anchor.clone()));
        }
    }
    if menu_row(ui, enabled, duplicate_item())
        .on_hover_text(tr("Copies the RAW with its sidecar"))
        .clicked()
    {
        action = Some(LibraryAction::Duplicate(assets.to_vec()));
    }
    if app.caps().is_desktop() {
        let revealable = match assets {
            [asset] => asset.desktop_path().is_some().then_some(asset),
            _ => None,
        };
        let item = MenuItem::new(tr("Show in File Manager")).icon(regular::FOLDER_OPEN);
        if menu_row(ui, enabled && revealable.is_some(), item).clicked() {
            action = revealable.cloned().map(LibraryAction::RevealFile);
        }
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
                Ok(()) => tr_fmt(
                    "Copied adjustments from {display_name}",
                    &[("display_name", &asset.display_name)],
                ),
                Err(error) => tr_fmt("Could not copy adjustments: {error}", &[("error", &error)]),
            };
        }
        LibraryAction::PasteAdjustments(assets) => {
            let (edited_count, failures) = app.library_adjustment_edit_count(&assets);
            if !failures.is_empty() {
                app.library.status = tr_fmt(
                    "Could not inspect selected adjustments. {details}",
                    &[("details", &failures.join(" · "))],
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
        LibraryAction::SetFilm(assets, film) => {
            apply_library_film(app, assets, film, ui.ctx(), frame);
        }
        LibraryAction::Copy(assets) => {
            set_library_clipboard(app, ImageClipboardMode::Copy, assets);
        }
        LibraryAction::Cut(assets) => {
            set_library_clipboard(app, ImageClipboardMode::Cut, assets);
        }
        LibraryAction::PasteIntoAssetFolder(asset) => match duplicate_destination(&asset) {
            Ok(destination) => start_image_clipboard_paste(app, destination, ui.ctx()),
            Err(error) => {
                app.library.status = tr_fmt(
                    "Could not determine paste folder: {error}",
                    &[("error", &error)],
                )
            }
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
            app.library.status = match (failures.is_empty(), total == 1) {
                (true, true) => tr_fmt(
                    "Cleared all adjustments for {total} selected image ({changed} changed)",
                    &[("total", &total), ("changed", &changed)],
                ),
                (true, false) => tr_fmt(
                    "Cleared all adjustments for {total} selected images ({changed} changed)",
                    &[("total", &total), ("changed", &changed)],
                ),
                (false, _) => tr_fmt(
                    "Cleared all adjustments for {cleared} of {total} selected images. {details}",
                    &[
                        ("cleared", &total.saturating_sub(failures.len())),
                        ("total", &total),
                        ("details", &failures.join(" · ")),
                    ],
                ),
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
    let presets = crate::ui::presets::PresetList::from_app(app);
    ui.add_enabled_ui(enabled && !presets.is_empty(), |ui| {
        let menu = moduwu_design::submenu(
            ui,
            MenuItem::new(tr("Apply preset")).icon(regular::PALETTE),
            |ui| presets.show_menu(ui),
        );
        menu.response
            .on_disabled_hover_text(tr("Create a preset in the editor's Presets tab first"));
        menu.inner.flatten()
    })
    .inner
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
    app.library.status = match (failures.is_empty(), app.caps().has_folders(), deleted == 1) {
        (true, true, true) => tr_fmt(
            "Moved {deleted} selected original to the {trash}",
            &[
                ("deleted", &deleted),
                ("trash", &crate::platform::trash_name()),
            ],
        ),
        (true, true, false) => tr_fmt(
            "Moved {deleted} selected originals to the {trash}",
            &[
                ("deleted", &deleted),
                ("trash", &crate::platform::trash_name()),
            ],
        ),
        (true, false, true) => tr_fmt(
            "Deleted {deleted} selected original",
            &[("deleted", &deleted)],
        ),
        (true, false, false) => tr_fmt(
            "Deleted {deleted} selected originals",
            &[("deleted", &deleted)],
        ),
        (false, _, _) => tr_fmt(
            "Removed {deleted} of {total} selected originals. {details}",
            &[
                ("deleted", &deleted),
                ("total", &total),
                ("details", &failures.join(" · ")),
            ],
        ),
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
    app.library.status = match (mode == ImageClipboardMode::Copy, count == 1) {
        (true, true) => tr_fmt(
            "Copied {count} photo. Choose Paste in a Library folder.",
            &[("count", &count)],
        ),
        (true, false) => tr_fmt(
            "Copied {count} photos. Choose Paste in a Library folder.",
            &[("count", &count)],
        ),
        (false, true) => tr_fmt(
            "Cut {count} photo. Choose Paste in a Library folder.",
            &[("count", &count)],
        ),
        (false, false) => tr_fmt(
            "Cut {count} photos. Choose Paste in a Library folder.",
            &[("count", &count)],
        ),
    };
}

/// A photo action with a button on the selection bar while the bar has
/// room. More lists every photo action, these included.
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
            Self::Export => tr("Export"),
            Self::Merge(kind) => kind.title(),
            Self::CopyAdjustments => tr("Copy adjustments"),
            Self::PasteAdjustments => tr("Paste adjustments"),
            Self::Copy => tr("Copy"),
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
    tr_fmt(
        "{dots_three}  More",
        &[("dots_three", &egui_phosphor::regular::DOTS_THREE)],
    )
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
        response.on_disabled_hover_text(tr("Copy adjustments from an image first"))
    } else {
        response
    }
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
                        tr_fmt("{count} selected", &[("count", &count)])
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
                    let inline = match layout {
                        BarLayout::Labeled => &actions[..],
                        BarLayout::Icons { inline } => &actions[..inline],
                    };
                    let labeled = layout == BarLayout::Labeled;

                    ui.horizontal(|ui| {
                        ui.strong(count_label).on_hover_text(if count == 1 {
                            tr_fmt("{count} selected photo", &[("count", &count)])
                        } else {
                            tr_fmt("{count} selected photos", &[("count", &count)])
                        });
                        select_all = moduwu_design::ghost_icon_button(
                            ui,
                            egui_phosphor::regular::SELECTION_ALL,
                            moduwu_design::toolbar_icon_size(),
                            tr("Select all"),
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
                        let more = ui
                            .add_enabled(action_enabled, egui::Button::new(more_label))
                            .on_hover_text(tr("More selection actions"));
                        let chosen = moduwu_design::dropdown_menu(&more, |ui| {
                            photo_actions_menu(ui, app, &selected, None)
                        });
                        if let Some(Some(action)) = chosen {
                            *library_action = Some(action);
                        }
                        ui.add(egui::Separator::default().spacing(BAR_SEPARATOR_SPACING));
                        clear_selection = moduwu_design::ghost_icon_button(
                            ui,
                            egui_phosphor::regular::X,
                            moduwu_design::toolbar_icon_size(),
                            tr("Clear selection"),
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
    let mut watermark_action = None;
    // The watermark is app-wide, so the dialog edits the Export panel's.
    let watermark = app.export.watermark.view();
    // Folder exports ask where to save; media-library exports are published.
    let publishes = app.caps().has_media_library();
    let presets = app.presets.all();
    let luts = app.luts.entries();
    if let Some(dialog) = app.library.export_dialog.as_mut() {
        let count = dialog.assets.len();
        let title = if count == 1 {
            tr("Export image").to_owned()
        } else {
            tr_fmt("Export {count} images", &[("count", &count)])
        };
        let help = if publishes {
            tr("Exports are saved to Pictures/CalibRaw. File names are generated from each photo's name.")
        } else if count > 1 {
            tr("A destination folder will be selected for the batch. File names are generated from each photo's name.")
        } else {
            tr("Choose the output file after pressing Export.")
        };
        let label = match (count, publishes) {
            (1, false) => tr("Export 1 image…").to_owned(),
            (1, true) => tr("Export 1 image").to_owned(),
            (_, false) => tr_fmt("Export {count} images…", &[("count", &count)]),
            (_, true) => tr_fmt("Export {count} images", &[("count", &count)]),
        };
        moduwu_design::Dialog::new("library-export-dialog", title)
            .size(moduwu_design::DialogSize::Wide)
            .show_with_footer(
                ui.ctx(),
                |ui| {
                    let change = show_library_export_settings_controls(
                        ui,
                        &mut dialog.format,
                        &mut dialog.settings,
                        watermark,
                    );
                    if change.format_changed {
                        selected_export_format = Some(dialog.format);
                    }
                    watermark_action = change.watermark;
                    show_export_override_controls(ui, &mut dialog.overrides, presets, luts);
                },
                |ui| {
                    if moduwu_design::primary_action_button(ui, label)
                        .on_hover_text(help)
                        .clicked()
                    {
                        confirm_export = true;
                    }
                    if moduwu_design::secondary_button(ui, tr("Cancel")).clicked() {
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
    if let Some(action) = watermark_action {
        app.apply_watermark_action(action);
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
