//! A library in the platform's media library: folders by relative path and
//! photos by content URI, listed and changed through the platform.

use super::*;
use crate::i18n::{tr, tr_fmt};
use egui_phosphor::regular;
use moduwu_design::MenuItem;

pub(super) mod catalog;
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

/// One root's folders as the sidebar draws them.
struct RootTreeView<'a> {
    key: &'a str,
    children_by_parent: HashMap<&'a str, Vec<&'a crate::platform::LibraryFolder>>,
    /// The folder selected, while this root is shown. Creating and refreshing
    /// folders act on the root shown, so only its rows offer them.
    selected_folder: Option<&'a str>,
    /// Set on a linked folder, with whether its last scan could reach it.
    linked: Option<(&'a crate::platform::LinkedFolder, bool)>,
    action_in_progress: bool,
}

impl<'a> RootTreeView<'a> {
    fn new(
        key: &'a str,
        folders: &'a [crate::platform::LibraryFolder],
        selected_folder: Option<&'a str>,
        linked: Option<(&'a crate::platform::LinkedFolder, bool)>,
        action_in_progress: bool,
    ) -> Self {
        let mut children_by_parent = HashMap::<&str, Vec<&crate::platform::LibraryFolder>>::new();
        for folder in folders {
            children_by_parent
                .entry(android_folder_parent(&folder.path))
                .or_default()
                .push(folder);
        }
        Self {
            key,
            children_by_parent,
            selected_folder,
            linked,
            action_in_progress,
        }
    }
}

fn show_android_library_folder_node(
    ui: &mut Ui,
    tree: &RootTreeView<'_>,
    path: &str,
    name: &str,
    depth: usize,
    expanded_folders: &mut HashSet<String>,
    requested_action: &mut Option<AndroidLibraryFolderUiAction>,
) {
    let children = tree
        .children_by_parent
        .get(path)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let has_children = !children.is_empty();
    let expanded = has_children && expanded_folders.contains(path);
    let linked = tree.linked.filter(|_| path.is_empty());
    let row = ui
        .add_enabled_ui(!tree.action_in_progress, |ui| {
            crate::ui::icons::folder_tree_row_with_icon(
                ui,
                ("library-folder", tree.key, path),
                name,
                depth,
                has_children.then_some(expanded),
                tree.selected_folder == Some(path),
                Sense::click(),
                match linked {
                    Some((_, available)) => linked_folder_icon(!available),
                    None if expanded => regular::FOLDER_OPEN,
                    None => regular::FOLDER,
                },
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
        *requested_action = Some(AndroidLibraryFolderUiAction::Select {
            root: tree.key.to_owned(),
            folder: path.to_owned(),
        });
    }
    let shown = tree.selected_folder.is_some();
    if shown || linked.is_some() {
        moduwu_design::context_menu(&row.row, |ui| {
            if shown {
                let new_item = MenuItem::new(tr("New folder here…")).icon(regular::FOLDER_PLUS);
                if ui.add(new_item).clicked() {
                    *requested_action = Some(AndroidLibraryFolderUiAction::New(path.to_owned()));
                    ui.close();
                }
                let refresh_item =
                    MenuItem::new(tr("Refresh folders")).icon(regular::ARROWS_CLOCKWISE);
                if ui.add(refresh_item).clicked() {
                    *requested_action = Some(AndroidLibraryFolderUiAction::Refresh);
                    ui.close();
                }
            }
            if let Some((folder, _)) = linked {
                let unlink_item = MenuItem::new(tr("Unlink folder")).icon(regular::LINK_BREAK);
                if ui.add(unlink_item).clicked() {
                    *requested_action = Some(AndroidLibraryFolderUiAction::Unlink(folder.clone()));
                    ui.close();
                }
            }
        });
    }

    if expanded {
        for child in children {
            show_android_library_folder_node(
                ui,
                tree,
                &child.path,
                &child.name,
                depth + 1,
                expanded_folders,
                requested_action,
            );
        }
    }
}

/// The link shows a folder is the person's own; the warning, that it could
/// not be reached.
fn linked_folder_icon(unavailable: bool) -> &'static str {
    if unavailable {
        regular::WARNING
    } else {
        regular::LINK
    }
}

/// Draws the root `key` as a tree. A root not shown has no rows below its
/// own until its folders were walked.
#[allow(clippy::too_many_arguments)]
fn show_root_tree(
    ui: &mut Ui,
    trees: &mut HashMap<String, FolderTree>,
    key: &str,
    label: &str,
    shown_root: &str,
    selected_folder: &str,
    linked: Option<(&crate::platform::LinkedFolder, bool)>,
    action_in_progress: bool,
    requested_action: &mut Option<AndroidLibraryFolderUiAction>,
) {
    let mut unlisted = FolderTree::default();
    let FolderTree {
        folders, expanded, ..
    } = trees.get_mut(key).unwrap_or(&mut unlisted);
    let view = RootTreeView::new(
        key,
        folders,
        (key == shown_root).then_some(selected_folder),
        linked,
        action_in_progress,
    );
    show_android_library_folder_node(ui, &view, "", label, 0, expanded, requested_action);
}

pub(super) fn apply_android_library_folder_ui_action(
    app: &mut CalibRawApp,
    action: AndroidLibraryFolderUiAction,
    context: &egui::Context,
) {
    match action {
        AndroidLibraryFolderUiAction::Select { root, folder } => {
            app.select_android_library_root_folder(root, folder);
            app.set_library_folder_sidebar_open(false);
        }
        AndroidLibraryFolderUiAction::Link => app.library.media.link_dialog = true,
        AndroidLibraryFolderUiAction::Unlink(folder) => {
            app.library.media.unlink_dialog = Some(folder);
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
    let can_link = app.caps().linked_folders;
    let MediaLibraryState {
        folder: selected_folder,
        trees,
        root,
        linked_folders,
        unavailable_roots,
        ..
    } = &mut app.library.media;
    let mut requested_action = None;

    moduwu_design::tree(ui, |ui| {
        show_root_tree(
            ui,
            trees,
            "",
            tr("Library"),
            root,
            selected_folder,
            None,
            action_in_progress,
            &mut requested_action,
        );
    });

    if can_link {
        ui.add_space(moduwu_design::SPACE_SM);
        ui.weak(tr("Linked folders"));
        moduwu_design::tree(ui, |ui| {
            for folder in linked_folders.iter() {
                let available = !unavailable_roots.contains(&folder.key);
                show_root_tree(
                    ui,
                    trees,
                    &folder.key,
                    &folder.label,
                    root,
                    selected_folder,
                    Some((folder, available)),
                    action_in_progress,
                    &mut requested_action,
                );
            }
            let row = ui
                .add_enabled_ui(!action_in_progress, |ui| {
                    crate::ui::icons::folder_tree_row_with_icon(
                        ui,
                        "library-link-a-folder",
                        tr("Link a folder…"),
                        0,
                        None,
                        false,
                        Sense::click(),
                        regular::PLUS,
                    )
                })
                .inner;
            if row.row.clicked() {
                requested_action = Some(AndroidLibraryFolderUiAction::Link);
            }
        });
    }

    if let Some(action) = requested_action {
        apply_android_library_folder_ui_action(app, action, ui.ctx());
    }
}

pub(in crate::ui::library) fn show_sidebar_dialogs(_ui: &mut Ui, _app: &mut CalibRawApp) {}

pub(in crate::ui::library) fn show_page_dialogs(ui: &mut Ui, app: &mut CalibRawApp) {
    show_android_library_folder_dialog(ui, app);
    show_link_folder_dialog(ui, app);
    show_unlink_folder_dialog(ui, app);
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
