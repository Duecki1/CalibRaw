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

pub(in crate::ui::library) fn scan_folder(
    folder: &Path,
    is_cancelled: impl Fn() -> bool,
) -> Result<Option<FolderScan>, String> {
    scan_folder_with_limit(folder, MAX_LIBRARY_FILES, is_cancelled)
}

pub(in crate::ui::library) fn scan_folder_with_limit(
    folder: &Path,
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
    let entries = std::fs::read_dir(folder).map_err(|error| {
        tr_fmt(
            "Could not scan {folder}: {error}",
            &[("folder", &folder.display()), ("error", &error)],
        )
    })?;
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
    let mut ranked_assets = ranked_assets.into_vec();
    ranked_assets.sort();

    let mut metadata_cache = FolderDisplayMetadataCache::load(folder);
    let mut pending_metadata = HashMap::new();
    let mut assets = Vec::with_capacity(ranked_assets.len());
    for RankedLibraryAsset {
        mut asset, stamp, ..
    } in ranked_assets
    {
        if is_cancelled() {
            return Ok(None);
        }
        if let Some(path) = asset.desktop_path() {
            let cached = path
                .file_name()
                .zip(stamp)
                .and_then(|(file_name, stamp)| metadata_cache.get(file_name, stamp));
            match cached {
                Some(metadata) => {
                    apply_display_metadata(&mut asset.metadata, metadata);
                }
                None => {
                    if let Some(file_name) = path.file_name() {
                        pending_metadata.insert(file_name.to_owned(), stamp);
                    }
                }
            }
        }
        assets.push(asset);
    }
    // Drops the entries of removed photos now; the workers save what they read.
    if let Err(error) = metadata_cache.save() {
        log::warn!("{error}");
    }

    Ok(Some(FolderScan {
        assets,
        warning_count,
        truncated,
        deferred_metadata: DeferredFolderMetadata::new(metadata_cache, pending_metadata),
    }))
}
