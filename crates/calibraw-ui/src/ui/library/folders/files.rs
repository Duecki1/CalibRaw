use super::super::*;
use crate::i18n::{tr, tr_fmt};

fn numbered_file_name(
    original: &std::ffi::OsStr,
    stem: &std::ffi::OsStr,
    extension: Option<&std::ffi::OsStr>,
    number: usize,
) -> OsString {
    if number == 0 {
        return original.to_os_string();
    }

    let mut file_name = stem.to_os_string();
    file_name.push(format!(" ({number})"));
    if let Some(extension) = extension {
        file_name.push(".");
        file_name.push(extension);
    }
    file_name
}

pub(in crate::ui::library) fn copy_file_create_new(
    source: &Path,
    destination: &Path,
) -> io::Result<()> {
    let mut input = OpenOptions::new().read(true).open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let result = io::copy(&mut input, &mut output).and_then(|_| output.sync_all());
    if result.is_err() {
        drop(output);
        let _ = fs::remove_file(destination);
    } else if let Ok(metadata) = fs::metadata(source) {
        let _ = fs::set_permissions(destination, metadata.permissions());
    }
    result.map(|_| ())
}

pub(in crate::ui::library) fn copy_raw_bundle_to_folder(
    source_raw: &Path,
    requested_name: &std::ffi::OsStr,
    destination_folder: &Path,
) -> Result<PathBuf, String> {
    let requested_path = Path::new(requested_name);
    if requested_path.file_name() != Some(requested_name)
        || !crate::pipeline::is_supported_image_path(requested_path)
    {
        return Err(tr("The photo has an unsafe or unsupported filename.").to_owned());
    }
    if !source_raw.is_file() {
        return Err(tr_fmt(
            "{source_raw} is no longer a file.",
            &[("source_raw", &source_raw.display())],
        ));
    }
    if !destination_folder.is_dir() {
        return Err(tr_fmt(
            "The destination folder {destination_folder} no longer exists.",
            &[("destination_folder", &destination_folder.display())],
        ));
    }

    let stem = requested_path
        .file_stem()
        .map(OsString::from)
        .unwrap_or_else(|| requested_name.to_os_string());
    let extension = requested_path.extension().map(OsString::from);
    let source_companions = crate::sidecar::raw_companion_paths(source_raw);
    for number in 0..=10_000usize {
        let file_name = numbered_file_name(requested_name, &stem, extension.as_deref(), number);
        let destination_raw = destination_folder.join(file_name);
        let destination_companions = crate::sidecar::raw_companion_paths(&destination_raw);
        if destination_raw.exists() || destination_companions.iter().any(|path| path.exists()) {
            continue;
        }
        match copy_file_create_new(source_raw, &destination_raw) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(tr_fmt(
                    "Could not copy {source_raw}: {error}",
                    &[("source_raw", &source_raw.display()), ("error", &error)],
                ));
            }
        }
        for (source, destination) in source_companions.iter().zip(&destination_companions) {
            if !source.is_file() {
                continue;
            }
            if let Err(error) = copy_file_create_new(source, destination) {
                let _ = fs::remove_file(&destination_raw);
                for created in &destination_companions {
                    let _ = fs::remove_file(created);
                }
                return Err(tr_fmt(
                    "Could not copy {source}: {error}",
                    &[("source", &source.display()), ("error", &error)],
                ));
            }
        }
        if let Err(error) =
            crate::sidecar::copy_developed_thumbnail_cache(source_raw, &destination_raw)
        {
            log::warn!(
                "could not copy developed thumbnail cache for {}: {error}",
                source_raw.display()
            );
            let _ = crate::sidecar::invalidate_developed_thumbnail_cache(&destination_raw);
        }
        if let Err(error) = calibraw_core::file_ops::sync_parent_directory(destination_folder) {
            log::warn!(
                "could not sync image paste folder {} after copying {}: {error}",
                destination_folder.display(),
                destination_raw.display()
            );
        }
        return Ok(destination_raw);
    }
    Err(tr_fmt(
        "Could not find an unused name for {requested_name} in {destination_folder}.",
        &[
            ("requested_name", &format!("{:?}", requested_name)),
            ("destination_folder", &destination_folder.display()),
        ],
    ))
}

