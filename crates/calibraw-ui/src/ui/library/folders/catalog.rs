use super::super::*;
use crate::i18n::tr_fmt;
use calibraw_core::display_metadata_cache::{FileStamp, FolderDisplayMetadataCache};
use std::collections::BinaryHeap;
use std::time::UNIX_EPOCH;

pub(in crate::ui::library) struct RankedLibraryAsset {
    asset: LibraryAsset,
    lowercase_name: String,
    /// Validates the folder's display-metadata cache; not part of the ranking.
    stamp: Option<FileStamp>,
}

impl RankedLibraryAsset {
    fn new(asset: LibraryAsset, stamp: Option<FileStamp>) -> Self {
        let lowercase_name = asset.display_name.to_lowercase();
        Self {
            asset,
            lowercase_name,
            stamp,
        }
    }
}

impl PartialEq for RankedLibraryAsset {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == CmpOrdering::Equal
    }
}

impl Eq for RankedLibraryAsset {}

impl PartialOrd for RankedLibraryAsset {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

impl Ord for RankedLibraryAsset {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        // Which photos a truncated library keeps follows the order the displayed
        // list is sorted in: the date a photo was taken once its capture date is
        // known, otherwise the file's own date, which is all a first scan knows.
        other
            .asset
            .metadata
            .date_order_seconds()
            .cmp(&self.asset.metadata.date_order_seconds())
            .then_with(|| self.lowercase_name.cmp(&other.lowercase_name))
            .then_with(|| self.asset.display_path.cmp(&other.asset.display_path))
            .then_with(|| self.asset.id.cmp(&other.asset.id))
    }
}

pub(in crate::ui::library) struct FolderScan {
    pub(in crate::ui::library) assets: Vec<LibraryAsset>,
    pub(in crate::ui::library) warning_count: usize,
    pub(in crate::ui::library) truncated: bool,
    pub(in crate::ui::library) deferred_metadata: DeferredFolderMetadata,
}

pub(in crate::ui::library) fn scan_folder_tree(
    root: &Path,
    is_cancelled: impl Fn() -> bool,
) -> Option<LibraryFolderNode> {
    fn visit(path: PathBuf, is_cancelled: &impl Fn() -> bool) -> Option<LibraryFolderNode> {
        if is_cancelled() {
            return None;
        }

        let mut node = LibraryFolderNode::empty(path.clone());
        let entries = match std::fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(error) => {
                log::warn!(
                    "could not read library folder hierarchy at {}: {error}",
                    path.display()
                );
                return Some(node);
            }
        };

        for entry in entries {
            if is_cancelled() {
                return None;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    log::warn!("could not read a library folder entry: {error}");
                    continue;
                }
            };
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) => {
                    log::warn!("could not inspect {}: {error}", entry.path().display());
                    continue;
                }
            };
            if !file_type.is_dir() {
                continue;
            }
            if let Some(child) = visit(entry.path(), is_cancelled) {
                node.children.push(child);
            } else {
                return None;
            }
        }

        node.children.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.path.cmp(&right.path))
        });
        Some(node)
    }

    visit(root.to_path_buf(), &is_cancelled)
}

/// The photos of `folder` and, with `include_subfolders`, of every folder
/// below it, newest first up to the library limit. Symbolic links to folders
/// are not followed, so a walk cannot loop.
pub(in crate::ui::library) fn scan_folder(
    folder: &Path,
    include_subfolders: bool,
    is_cancelled: impl Fn() -> bool,
) -> Result<Option<FolderScan>, String> {
    scan_folder_with_limit(folder, include_subfolders, MAX_LIBRARY_FILES, is_cancelled)
}

