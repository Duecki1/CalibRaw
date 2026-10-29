use super::*;
use crate::i18n::{tr, tr_fmt};

pub(super) fn image_paste_summary(
    mode: ImageClipboardMode,
    total: usize,
    completed: usize,
    destination: &str,
    errors: Vec<String>,
) -> Result<String, String> {
    let copy = mode == ImageClipboardMode::Copy;
    let one = total == 1;
    if errors.is_empty() {
        Ok(match (copy, one) {
            (true, true) => tr_fmt(
                "Copied {completed} photo to {destination}.",
                &[("completed", &completed), ("destination", &destination)],
            ),
            (true, false) => tr_fmt(
                "Copied {completed} photos to {destination}.",
                &[("completed", &completed), ("destination", &destination)],
            ),
            (false, true) => tr_fmt(
                "Moved {completed} photo to {destination}.",
                &[("completed", &completed), ("destination", &destination)],
            ),
            (false, false) => tr_fmt(
                "Moved {completed} photos to {destination}.",
                &[("completed", &completed), ("destination", &destination)],
            ),
        })
    } else {
        let details = errors.join(" · ");
        Err(match (copy, one) {
            (true, true) => tr_fmt(
                "Copied {completed} of {total} photo to {destination}. {details}",
                &[
                    ("completed", &completed),
                    ("total", &total),
                    ("destination", &destination),
                    ("details", &details),
                ],
            ),
            (true, false) => tr_fmt(
                "Copied {completed} of {total} photos to {destination}. {details}",
                &[
                    ("completed", &completed),
                    ("total", &total),
                    ("destination", &destination),
                    ("details", &details),
                ],
            ),
            (false, true) => tr_fmt(
                "Moved {completed} of {total} photo to {destination}. {details}",
                &[
                    ("completed", &completed),
                    ("total", &total),
                    ("destination", &destination),
                    ("details", &details),
                ],
            ),
            (false, false) => tr_fmt(
                "Moved {completed} of {total} photos to {destination}. {details}",
                &[
                    ("completed", &completed),
                    ("total", &total),
                    ("destination", &destination),
                    ("details", &details),
                ],
            ),
        })
    }
}

fn copy_asset_to_destination(
    asset: &LibraryAsset,
    destination: &LibraryTransferDestination,
    platform: &dyn Platform,
) -> Result<ImportedLibraryAsset, String> {
    let materialized = materialize_library_asset(asset, platform)?;
    let result = import_materialized_library_asset(&materialized, destination, platform).and_then(
        |imported| {
            if let Err(error) = preserve_imported_thumbnail(asset, &imported, platform) {
                rollback_imported_library_asset(imported, platform);
                Err(error)
            } else {
                Ok(imported)
            }
        },
    );
    materialized.cleanup();
    result
}

pub(super) fn run_image_paste(
    clipboard: ImageClipboard,
    destination: LibraryTransferDestination,
    platform: &dyn Platform,
) -> AssetTransferCompletion {
    let mode = clipboard.mode;
    let total = clipboard.assets.len();
    let mut completed = 0usize;
    let mut errors = Vec::new();
    let mut remaining = clipboard.assets.clone();

    for asset in clipboard.assets {
        if mode == ImageClipboardMode::Cut && asset_is_at_destination(&asset, &destination) {
            completed += 1;
            remaining.retain(|candidate| candidate.id != asset.id);
            continue;
        }
        let result =
            copy_asset_to_destination(&asset, &destination, platform).and_then(|imported| {
                if mode != ImageClipboardMode::Cut {
                    return Ok(());
                }
                if let Err(error) = remove_library_asset(&asset) {
                    rollback_imported_library_asset(imported, platform);
                    Err(error)
                } else {
                    Ok(())
                }
            });

        match result {
            Ok(()) => {
                completed += 1;
                remaining.retain(|candidate| candidate.id != asset.id);
            }
            Err(error) => errors.push(format!("{}: {error}", asset.display_name)),
        }
    }

    let destination_label = match &destination {
        LibraryTransferDestination::LocalFolder(folder) => folder.display().to_string(),
        LibraryTransferDestination::LocalLibrary => tr("the library").to_owned(),
    };

    let result = image_paste_summary(mode, total, completed, &destination_label, errors);
    let remaining_clipboard = if mode == ImageClipboardMode::Cut && !remaining.is_empty() {
        Some(ImageClipboard {
            mode,
            assets: remaining,
        })
    } else {
        None
    };
    AssetTransferCompletion {
        result,
        clear_clipboard: mode == ImageClipboardMode::Cut && remaining_clipboard.is_none(),
        remaining_clipboard,
    }
}