pub(in crate::ui::library) fn remove_local_raw_bundle(raw_path: &Path) -> Result<(), String> {
    fs::remove_file(raw_path).map_err(|error| {
        tr_fmt(
            "Could not remove {raw_path} after copying it: {error}",
            &[("raw_path", &raw_path.display()), ("error", &error)],
        )
    })?;
    for companion in crate::sidecar::raw_companion_paths(raw_path) {
        if let Err(error) = fs::remove_file(&companion) {
            if error.kind() != io::ErrorKind::NotFound {
                log::warn!(
                    "could not remove {} after moving its RAW: {error}",
                    companion.display()
                );
            }
        }
    }
    if let Err(error) = crate::sidecar::invalidate_developed_thumbnail_cache(raw_path) {
        log::warn!("could not remove the old developed thumbnail after moving a RAW: {error}");
    }
    Ok(())
}

pub(in crate::ui::library) fn rename_raw_bundle(
    source_raw: &Path,
    requested_name: &str,
) -> Result<PathBuf, String> {
    validate_library_item_name(requested_name, true)?;
    let parent = source_raw
        .parent()
        .ok_or_else(|| tr("The RAW has no parent folder.").to_owned())?;
    let destination_raw = parent.join(requested_name);
    if destination_raw == source_raw {
        return Ok(destination_raw);
    }
    let source_companions = crate::sidecar::raw_companion_paths(source_raw);
    let destination_companions = crate::sidecar::raw_companion_paths(&destination_raw);
    if destination_raw.exists() || destination_companions.iter().any(|path| path.exists()) {
        return Err(tr_fmt(
            "{destination_raw} already exists.",
            &[("destination_raw", &destination_raw.display())],
        ));
    }
    let developed_thumbnail = match crate::sidecar::load_developed_thumbnail_cache(source_raw, 8192)
    {
        Ok(thumbnail) => thumbnail,
        Err(error) => {
            log::warn!(
                "could not read developed thumbnail cache before renaming {}: {error}",
                source_raw.display()
            );
            None
        }
    };
    fs::rename(source_raw, &destination_raw).map_err(|error| {
        tr_fmt(
            "Could not rename {source_raw} to {destination_raw}: {error}",
            &[
                ("source_raw", &source_raw.display()),
                ("destination_raw", &destination_raw.display()),
                ("error", &error),
            ],
        )
    })?;
    let mut renamed = Vec::new();
    for (source, destination) in source_companions.iter().zip(&destination_companions) {
        if !source.is_file() {
            continue;
        }
        if let Err(error) = fs::rename(source, destination) {
            let rollback = renamed
                .iter()
                .try_for_each(|&(source, destination)| fs::rename(destination, source))
                .and_then(|()| fs::rename(&destination_raw, source_raw));
            return Err(if rollback.is_ok() {
                tr_fmt(
                    "Could not rename {source}: {error}",
                    &[("source", &source.display()), ("error", &error)],
                )
            } else {
                tr_fmt("The RAW was renamed to {destination_raw}, but {source} could not be renamed: {error}", &[("destination_raw", &destination_raw.display()), ("source", &source.display()), ("error", &error)])
            });
        }
        renamed.push((source, destination));
    }
    if let Some(thumbnail) = developed_thumbnail {
        let thumbnail_result = crate::sidecar::desktop_sidecar_fingerprint(&destination_raw)
            .and_then(|fingerprint| {
                fingerprint
                    .ok_or_else(|| tr("The renamed RAW's edit sidecar disappeared.").to_owned())
            })
            .and_then(|fingerprint| {
                crate::sidecar::save_developed_thumbnail_cache(
                    &destination_raw,
                    &thumbnail,
                    fingerprint,
                )
                .map(|_| ())
            });
        if let Err(error) = thumbnail_result {
            log::warn!(
                "could not preserve developed thumbnail cache while renaming {}: {error}",
                source_raw.display()
            );
            let _ = crate::sidecar::invalidate_developed_thumbnail_cache(&destination_raw);
        }
    }
    if let Err(error) = crate::sidecar::invalidate_developed_thumbnail_cache(source_raw) {
        log::warn!("could not clear the old thumbnail after renaming a RAW: {error}");
    }
    if let Err(error) = calibraw_core::file_ops::sync_parent_directory(parent) {
        log::warn!(
            "could not sync RAW folder {} after renaming {}: {error}",
            parent.display(),
            destination_raw.display()
        );
    }
    Ok(destination_raw)
}

