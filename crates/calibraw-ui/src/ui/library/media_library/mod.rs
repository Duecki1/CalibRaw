//! A library in the platform's media library: folders by relative path and
//! photos by content URI, listed and changed through the platform.

use super::*;
use crate::i18n::{tr, tr_fmt};
use egui_phosphor::regular;
use moduwu_design::MenuItem;

mod local;

pub(in crate::ui::library) fn default_thumbnail_worker_count() -> usize {
    1
}

pub(in crate::ui::library) fn automatic_thumbnail_worker_count() -> usize {
    default_thumbnail_worker_count()
}

pub(in crate::ui::library) fn maximum_thumbnail_worker_count() -> usize {
    super::MAX_ANDROID_THUMBNAIL_WORKERS
}

#[allow(clippy::too_many_arguments)]
pub(super) fn show_android_library_folder_node(
    ui: &mut Ui,
    path: &str,
    name: &str,
    depth: usize,
    children_by_parent: &HashMap<&str, Vec<&crate::platform::LibraryFolder>>,
    selected_folder: &str,
    action_in_progress: bool,
    expanded_folders: &mut HashSet<String>,
    requested_action: &mut Option<AndroidLibraryFolderUiAction>,
) {
    let children = children_by_parent
        .get(path)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let has_children = !children.is_empty();
    let expanded = has_children && expanded_folders.contains(path);
    let row = ui
        .add_enabled_ui(!action_in_progress, |ui| {
            crate::ui::icons::folder_tree_row(
                ui,
                path,
                name,
                depth,
                has_children.then_some(expanded),
                selected_folder == path,
                Sense::click(),
            )
        })
        .inner;
    if row.toggled() {
        if expanded {
            expanded_folders.remove(path);
        } else {
            expanded_folders.insert(path.to_owned());
        }
    }
    if row.row.clicked() {
        *requested_action = Some(AndroidLibraryFolderUiAction::Select(path.to_owned()));
    }
    moduwu_design::context_menu(&row.row, |ui| {
        let new_item = MenuItem::new(tr("New folder here…")).icon(regular::FOLDER_PLUS);
        if ui.add(new_item).clicked() {
            *requested_action = Some(AndroidLibraryFolderUiAction::New(path.to_owned()));
            ui.close();
        }
        let refresh_item = MenuItem::new(tr("Refresh folders")).icon(regular::ARROWS_CLOCKWISE);
        if ui.add(refresh_item).clicked() {
            *requested_action = Some(AndroidLibraryFolderUiAction::Refresh);
            ui.close();
        }
    });

    if expanded {
        for child in children {
            show_android_library_folder_node(
                ui,
                &child.path,
                &child.name,
                depth + 1,
                children_by_parent,
                selected_folder,
                action_in_progress,
                expanded_folders,
                requested_action,
            );
        }
    }
}

pub(super) fn apply_android_library_folder_ui_action(
    app: &mut CalibRawApp,
    action: AndroidLibraryFolderUiAction,
    context: &egui::Context,
) {
    match action {
        AndroidLibraryFolderUiAction::Select(folder) => {
            app.select_android_library_folder(folder);
            app.set_library_folder_sidebar_open(false);
        }
        AndroidLibraryFolderUiAction::New(parent) => {
            app.library.media.folder_name_dialog = Some(AndroidLibraryFolderNameDialog {
                parent,
                name: String::new(),
                error: None,
                focus_requested: false,
            });
        }
        AndroidLibraryFolderUiAction::Refresh => app.library.refresh(context),
    }
}

pub(in crate::ui::library) fn start_local_library_ai_mask_refresh(
    app: &mut CalibRawApp,
    assets: &[LibraryAsset],
    frame: &eframe::Frame,
) {
    app.start_library_ai_mask_refresh_android(android_targets(assets), frame);
}

pub(in crate::ui::library) fn start_local_library_export(
    app: &mut CalibRawApp,
    assets: &[LibraryAsset],
    settings: ExportSettings,
    format: ExportFormat,
    export_override: crate::app::ExportOverride,
    _frame: &eframe::Frame,
) -> bool {
    let targets = android_targets(assets)
        .into_iter()
        .map(|(uri, display_name)| crate::app::AndroidLibraryExportTarget { uri, display_name })
        .collect::<Vec<_>>();
    if targets.is_empty() {
        return false;
    }
    app.start_android_library_exports(targets, settings, format, export_override);
    true
}

