//! A library of folders on a file system: a folder tree under the opened
//! root, with photos and their sidecars read and changed by path.

use super::*;
use crate::i18n::{tr, tr_fmt};
use egui_phosphor::regular;
use moduwu_design::MenuItem;

pub(super) mod catalog;
pub(super) mod files;
mod local;

pub(in crate::ui::library) fn default_thumbnail_worker_count() -> usize {
    automatic_thumbnail_worker_count()
}

/// Sized once per process from the logical cores and installed memory.
pub(in crate::ui::library) fn automatic_thumbnail_worker_count() -> usize {
    static COUNT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *COUNT.get_or_init(|| {
        let cores = logical_core_count();
        let memory = calibraw_core::system_memory::total_memory_bytes();
        let count = automatic_thumbnail_worker_count_for(cores, memory);
        log::info!(
            "automatic thumbnail workers: {count} ({cores} logical cores, {} installed memory)",
            memory.map_or_else(
                || "unknown".to_owned(),
                |bytes| format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
            ),
        );
        count
    })
}

/// Every logical core, but never below the previous fixed maximum.
pub(in crate::ui::library) fn maximum_thumbnail_worker_count() -> usize {
    logical_core_count().clamp(
        super::MIN_DESKTOP_THUMBNAIL_WORKER_LIMIT,
        super::MAX_DESKTOP_THUMBNAIL_WORKER_LIMIT,
    )
}

fn logical_core_count() -> usize {
    std::thread::available_parallelism().map_or(1, usize::from)
}