pub(in crate::ui::library) fn import_raw_into_folder(
    source: &Path,
    folder: &Path,
) -> Result<RawImportOutcome, String> {
    let original_name = source.file_name().map(OsString::from).ok_or_else(|| {
        tr_fmt(
            "{source} has no file name",
            &[("source", &source.display())],
        )
    })?;
    let stem = source
        .file_stem()
        .map(OsString::from)
        .unwrap_or_else(|| original_name.clone());
    let extension = source.extension().map(OsString::from);

    for number in 0..=10_000usize {
        let file_name = numbered_file_name(&original_name, &stem, extension.as_deref(), number);
        let destination = folder.join(file_name);

        if destination.exists() {
            if same_existing_file(source, &destination) {
                return Ok(RawImportOutcome::AlreadyPresent);
            }
            continue;
        }

        match copy_file_create_new(source, &destination) {
            Ok(()) => {
                if let Err(error) = calibraw_core::file_ops::sync_parent_directory(folder) {
                    log::warn!(
                        "could not sync RAW import folder {} after copying {}: {error}",
                        folder.display(),
                        destination.display()
                    );
                }
                return Ok(RawImportOutcome::Imported(destination));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(tr_fmt(
                    "Could not import {source} into {folder}: {error}",
                    &[
                        ("source", &source.display()),
                        ("folder", &folder.display()),
                        ("error", &error),
                    ],
                ));
            }
        }
    }

    Err(tr_fmt(
        "Could not find an unused name for {source} in {folder}",
        &[("source", &source.display()), ("folder", &folder.display())],
    ))
}

pub(in crate::ui::library) fn same_existing_file(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

pub(in crate::ui::library) fn validate_folder_name(name: &str) -> Result<OsString, String> {
    if name.is_empty() || name.trim() != name {
        return Err(tr("Folder names cannot be empty or start/end with whitespace.").to_owned());
    }
    let path = Path::new(name);
    let mut components = path.components();
    let Some(std::path::Component::Normal(component)) = components.next() else {
        return Err(tr("Enter a single folder name, without a path.").to_owned());
    };
    if components.next().is_some() {
        return Err(tr("Enter a single folder name, without a path.").to_owned());
    }
    Ok(component.to_os_string())
}

pub(in crate::ui::library) fn canonical_library_directory(
    root: &Path,
    path: &Path,
    allow_root: bool,
) -> Result<PathBuf, String> {
    let root = fs::canonicalize(root).map_err(|error| {
        tr_fmt(
            "Could not resolve library root {root}: {error}",
            &[("root", &root.display()), ("error", &error)],
        )
    })?;
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        tr_fmt(
            "Could not inspect folder {path}: {error}",
            &[("path", &path.display()), ("error", &error)],
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(tr_fmt(
            "{path} is not a real folder",
            &[("path", &path.display())],
        ));
    }
    let resolved = fs::canonicalize(path).map_err(|error| {
        tr_fmt(
            "Could not resolve folder {path}: {error}",
            &[("path", &path.display()), ("error", &error)],
        )
    })?;
    if !resolved.starts_with(&root) {
        return Err(tr_fmt(
            "Refusing to operate outside the library root: {path}",
            &[("path", &path.display())],
        ));
    }
    if !allow_root && resolved == root {
        return Err(
            tr("The top-level library folder cannot be moved, renamed, or deleted.").to_owned(),
        );
    }
    Ok(resolved)
}

pub(in crate::ui::library) fn path_entry_exists(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(error) => error.kind() != io::ErrorKind::NotFound,
    }
}