pub(in crate::ui::library) fn local_action_in_progress(app: &CalibRawApp) -> bool {
    app.library.local_mutation_in_progress() || app.android.picker_pending
}

pub(in crate::ui::library) fn local_folders_available(_app: &CalibRawApp) -> bool {
    true
}

pub(in crate::ui::library) fn can_create_local_folder(_app: &CalibRawApp) -> bool {
    true
}

pub(in crate::ui::library) fn apply_local_toolbar_action(
    app: &mut CalibRawApp,
    action: LocalFolderToolbarAction,
    context: &egui::Context,
) {
    let action = match action {
        LocalFolderToolbarAction::Refresh => AndroidLibraryFolderUiAction::Refresh,
        LocalFolderToolbarAction::New => {
            AndroidLibraryFolderUiAction::New(app.library.media.folder.clone())
        }
    };
    apply_android_library_folder_ui_action(app, action, context);
}

pub(in crate::ui::library) fn show_local_folder_tree(
    ui: &mut Ui,
    app: &mut CalibRawApp,
    action_in_progress: bool,
) {
    let MediaLibraryState {
        folder: selected_folder,
        folders,
        expanded_folders,
        ..
    } = &mut app.library.media;
    let mut children_by_parent = HashMap::<&str, Vec<&crate::platform::LibraryFolder>>::new();
    for folder in folders.iter() {
        children_by_parent
            .entry(android_folder_parent(&folder.path))
            .or_default()
            .push(folder);
    }
    let mut requested_action = None;

    moduwu_design::tree(ui, |ui| {
        show_android_library_folder_node(
            ui,
            "",
            tr("Library"),
            0,
            &children_by_parent,
            selected_folder.as_str(),
            action_in_progress,
            expanded_folders,
            &mut requested_action,
        );
    });

    if let Some(action) = requested_action {
        apply_android_library_folder_ui_action(app, action, ui.ctx());
    }
}

pub(in crate::ui::library) fn show_sidebar_dialogs(_ui: &mut Ui, _app: &mut CalibRawApp) {}

pub(in crate::ui::library) fn show_page_dialogs(ui: &mut Ui, app: &mut CalibRawApp) {
    show_android_library_folder_dialog(ui, app);
    if let Some(status) = app.android.photo_import {
        show_photo_import_progress(ui, status);
    }
}

/// Covers the gap between picking photos and their placeholders appearing.
/// Android copies them without a way to cancel, so the dialog has no buttons.
fn show_photo_import_progress(ui: &mut Ui, status: crate::app::PhotoImportStatus) {
    moduwu_design::Dialog::new("library-photo-import-progress", tr("Importing photos")).show(
        ui.ctx(),
        |ui| match status {
            crate::app::PhotoImportStatus::Copying(progress) => {
                let fraction = if progress.total == 0 {
                    0.0
                } else {
                    (progress.completed as f32 / progress.total as f32).clamp(0.0, 1.0)
                };
                ui.label(
                    egui::RichText::new(tr_fmt(
                        "{completed} / {total} photos copied",
                        &[
                            ("completed", &progress.completed),
                            ("total", &progress.total),
                        ],
                    ))
                    .strong(),
                );
                ui.add_space(6.0);
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .show_percentage()
                        .animate(true),
                );
            }
            crate::app::PhotoImportStatus::Loading => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr("Loading library…"));
                });
            }
        },
    );
}

pub(in crate::ui::library) fn android_library_location_label(root: &str, folder: &str) -> String {
    if folder.is_empty() {
        root.to_owned()
    } else {
        format!("{root}/{folder}")
    }
}

pub(in crate::ui::library) fn android_folder_parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

pub(in crate::ui::library) fn android_folder_ancestors(path: &str) -> HashSet<String> {
    let mut expanded = HashSet::from([String::new()]);
    let mut current = path.to_owned();
    while !current.is_empty() {
        let parent = android_folder_parent(&current).to_owned();
        expanded.insert(parent.clone());
        current = parent;
    }
    expanded
}
