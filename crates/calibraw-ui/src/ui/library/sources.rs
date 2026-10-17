//! Where the library's photos live, as the platform's `StorageKind` says:
//! folders on disk (`folders`) or the platform's media library
//! (`media_library`). Library views call these and stay storage-neutral.

use super::*;
use crate::platform::StorageKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LocalFolderToolbarAction {
    Refresh,
    New,
}

/// Fixed small pools where memory is constrained; sized from the cores and
/// memory elsewhere.
pub(super) fn default_thumbnail_worker_count() -> usize {
    if PlatformCaps::CURRENT.constrained_memory() {
        media_library::default_thumbnail_worker_count()
    } else {
        folders::default_thumbnail_worker_count()
    }
}

pub(super) fn automatic_thumbnail_worker_count() -> usize {
    if PlatformCaps::CURRENT.constrained_memory() {
        media_library::automatic_thumbnail_worker_count()
    } else {
        folders::automatic_thumbnail_worker_count()
    }
}

pub(super) fn maximum_thumbnail_worker_count() -> usize {
    if PlatformCaps::CURRENT.constrained_memory() {
        media_library::maximum_thumbnail_worker_count()
    } else {
        folders::maximum_thumbnail_worker_count()
    }
}

pub(super) fn start_local_library_ai_mask_refresh(
    app: &mut CalibRawApp,
    assets: &[LibraryAsset],
    frame: &eframe::Frame,
) {
    match app.caps().storage {
        StorageKind::Folders => folders::start_local_library_ai_mask_refresh(app, assets, frame),
        StorageKind::MediaLibrary => {
            media_library::start_local_library_ai_mask_refresh(app, assets, frame);
        }
    }
}

pub(super) fn start_local_library_export(
    app: &mut CalibRawApp,
    assets: &[LibraryAsset],
    settings: ExportSettings,
    format: ExportFormat,
    frame: &eframe::Frame,
) -> bool {
    match app.caps().storage {
        StorageKind::Folders => {
            folders::start_local_library_export(app, assets, settings, format, frame)
        }
        StorageKind::MediaLibrary => {
            media_library::start_local_library_export(app, assets, settings, format, frame)
        }
    }
}

pub(super) fn local_action_in_progress(app: &CalibRawApp) -> bool {
    match app.caps().storage {
        StorageKind::Folders => folders::local_action_in_progress(app),
        StorageKind::MediaLibrary => media_library::local_action_in_progress(app),
    }
}

pub(super) fn local_folders_available(app: &CalibRawApp) -> bool {
    match app.caps().storage {
        StorageKind::Folders => folders::local_folders_available(app),
        StorageKind::MediaLibrary => media_library::local_folders_available(app),
    }
}

pub(super) fn can_create_local_folder(app: &CalibRawApp) -> bool {
    match app.caps().storage {
        StorageKind::Folders => folders::can_create_local_folder(app),
        StorageKind::MediaLibrary => media_library::can_create_local_folder(app),
    }
}

pub(super) fn apply_local_toolbar_action(
    app: &mut CalibRawApp,
    action: LocalFolderToolbarAction,
    context: &egui::Context,
) {
    match app.caps().storage {
        StorageKind::Folders => folders::apply_local_toolbar_action(app, action, context),
        StorageKind::MediaLibrary => {
            media_library::apply_local_toolbar_action(app, action, context);
        }
    }
}

pub(super) fn show_local_folder_tree(ui: &mut Ui, app: &mut CalibRawApp, action_in_progress: bool) {
    match app.caps().storage {
        StorageKind::Folders => folders::show_local_folder_tree(ui, app, action_in_progress),
        StorageKind::MediaLibrary => {
            media_library::show_local_folder_tree(ui, app, action_in_progress);
        }
    }
}

pub(super) fn show_sidebar_dialogs(ui: &mut Ui, app: &mut CalibRawApp) {
    match app.caps().storage {
        StorageKind::Folders => folders::show_sidebar_dialogs(ui, app),
        StorageKind::MediaLibrary => media_library::show_sidebar_dialogs(ui, app),
    }
}

pub(super) fn show_page_dialogs(ui: &mut Ui, app: &mut CalibRawApp) {
    match app.caps().storage {
        StorageKind::Folders => folders::show_page_dialogs(ui, app),
        StorageKind::MediaLibrary => media_library::show_page_dialogs(ui, app),
    }
}