pub(in crate::ui::library) fn unique_folder_destination(
    parent: &Path,
    name: &std::ffi::OsStr,
) -> Result<PathBuf, String> {
    for number in 0..=10_000usize {
        let file_name = if number == 0 {
            OsString::from(name)
        } else {
            let mut copy_name = OsString::from(name);
            if number == 1 {
                copy_name.push(" copy");
            } else {
                copy_name.push(format!(" copy {number}"));
            }
            copy_name
        };
        let destination = parent.join(file_name);
        if !path_entry_exists(&destination) {
            return Ok(destination);
        }
    }
    Err(tr_fmt(
        "Could not find an unused folder name in {parent}",
        &[("parent", &parent.display())],
    ))
}

pub(in crate::ui::library) fn copy_directory_create_new(
    source: &Path,
    destination: &Path,
) -> Result<(), String> {
    let source_metadata = fs::symlink_metadata(source).map_err(|error| {
        tr_fmt(
            "Could not inspect {source}: {error}",
            &[("source", &source.display()), ("error", &error)],
        )
    })?;
    if source_metadata.file_type().is_symlink() || !source_metadata.is_dir() {
        return Err(tr_fmt(
            "{source} is not a real folder",
            &[("source", &source.display())],
        ));
    }
    let source_resolved = fs::canonicalize(source).map_err(|error| {
        tr_fmt(
            "Could not resolve {source}: {error}",
            &[("source", &source.display()), ("error", &error)],
        )
    })?;
    let destination_parent = destination.parent().ok_or_else(|| {
        tr_fmt(
            "{destination} has no parent folder",
            &[("destination", &destination.display())],
        )
    })?;
    let destination_parent_resolved = fs::canonicalize(destination_parent).map_err(|error| {
        tr_fmt(
            "Could not resolve destination {destination_parent}: {error}",
            &[
                ("destination_parent", &destination_parent.display()),
                ("error", &error),
            ],
        )
    })?;
    if destination_parent_resolved.starts_with(&source_resolved) {
        return Err(
            tr("A folder cannot be copied into itself or one of its subfolders.").to_owned(),
        );
    }

    fn copy_contents(source: &Path, destination: &Path) -> Result<(), String> {
        for entry in fs::read_dir(source).map_err(|error| {
            tr_fmt(
                "Could not read {source}: {error}",
                &[("source", &source.display()), ("error", &error)],
            )
        })? {
            let entry = entry.map_err(|error| {
                tr_fmt(
                    "Could not read an entry in {source}: {error}",
                    &[("source", &source.display()), ("error", &error)],
                )
            })?;
            let source_path = entry.path();
            let destination_path = destination.join(entry.file_name());
            let file_type = entry.file_type().map_err(|error| {
                tr_fmt(
                    "Could not inspect {source_path}: {error}",
                    &[("source_path", &source_path.display()), ("error", &error)],
                )
            })?;
            if file_type.is_symlink() {
                return Err(tr_fmt(
                    "Refusing to follow symbolic link {source_path} while copying a folder",
                    &[("source_path", &source_path.display())],
                ));
            }
            if file_type.is_dir() {
                fs::create_dir(&destination_path).map_err(|error| {
                    tr_fmt(
                        "Could not create {destination_path}: {error}",
                        &[
                            ("destination_path", &destination_path.display()),
                            ("error", &error),
                        ],
                    )
                })?;
                copy_contents(&source_path, &destination_path)?;
                if let Err(error) =
                    calibraw_core::file_ops::sync_parent_directory(&destination_path)
                {
                    log::warn!(
                        "could not sync copied folder {}: {error}",
                        destination_path.display()
                    );
                }
            } else if file_type.is_file() {
                copy_file_create_new(&source_path, &destination_path).map_err(|error| {
                    tr_fmt(
                        "Could not copy {source_path}: {error}",
                        &[("source_path", &source_path.display()), ("error", &error)],
                    )
                })?;
            } else {
                return Err(tr_fmt(
                    "Refusing to copy special filesystem entry {source_path}",
                    &[("source_path", &source_path.display())],
                ));
            }
        }
        Ok(())
    }

    fs::create_dir(destination).map_err(|error| {
        tr_fmt(
            "Could not create {destination}: {error}",
            &[("destination", &destination.display()), ("error", &error)],
        )
    })?;
    if let Err(error) = copy_contents(source, destination) {
        if let Err(cleanup_error) = fs::remove_dir_all(destination) {
            return Err(tr_fmt(
                "{error} Cleanup of incomplete folder {destination} also failed: {cleanup_error}",
                &[
                    ("error", &error),
                    ("destination", &destination.display()),
                    ("cleanup_error", &cleanup_error),
                ],
            ));
        }
        return Err(error);
    }
    if let Err(error) = calibraw_core::file_ops::sync_parent_directory(destination_parent) {
        log::warn!(
            "could not sync folder {} after copying {}: {error}",
            destination_parent.display(),
            destination.display()
        );
    }
    Ok(())
}

