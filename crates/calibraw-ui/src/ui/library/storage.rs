use super::*;
use crate::i18n::{tr, tr_fmt};

pub(super) fn desktop_paths(assets: &[LibraryAsset]) -> Vec<PathBuf> {
    assets
        .iter()
        .filter_map(|asset| asset.desktop_path().map(Path::to_path_buf))
        .collect()
}

pub(super) fn android_targets(assets: &[LibraryAsset]) -> Vec<(String, String)> {
    assets
        .iter()
        .filter_map(|asset| {
            asset
                .android_uri()
                .map(|uri| (uri.to_owned(), asset.display_name.clone()))
        })
        .collect()
}

pub(super) fn reset_asset_adjustments(
    app: &mut CalibRawApp,
    asset: &LibraryAsset,
) -> Result<bool, String> {
    match &asset.locator {
        LibraryLocator::Desktop(path) => {
            if app.develop.current_path.as_deref() == Some(path.as_path()) {
                crate::sidecar::reset_desktop_adjustments_with_editing_time(
                    path,
                    app.raw_editing_time_ms(),
                )
            } else {
                crate::sidecar::reset_desktop_adjustments(path)
            }
        }
        LibraryLocator::Android { uri } => {
            app.reset_android_library_adjustments(uri, &asset.display_name)?;
            Ok(true)
        }
    }
}

pub(super) fn rename_asset(
    app: &mut CalibRawApp,
    asset: &LibraryAsset,
    requested_name: &str,
) -> Result<LibraryAsset, String> {
    match &asset.locator {
        LibraryLocator::Desktop(path) => {
            let destination = rename_raw_bundle(path, requested_name)?;
            let mut renamed = asset.clone();
            renamed.id = LibraryAssetId::Desktop(destination.clone());
            renamed.display_name = requested_name.to_owned();
            renamed.display_path = destination.display().to_string();
            renamed.locator = LibraryLocator::Desktop(destination);
            Ok(renamed)
        }
        LibraryLocator::Android { uri } => {
            let renamed_uri =
                app.rename_android_library_item(uri, &asset.display_name, requested_name)?;
            let mut renamed = asset.clone();
            renamed.id = LibraryAssetId::Android(renamed_uri.clone());
            renamed.display_name = requested_name.to_owned();
            renamed.display_path = Path::new(&asset.display_path)
                .parent()
                .map(|parent| parent.join(requested_name).display().to_string())
                .unwrap_or_else(|| requested_name.to_owned());
            renamed.locator = LibraryLocator::Android { uri: renamed_uri };
            Ok(renamed)
        }
    }
}

pub(super) fn delete_library_asset(
    app: &mut CalibRawApp,
    asset: &LibraryAsset,
) -> Result<(), String> {
    match &asset.locator {
        LibraryLocator::Desktop(path) => {
            let was_current = app.develop.current_path.as_deref() == Some(path.as_path());
            if was_current {
                app.detach_current_file_for_library_action(path);
            }
            recycle_local_raw_bundle(path)?;
            if was_current {
                app.develop.current_path = None;
            }
            Ok(())
        }
        LibraryLocator::Android { uri } => {
            app.delete_android_library_item(uri, &asset.display_name)
        }
    }
}

fn recycle_local_raw_bundle(raw_path: &Path) -> Result<(), String> {
    crate::platform::trash(raw_path).map_err(|error| {
        tr_fmt(
            "Could not move {raw_path} to the system {trash_name}: {error}",
            &[
                ("raw_path", &raw_path.display()),
                ("trash_name", &crate::platform::trash_name()),
                ("error", &error),
            ],
        )
    })?;

    for companion in crate::sidecar::raw_companion_paths(raw_path) {
        if !companion.is_file() {
            continue;
        }
        if let Err(error) = crate::platform::trash(&companion) {
            log::warn!(
                "moved RAW {} to the system {} but could not move {}: {error}",
                raw_path.display(),
                crate::platform::trash_name(),
                companion.display()
            );
        }
    }
    if let Err(error) = crate::sidecar::invalidate_developed_thumbnail_cache(raw_path) {
        log::warn!("could not remove the developed thumbnail after recycling a RAW: {error}");
    }
    Ok(())
}

#[derive(Debug)]
pub(super) struct MaterializedLibraryAsset {
    pub(super) raw_path: PathBuf,
    pub(super) display_name: String,
    cleanup: bool,
}

impl MaterializedLibraryAsset {
    pub(super) fn cleanup(&self) {
        if !self.cleanup {
            return;
        }
        let _ = fs::remove_file(&self.raw_path);
        let _ = fs::remove_file(crate::sidecar::sidecar_path_for_raw(&self.raw_path));
    }
}