pub(in crate::ui::library) fn scan_folder_with_limit(
    folder: &Path,
    include_subfolders: bool,
    maximum_files: usize,
    is_cancelled: impl Fn() -> bool,
) -> Result<Option<FolderScan>, String> {
    if is_cancelled() {
        return Ok(None);
    }
    let metadata = std::fs::metadata(folder).map_err(|error| {
        tr_fmt(
            "Could not read {folder}: {error}",
            &[("folder", &folder.display()), ("error", &error)],
        )
    })?;
    if !metadata.is_dir() {
        return Err(tr_fmt(
            "{folder} is not a folder",
            &[("folder", &folder.display())],
        ));
    }

    let mut ranked_assets = BinaryHeap::with_capacity(maximum_files);
    let mut warning_count = 0usize;
    let mut truncated = false;
    let mut unread_folders = vec![folder.to_path_buf()];
    while let Some(current) = unread_folders.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(entries) => entries,
            // The chosen folder must be readable; a subfolder only counts as
            // an unreadable item.
            Err(error) if current == folder => {
                return Err(tr_fmt(
                    "Could not scan {folder}: {error}",
                    &[("folder", &folder.display()), ("error", &error)],
                ));
            }
            Err(error) => {
                warning_count += 1;
                log::warn!("could not scan {}: {error}", current.display());
                continue;
            }
        };
        for entry in entries {
            if is_cancelled() {
                return Ok(None);
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    warning_count += 1;
                    log::warn!("could not read a library directory entry: {error}");
                    continue;
                }
            };
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) => {
                    warning_count += 1;
                    log::warn!("could not inspect {}: {error}", entry.path().display());
                    continue;
                }
            };
            if include_subfolders && file_type.is_dir() {
                unread_folders.push(entry.path());
                continue;
            }
            if !file_type.is_file() || !is_supported_image_path(&entry.path()) {
                continue;
            }
            let path = entry.path();
            let file_metadata = entry.metadata().ok();
            let modified_seconds = file_metadata
                .as_ref()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_secs());
            let asset = LibraryAsset::from_desktop_path(
                path,
                file_metadata.as_ref().map_or(0, std::fs::Metadata::len),
                modified_seconds,
                None,
            );
            let stamp = file_metadata.as_ref().map(FileStamp::from_metadata);
            let candidate = RankedLibraryAsset::new(asset, stamp);
            if ranked_assets.len() < maximum_files {
                ranked_assets.push(candidate);
            } else {
                truncated = true;
                if ranked_assets
                    .peek()
                    .is_some_and(|worst_retained| candidate < *worst_retained)
                {
                    ranked_assets.pop();
                    ranked_assets.push(candidate);
                }
            }
        }
    }
    let mut ranked_assets = ranked_assets.into_vec();
    ranked_assets.sort();

    // The chosen folder's cache is always loaded, so it drops the entries of
    // its removed photos even when none are left; a subfolder's is loaded
    // once a photo of it is shown.
    let mut deferred_metadata = DeferredFolderMetadata::new(0);
    let mut folder_slots = HashMap::from([(
        folder.to_path_buf(),
        deferred_metadata.add_folder(FolderDisplayMetadataCache::load(folder)),
    )]);
    let mut assets = Vec::with_capacity(ranked_assets.len());
    for RankedLibraryAsset {
        mut asset, stamp, ..
    } in ranked_assets
    {
        if is_cancelled() {
            return Ok(None);
        }
        if let Some((parent, file_name)) = asset
            .desktop_path()
            .and_then(|path| path.parent().zip(path.file_name()))
        {
            let slot = *folder_slots.entry(parent.to_path_buf()).or_insert_with(|| {
                deferred_metadata.add_folder(FolderDisplayMetadataCache::load(parent))
            });
            let cached =
                stamp.and_then(|stamp| deferred_metadata.cache_mut(slot).get(file_name, stamp));
            match cached {
                Some(metadata) => {
                    apply_display_metadata(&mut asset.metadata, metadata);
                }
                None => {
                    let file_name = file_name.to_owned();
                    deferred_metadata.defer(asset.id.clone(), slot, file_name, stamp);
                }
            }
        }
        assets.push(asset);
    }
    // Drops the entries of removed photos now; the workers save what they read.
    deferred_metadata.save();

    Ok(Some(FolderScan {
        assets,
        warning_count,
        truncated,
        deferred_metadata,
    }))
}
