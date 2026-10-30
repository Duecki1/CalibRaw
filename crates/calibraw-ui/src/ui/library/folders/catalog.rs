use super::super::*;
use crate::i18n::tr_fmt;
use calibraw_core::display_metadata_cache::{FileStamp, FolderDisplayMetadataCache};
use calibraw_core::pipeline::RawDisplayMetadata;
use std::collections::BinaryHeap;
use std::sync::{MutexGuard, PoisonError};
use std::time::UNIX_EPOCH;

/// Photos whose metadata was read since the folder cache was last written;
/// bounds the work lost when the app closes during indexing.
const DEFERRED_METADATA_SAVE_INTERVAL: usize = 32;

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
    pub(in crate::ui::library) deferred_metadata: DeferredDisplayMetadata,
}

/// Display metadata the folder scan did not find in the folder cache.
///
/// The scan does not open photos, so the catalog appears after one directory
/// listing. Thumbnail workers read the metadata instead, from the same open as
/// the embedded preview when the preview is not cached, and record it here.
pub(in crate::ui::library) struct DeferredDisplayMetadata {
    state: Mutex<DeferredMetadataState>,
}

struct DeferredMetadataState {
    cache: FolderDisplayMetadataCache,
    /// Photos whose metadata is unread, with the stamp that validates a cache
    /// entry; photos without a stamp are read but never cached.
    pending: HashMap<PathBuf, Option<FileStamp>>,
    unsaved: usize,
}

impl DeferredDisplayMetadata {
    pub(in crate::ui::library) fn new(
        cache: FolderDisplayMetadataCache,
        pending: HashMap<PathBuf, Option<FileStamp>>,
    ) -> Self {
        Self {
            state: Mutex::new(DeferredMetadataState {
                cache,
                pending,
                unsaved: 0,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, DeferredMetadataState> {
        // Every update leaves the state consistent, so a poisoned lock is usable.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(in crate::ui::library) fn is_pending(&self, path: &Path) -> bool {
        self.lock().pending.contains_key(path)
    }

    /// Records metadata read for a pending photo and returns it for delivery
    /// with the thumbnail. A failed read is not cached and retried next scan.
    pub(in crate::ui::library) fn complete(
        &self,
        path: &Path,
        metadata: Option<RawDisplayMetadata>,
    ) -> Option<RawDisplayMetadata> {
        let mut state = self.lock();
        let Some(stamp) = state.pending.remove(path) else {
            return metadata;
        };
        if let (Some(file_name), Some(stamp), Some(metadata)) = (path.file_name(), stamp, metadata)
        {
            state.cache.insert(file_name, stamp, metadata);
            state.unsaved += 1;
        }
        if state.pending.is_empty() || state.unsaved >= DEFERRED_METADATA_SAVE_INTERVAL {
            state.save();
        }
        metadata
    }

    /// Reads a pending photo's metadata on its own, for thumbnails served from
    /// a cache. Returns `None` when the photo is not pending.
    pub(in crate::ui::library) fn read_pending(&self, path: &Path) -> Option<RawDisplayMetadata> {
        // The read runs unlocked so other workers are not held up by its I/O.
        if !self.is_pending(path) {
            return None;
        }
        let metadata =
            calibraw_core::serialized_reads::run(|| load_raw_display_metadata(path)).ok();
        self.complete(path, metadata)
    }

    pub(in crate::ui::library) fn save(&self) {
        self.lock().save();
    }
}

impl DeferredMetadataState {
    fn save(&mut self) {
        if let Err(error) = self.cache.save() {
            log::warn!("{error}");
        }
        self.unsaved = 0;
    }
}

/// Folds display metadata into the asset; says whether that moves the asset in
/// the newest/oldest order, which needs the library list to be sorted again.
pub(in crate::ui::library) fn apply_display_metadata(
    asset: &mut LibraryAssetMetadata,
    metadata: RawDisplayMetadata,
) -> bool {
    let order_before = asset.date_order_seconds();
    asset.dimensions_hint = Some(metadata.dimensions);
    asset.iso_speed = metadata.iso_speed;
    asset.shutter_seconds = metadata.shutter_seconds;
    asset.focal_length = metadata.focal_length;
    asset.aperture = metadata.aperture;
    asset.taken_seconds = metadata.taken_seconds;
    order_before != asset.date_order_seconds()
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
                    pending_metadata.insert(path.to_path_buf(), stamp);
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
        deferred_metadata: DeferredDisplayMetadata::new(metadata_cache, pending_metadata),
    }))
}