pub(in crate::ui::library) fn import_folder_into_library(
    source: &Path,
    folder: &Path,
) -> Result<PathBuf, String> {
    let source_name = source.file_name().ok_or_else(|| {
        tr_fmt(
            "{source} has no folder name",
            &[("source", &source.display())],
        )
    })?;
    let destination = unique_folder_destination(folder, source_name)?;
    copy_directory_create_new(source, &destination).map_err(|error| {
        tr_fmt(
            "Could not import folder {source} into {folder}: {error}",
            &[
                ("source", &source.display()),
                ("folder", &folder.display()),
                ("error", &error),
            ],
        )
    })?;
    Ok(destination)
}

pub(in crate::ui::library) fn folder_operation_progress_status(
    operation: &LibraryFolderOperation,
) -> String {
    match operation {
        LibraryFolderOperation::Create { parent, .. } => tr_fmt(
            "Creating a folder in {parent}…",
            &[("parent", &parent.display())],
        ),
        LibraryFolderOperation::Copy { source, .. } => {
            tr_fmt("Copying folder {source}…", &[("source", &source.display())])
        }
        LibraryFolderOperation::Move { source, .. } => {
            tr_fmt("Moving folder {source}…", &[("source", &source.display())])
        }
        LibraryFolderOperation::Delete { target, .. } => tr_fmt(
            "Deleting folder {target}…",
            &[("target", &target.display())],
        ),
    }
}