/// One worker per core except one kept for the UI thread and GPU submission,
/// bounded by a share of installed memory at a per-worker peak budget.
fn automatic_thumbnail_worker_count_for(cores: usize, total_memory_bytes: Option<u64>) -> usize {
    let by_cores = cores.saturating_sub(1);
    let by_memory = total_memory_bytes.map_or(super::UNKNOWN_MEMORY_THUMBNAIL_WORKERS, |bytes| {
        let share = bytes / super::THUMBNAIL_WORKER_MEMORY_SHARE_DIVISOR;
        usize::try_from(share / super::THUMBNAIL_WORKER_MEMORY_BUDGET_BYTES).unwrap_or(usize::MAX)
    });
    by_cores
        .min(by_memory)
        .clamp(1, super::MAX_AUTOMATIC_THUMBNAIL_WORKERS)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn show_library_folder_node(
    ui: &mut Ui,
    node: &LibraryFolderNode,
    depth: usize,
    root_folder: &Path,
    selected_folder: Option<&Path>,
    clipboard: Option<&LibraryFolderClipboard>,
    image_clipboard: Option<&ImageClipboard>,
    action_in_progress: bool,
    expanded_folders: &mut HashSet<PathBuf>,
    requested_folder: &mut Option<PathBuf>,
    requested_action: &mut Option<LibraryFolderUiAction>,
) {
    let has_children = !node.children.is_empty();
    let expanded = has_children && expanded_folders.contains(&node.path);
    let row = crate::ui::icons::folder_tree_row(
        ui,
        &node.path,
        &node.name,
        depth,
        has_children.then_some(expanded),
        selected_folder == Some(node.path.as_path()),
        Sense::click_and_drag(),
    );
    if row.toggled() {
        if expanded {
            expanded_folders.remove(&node.path);
        } else {
            expanded_folders.insert(node.path.clone());
        }
    }

    let response = row.row.on_hover_text(tr_fmt(
        "{path}\nDrag onto another folder to move",
        &[("path", &node.path.display())],
    ));
    response.dnd_set_drag_payload(LibraryFolderDrag(node.path.clone()));
    if response.clicked() {
        *requested_folder = Some(node.path.clone());
    }

    let is_root = node.path == root_folder;
    moduwu_design::context_menu(&response, |ui| {
        let enabled = !action_in_progress;
        let path = &node.path;
        let mut row = |ui: &mut Ui, enabled: bool, item, action| {
            if ui.add_enabled(enabled, item).clicked() {
                *requested_action = Some(action);
                ui.close();
            }
        };
        row(
            ui,
            enabled,
            MenuItem::new(tr("New Folder…")).icon(regular::FOLDER_PLUS),
            LibraryFolderUiAction::New(path.clone()),
        );
        let paste_label = image_clipboard.map_or_else(
            || {
                clipboard
                    .and_then(|clipboard| clipboard.path.file_name())
                    .map(|name| {
                        tr_fmt(
                            "Paste “{to_string_lossy}”",
                            &[("to_string_lossy", &name.to_string_lossy())],
                        )
                    })
                    .unwrap_or_else(|| tr("Paste").to_owned())
            },
            ImageClipboard::paste_label,
        );
        row(
            ui,
            enabled && (clipboard.is_some() || image_clipboard.is_some()),
            MenuItem::new(paste_label).icon(regular::CLIPBOARD),
            if image_clipboard.is_some() {
                LibraryFolderUiAction::PasteImages(path.clone())
            } else {
                LibraryFolderUiAction::Paste(path.clone())
            },
        );
        ui.separator();
        let movable = enabled && !is_root;
        row(
            ui,
            movable,
            MenuItem::new(tr("Copy Folder")).icon(regular::COPY),
            LibraryFolderUiAction::Copy(path.clone()),
        );
        row(
            ui,
            movable,
            MenuItem::new(tr("Cut Folder")).icon(regular::SCISSORS),
            LibraryFolderUiAction::Cut(path.clone()),
        );
        row(
            ui,
            movable,
            MenuItem::new(tr("Rename Folder…")).icon(regular::PENCIL_SIMPLE),
            LibraryFolderUiAction::Rename(path.clone()),
        );
        ui.separator();
        row(
            ui,
            movable,
            MenuItem::new(tr("Delete Folder…"))
                .icon(regular::TRASH)
                .destructive(),
            LibraryFolderUiAction::Delete(path.clone()),
        );
        row(
            ui,
            enabled,
            MenuItem::new(tr("Refresh Folders")).icon(regular::ARROWS_CLOCKWISE),
            LibraryFolderUiAction::Refresh,
        );
    });

    if let Some(payload) = response.dnd_hover_payload::<LibraryFolderDrag>() {
        let source = &payload.0;
        let can_drop = !action_in_progress
            && source != root_folder
            && source != &node.path
            && !node.path.starts_with(source);
        if can_drop {
            ui.painter().rect_stroke(
                response.rect,
                moduwu_design::CARD_RADIUS,
                Stroke::new(2.0, ui.visuals().selection.stroke.color),
                StrokeKind::Inside,
            );
            if let Some(payload) = response.dnd_release_payload::<LibraryFolderDrag>() {
                *requested_action = Some(LibraryFolderUiAction::Move {
                    source: payload.0.clone(),
                    destination_parent: node.path.clone(),
                });
            }
        }
    }

    if expanded {
        for child in &node.children {
            show_library_folder_node(
                ui,
                child,
                depth + 1,
                root_folder,
                selected_folder,
                clipboard,
                image_clipboard,
                action_in_progress,
                expanded_folders,
                requested_folder,
                requested_action,
            );
        }
    }
}

pub(super) fn apply_library_folder_ui_action(
    app: &mut CalibRawApp,
    action: LibraryFolderUiAction,
    context: &egui::Context,
) {
    let Some(root) = app.library.root_folder.clone() else {
        return;
    };
    match action {
        LibraryFolderUiAction::New(parent) => {
            app.library.folder_name_dialog = Some(LibraryFolderNameDialog {
                kind: LibraryFolderNameDialogKind::Create { parent },
                name: String::new(),
                error: None,
                focus_requested: false,
            });
        }
        LibraryFolderUiAction::Copy(path) => {
            match canonical_library_directory(&root, &path, false) {
                Ok(_) => {
                    app.library.folder_clipboard = Some(LibraryFolderClipboard {
                        path: path.clone(),
                        mode: LibraryFolderClipboardMode::Copy,
                    });
                    app.library.image_clipboard = None;
                    app.library.status = tr_fmt(
                        "Copied folder {path}. Choose Paste Folder in a destination.",
                        &[("path", &path.display())],
                    );
                }
                Err(error) => app.library.status = error,
            }
        }
        LibraryFolderUiAction::Cut(path) => {
            match canonical_library_directory(&root, &path, false) {
                Ok(_) => {
                    app.library.folder_clipboard = Some(LibraryFolderClipboard {
                        path: path.clone(),
                        mode: LibraryFolderClipboardMode::Cut,
                    });
                    app.library.image_clipboard = None;
                    app.library.status = tr_fmt(
                        "Cut folder {path}. Choose Paste Folder in a destination.",
                        &[("path", &path.display())],
                    );
                }
                Err(error) => app.library.status = error,
            }
        }
        LibraryFolderUiAction::Paste(destination_parent) => {
            let Some(clipboard) = app.library.folder_clipboard.clone() else {
                app.library.status = tr("Copy or cut a folder first.").to_owned();
                return;
            };
            if clipboard.mode == LibraryFolderClipboardMode::Cut
                && app
                    .develop
                    .current_path
                    .as_ref()
                    .is_some_and(|path| path.starts_with(&clipboard.path))
            {
                app.library.status =
                    tr("Open an image outside this folder before moving it.").to_owned();
                return;
            }
            let operation = match clipboard.mode {
                LibraryFolderClipboardMode::Copy => LibraryFolderOperation::Copy {
                    root,
                    source: clipboard.path,
                    destination_parent,
                },
                LibraryFolderClipboardMode::Cut => LibraryFolderOperation::Move {
                    root,
                    source: clipboard.path,
                    destination_parent,
                    new_name: None,
                },
            };
            app.library.start_folder_operation(operation, context);
        }
        LibraryFolderUiAction::PasteImages(destination_parent) => {
            start_image_clipboard_paste(
                app,
                LibraryTransferDestination::LocalFolder(destination_parent),
                context,
            );
        }
        LibraryFolderUiAction::Rename(source) => {
            if app
                .develop
                .current_path
                .as_ref()
                .is_some_and(|path| path.starts_with(&source))
            {
                app.library.status =
                    tr("Open an image outside this folder before renaming it.").to_owned();
                return;
            }
            let Some(name) = source
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
            else {
                app.library.status = tr("This folder name cannot be edited as text.").to_owned();
                return;
            };
            app.library.folder_name_dialog = Some(LibraryFolderNameDialog {
                kind: LibraryFolderNameDialogKind::Rename { source },
                name,
                error: None,
                focus_requested: false,
            });
        }
        LibraryFolderUiAction::Delete(path) => {
            app.library.folder_delete_confirmation = Some(path);
        }
        LibraryFolderUiAction::Move {
            source,
            destination_parent,
        } => {
            if app
                .develop
                .current_path
                .as_ref()
                .is_some_and(|path| path.starts_with(&source))
            {
                app.library.status =
                    tr("Open an image outside this folder before moving it.").to_owned();
                return;
            }
            app.library.start_folder_operation(
                LibraryFolderOperation::Move {
                    root,
                    source,
                    destination_parent,
                    new_name: None,
                },
                context,
            );
        }
        LibraryFolderUiAction::Refresh => app.library.refresh(context),
    }
}

pub(in crate::ui::library) fn start_local_library_ai_mask_refresh(
    app: &mut CalibRawApp,
    assets: &[LibraryAsset],
    frame: &eframe::Frame,
) {
    app.start_library_ai_mask_refresh_paths(desktop_paths(assets), frame);
}

pub(in crate::ui::library) fn start_local_library_export(
    app: &mut CalibRawApp,
    assets: &[LibraryAsset],
    settings: ExportSettings,
    format: ExportFormat,
    export_override: crate::app::ExportOverride,
    frame: &eframe::Frame,
) -> bool {
    let Some(jobs) = library_export_jobs(
        &desktop_paths(assets),
        format,
        &app.preferences.export_name_template,
    ) else {
        return false;
    };
    app.start_library_exports(jobs, settings, format, export_override, frame);
    true
}

pub(in crate::ui::library) fn local_action_in_progress(app: &CalibRawApp) -> bool {
    app.library.file_action_in_progress()
}

pub(in crate::ui::library) fn local_folders_available(app: &CalibRawApp) -> bool {
    app.library.root_folder.is_some()
}

pub(in crate::ui::library) fn can_create_local_folder(app: &CalibRawApp) -> bool {
    app.library.folder.is_some()
}

pub(in crate::ui::library) fn apply_local_toolbar_action(
    app: &mut CalibRawApp,
    action: LocalFolderToolbarAction,
    context: &egui::Context,
) {
    let Some(folder) = app.library.folder.clone() else {
        if matches!(action, LocalFolderToolbarAction::Refresh) {
            app.library.refresh(context);
        }
        return;
    };
    let action = match action {
        LocalFolderToolbarAction::Refresh => LibraryFolderUiAction::Refresh,
        LocalFolderToolbarAction::New => LibraryFolderUiAction::New(folder),
    };
    apply_library_folder_ui_action(app, action, context);
}

pub(in crate::ui::library) fn show_local_folder_tree(
    ui: &mut Ui,
    app: &mut CalibRawApp,
    action_in_progress: bool,
) {
    let tree = app.library.folder_tree.as_ref();
    let root_folder = app.library.root_folder.as_deref();
    let selected_folder = app.library.folder.as_deref();
    let clipboard = app.library.folder_clipboard.as_ref();
    let image_clipboard = app.library.image_clipboard.as_ref();
    let expanded_folders = &mut app.library.expanded_folders;
    let mut requested_folder = None;
    let mut requested_action = None;

    if let (Some(tree), Some(root_folder)) = (tree, root_folder) {
        moduwu_design::tree(ui, |ui| {
            show_library_folder_node(
                ui,
                tree,
                0,
                root_folder,
                selected_folder,
                clipboard,
                image_clipboard,
                action_in_progress,
                expanded_folders,
                &mut requested_folder,
                &mut requested_action,
            );
        });
    } else {
        ui.label(
            egui::RichText::new(tr("Open a top-level folder to browse its hierarchy."))
                .small()
                .color(ui.visuals().weak_text_color()),
        );
    }

    if let Some(folder) = requested_folder {
        app.select_library_folder(folder);
    }
    if let Some(action) = requested_action {
        apply_library_folder_ui_action(app, action, ui.ctx());
    }
}

pub(in crate::ui::library) fn show_sidebar_dialogs(ui: &mut Ui, app: &mut CalibRawApp) {
    show_library_folder_dialogs(ui, app);
}

pub(in crate::ui::library) fn show_page_dialogs(_ui: &mut Ui, _app: &mut CalibRawApp) {}