pub(super) fn run_duplicate_assets(
    assets: Vec<LibraryAsset>,
    platform: &dyn Platform,
) -> AssetTransferCompletion {
    let total = assets.len();
    let mut completed = 0usize;
    let mut errors = Vec::new();
    for asset in assets {
        let result = duplicate_destination(&asset).and_then(|destination| {
            copy_asset_to_destination(&asset, &destination, platform).map(|_| ())
        });
        match result {
            Ok(()) => completed += 1,
            Err(error) => errors.push(format!("{}: {error}", asset.display_name)),
        }
    }
    let one = total == 1;
    let result = match (errors.is_empty(), one) {
        (true, true) => Ok(tr_fmt(
            "Duplicated {completed} selected image",
            &[("completed", &completed)],
        )),
        (true, false) => Ok(tr_fmt(
            "Duplicated {completed} selected images",
            &[("completed", &completed)],
        )),
        (false, _) => Err(tr_fmt(
            "Duplicated {completed} of {total} selected images. {details}",
            &[
                ("completed", &completed),
                ("total", &total),
                ("details", &errors.join(" · ")),
            ],
        )),
    };
    AssetTransferCompletion {
        result,
        clear_clipboard: false,
        remaining_clipboard: None,
    }
}

pub(super) fn start_duplicate_assets(
    app: &mut CalibRawApp,
    assets: &[LibraryAsset],
    context: &egui::Context,
) {
    if app.library.local_mutation_in_progress() {
        app.library.status = tr("Another Library asset transfer is still running.").to_owned();
        return;
    }
    if assets.is_empty() {
        return;
    }
    let assets = assets.to_vec();
    let total = assets.len();
    let (sender, receiver) = mpsc::channel();
    app.library.asset_transfer_receiver = Some(receiver);
    app.library.status = if total == 1 {
        tr_fmt(
            "Duplicating {display_name}…",
            &[("display_name", &assets[0].display_name)],
        )
    } else {
        tr_fmt("Duplicating {total} selected images…", &[("total", &total)])
    };
    let repaint = context.clone();
    let platform = Arc::clone(&app.library.platform);
    let spawn = std::thread::Builder::new()
        .name("calibraw-library-duplicate".to_owned())
        .spawn(move || {
            let completion = run_duplicate_assets(assets, &*platform);
            let _ = sender.send(completion);
            repaint.request_repaint();
        });
    if let Err(error) = spawn {
        app.library.asset_transfer_receiver = None;
        app.library.status = tr_fmt(
            "Could not start duplicate operation: {error}",
            &[("error", &error)],
        );
    }
}

pub(super) fn start_image_clipboard_paste(
    app: &mut CalibRawApp,
    destination: LibraryTransferDestination,
    context: &egui::Context,
) {
    if app.library.local_mutation_in_progress() {
        app.library.status = tr("Another Library asset transfer is still running.").to_owned();
        return;
    }
    let Some(clipboard) = app.library.image_clipboard.clone() else {
        app.library.status = tr("Copy or cut Library images first.").to_owned();
        return;
    };
    let (sender, receiver) = mpsc::channel();
    app.library.asset_transfer_receiver = Some(receiver);
    app.library.status = tr_fmt(
        "Pasting {paste_label}…",
        &[("paste_label", &clipboard.paste_label())],
    );
    let repaint = context.clone();
    let platform = Arc::clone(&app.library.platform);
    let spawn = std::thread::Builder::new()
        .name("calibraw-library-paste".to_owned())
        .spawn(move || {
            let completion = run_image_paste(clipboard, destination, &*platform);
            let _ = sender.send(completion);
            repaint.request_repaint();
        });
    if let Err(error) = spawn {
        app.library.asset_transfer_receiver = None;
        app.library.status = tr_fmt(
            "Could not start Library paste: {error}",
            &[("error", &error)],
        );
    }
}