pub(in crate::ui::library) fn run_folder_operation(
    operation: LibraryFolderOperation,
) -> Result<LibraryFolderOperationResult, String> {
    match operation {
        LibraryFolderOperation::Create { root, parent, name } => {
            canonical_library_directory(&root, &parent, true)?;
            let name = validate_folder_name(&name)?;
            let destination = parent.join(name);
            fs::create_dir(&destination).map_err(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    tr_fmt(
                        "A folder named {destination} already exists.",
                        &[("destination", &destination.display())],
                    )
                } else {
                    tr_fmt(
                        "Could not create folder {destination}: {error}",
                        &[("destination", &destination.display()), ("error", &error)],
                    )
                }
            })?;
            if let Err(error) = calibraw_core::file_ops::sync_parent_directory(&parent) {
                log::warn!("could not sync folder {}: {error}", parent.display());
            }
            Ok(LibraryFolderOperationResult::Created(destination))
        }
        LibraryFolderOperation::Copy {
            root,
            source,
            destination_parent,
        } => {
            canonical_library_directory(&root, &source, false)?;
            canonical_library_directory(&root, &destination_parent, true)?;
            let name = source.file_name().ok_or_else(|| {
                tr_fmt(
                    "{source} has no folder name",
                    &[("source", &source.display())],
                )
            })?;
            let destination = unique_folder_destination(&destination_parent, name)?;
            copy_directory_create_new(&source, &destination)?;
            Ok(LibraryFolderOperationResult::Copied {
                source,
                destination,
            })
        }
        LibraryFolderOperation::Move {
            root,
            source,
            destination_parent,
            new_name,
        } => {
            let source_resolved = canonical_library_directory(&root, &source, false)?;
            let destination_parent_resolved =
                canonical_library_directory(&root, &destination_parent, true)?;
            if destination_parent_resolved.starts_with(&source_resolved) {
                return Err(
                    tr("A folder cannot be moved into itself or one of its subfolders.").to_owned(),
                );
            }
            let name = match new_name {
                Some(name) => validate_folder_name(&name)?,
                None => source.file_name().map(OsString::from).ok_or_else(|| {
                    tr_fmt(
                        "{source} has no folder name",
                        &[("source", &source.display())],
                    )
                })?,
            };
            let destination = destination_parent.join(name);
            if destination == source {
                return Err(tr("The folder is already in that location.").to_owned());
            }
            if path_entry_exists(&destination) {
                return Err(tr_fmt(
                    "A folder named {destination} already exists; nothing was overwritten.",
                    &[("destination", &destination.display())],
                ));
            }
            fs::rename(&source, &destination).map_err(|error| {
                tr_fmt(
                    "Could not move {source} to {destination}: {error}",
                    &[
                        ("source", &source.display()),
                        ("destination", &destination.display()),
                        ("error", &error),
                    ],
                )
            })?;
            if let Some(parent) = source.parent() {
                if let Err(error) = calibraw_core::file_ops::sync_parent_directory(parent) {
                    log::warn!("could not sync source folder {}: {error}", parent.display());
                }
            }
            if let Err(error) = calibraw_core::file_ops::sync_parent_directory(&destination_parent)
            {
                log::warn!(
                    "could not sync destination folder {}: {error}",
                    destination_parent.display()
                );
            }
            Ok(LibraryFolderOperationResult::Moved {
                source,
                destination,
            })
        }
        LibraryFolderOperation::Delete { root, target } => {
            canonical_library_directory(&root, &target, false)?;
            fs::remove_dir_all(&target).map_err(|error| {
                tr_fmt(
                    "Could not delete folder {target}: {error}",
                    &[("target", &target.display()), ("error", &error)],
                )
            })?;
            if let Some(parent) = target.parent() {
                if let Err(error) = calibraw_core::file_ops::sync_parent_directory(parent) {
                    log::warn!(
                        "could not sync folder {} after deletion: {error}",
                        parent.display()
                    );
                }
            }
            Ok(LibraryFolderOperationResult::Deleted(target))
        }
    }
}

pub(in crate::ui::library) fn raw_import_status(result: &RawImportResult) -> String {
    let mut parts = Vec::new();
    if !result.imported.is_empty() {
        let count = result.imported.len();
        parts.push(if count == 1 {
            tr_fmt("Imported {count} photo", &[("count", &count)])
        } else {
            tr_fmt("Imported {count} photos", &[("count", &count)])
        });
    }
    if !result.imported_folders.is_empty() {
        let count = result.imported_folders.len();
        parts.push(if count == 1 {
            tr_fmt("Imported {count} folder", &[("count", &count)])
        } else {
            tr_fmt("Imported {count} folders", &[("count", &count)])
        });
    }
    if result.already_present > 0 {
        parts.push(tr_fmt(
            "{already_present} already in this folder",
            &[("already_present", &result.already_present)],
        ));
    }
    if result.ignored > 0 {
        let count = result.ignored;
        parts.push(if count == 1 {
            tr_fmt(
                "ignored {count} unsupported or inaccessible item",
                &[("count", &count)],
            )
        } else {
            tr_fmt(
                "ignored {count} unsupported or inaccessible items",
                &[("count", &count)],
            )
        });
    }
    if !result.failures.is_empty() {
        parts.push(tr_fmt(
            "{count} failed",
            &[("count", &result.failures.len())],
        ));
        parts.push(result.failures.join(" · "));
    }
    if parts.is_empty() {
        tr("No RAW files or folders were imported.").to_owned()
    } else {
        format!("{}.", parts.join("; "))
    }
}