/// A local file with the photo and its sidecar: the photo itself for a
/// folder photo, a temporary copy for a media-library photo.
pub(super) fn materialize_library_asset(
    asset: &LibraryAsset,
    platform: &dyn Platform,
) -> Result<MaterializedLibraryAsset, String> {
    match &asset.locator {
        LibraryLocator::Desktop(path) => Ok(MaterializedLibraryAsset {
            raw_path: path.clone(),
            display_name: asset.display_name.clone(),
            cleanup: false,
        }),
        LibraryLocator::Android { uri } => {
            let raw_path = platform.materialize_library_document(uri, &asset.display_name)?;
            let sidecar = platform.materialize_raw_sidecar(uri, &asset.display_name)?;
            if let Some(sidecar) = sidecar {
                let destination = crate::sidecar::sidecar_path_for_raw(&raw_path);
                let copy_result = fs::copy(&sidecar, &destination)
                    .map(|_| ())
                    .map_err(|error| {
                        tr_fmt(
                            "could not stage {display_name} sidecar: {error}",
                            &[("display_name", &asset.display_name), ("error", &error)],
                        )
                    });
                let _ = fs::remove_file(sidecar);
                if let Err(error) = copy_result {
                    let _ = fs::remove_file(&raw_path);
                    return Err(error);
                }
            }
            Ok(MaterializedLibraryAsset {
                raw_path,
                display_name: asset.display_name.clone(),
                cleanup: true,
            })
        }
    }
}

pub(super) fn asset_is_at_destination(
    asset: &LibraryAsset,
    destination: &LibraryTransferDestination,
) -> bool {
    match destination {
        LibraryTransferDestination::LocalFolder(folder) => asset
            .desktop_path()
            .and_then(Path::parent)
            .is_some_and(|parent| parent == folder.as_path()),
        LibraryTransferDestination::LocalLibrary => false,
    }
}

pub(super) fn duplicate_destination(
    asset: &LibraryAsset,
) -> Result<LibraryTransferDestination, String> {
    match &asset.locator {
        LibraryLocator::Desktop(path) => {
            let parent = path
                .parent()
                .ok_or_else(|| tr("Library asset has no destination folder").to_owned())?;
            Ok(LibraryTransferDestination::LocalFolder(parent.to_owned()))
        }
        LibraryLocator::Android { .. } => Ok(LibraryTransferDestination::LocalLibrary),
    }
}

#[derive(Debug)]
pub(super) enum ImportedLibraryAsset {
    Desktop(PathBuf),
    Android(crate::platform::ImportedLibraryDocument),
}

pub(super) fn import_materialized_library_asset(
    materialized: &MaterializedLibraryAsset,
    destination: &LibraryTransferDestination,
    platform: &dyn Platform,
) -> Result<ImportedLibraryAsset, String> {
    match destination {
        LibraryTransferDestination::LocalFolder(folder) => {
            let destination = copy_raw_bundle_to_folder(
                &materialized.raw_path,
                std::ffi::OsStr::new(&materialized.display_name),
                folder,
            )?;
            Ok(ImportedLibraryAsset::Desktop(destination))
        }
        LibraryTransferDestination::LocalLibrary => platform
            .import_local_library_document(&materialized.raw_path, &materialized.display_name)
            .map(ImportedLibraryAsset::Android),
    }
}

/// Carries a media-library photo's developed thumbnail over to its copy;
/// folder copies take theirs along with the sidecar.
pub(super) fn preserve_imported_thumbnail(
    asset: &LibraryAsset,
    imported: &ImportedLibraryAsset,
    platform: &dyn Platform,
) -> Result<(), String> {
    match imported {
        ImportedLibraryAsset::Desktop(_) => Ok(()),
        ImportedLibraryAsset::Android(imported) => {
            let source_uri = asset.android_uri().ok_or_else(|| {
                tr("Library asset is not available from Android storage").to_owned()
            })?;
            platform.copy_library_developed_thumbnail_cache(source_uri, &imported.uri)
        }
    }
}

pub(super) fn remove_library_asset(asset: &LibraryAsset) -> Result<(), String> {
    let path = asset
        .desktop_path()
        .ok_or_else(|| tr("Library asset is not available from desktop storage").to_owned())?;
    remove_local_raw_bundle(path)
}

pub(super) fn rollback_imported_library_asset(
    imported: ImportedLibraryAsset,
    platform: &dyn Platform,
) {
    match imported {
        ImportedLibraryAsset::Desktop(path) => {
            if let Err(error) = remove_local_raw_bundle(&path) {
                log::warn!(
                    "could not roll back imported Library bundle {}: {error}",
                    path.display()
                );
            }
        }
        ImportedLibraryAsset::Android(imported) => {
            if let Err(error) =
                platform.delete_imported_library_document(&imported.uri, &imported.display_name)
            {
                log::warn!("could not roll back imported Android Library bundle: {error}");
            }
        }
    }
}
