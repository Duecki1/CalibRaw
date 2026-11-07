//! What a library scan did not find in its folders' caches, filled in by the
//! thumbnail workers: display metadata, and in the media library the
//! summaries of sidecars that changed.
//!
//! A scan reads only the folder listings and the folder caches, so the
//! catalog appears at once. A worker reads what is missing from the same open
//! as the embedded preview where it can, and records it here. A scan that
//! includes subfolders keeps one cache per folder its photos are in.

use super::*;
use crate::sidecar::SidecarSummary;
use calibraw_core::display_metadata_cache::{FileStamp, FolderDisplayMetadataCache};
use calibraw_core::pipeline::RawDisplayMetadata;
use std::sync::{MutexGuard, PoisonError};

/// Entries recorded since the folder caches were last written; bounds the work
/// lost when the app closes during indexing.
const SAVE_INTERVAL: usize = 32;

/// What the folder listing and its cache say about a photo's sidecar.
#[derive(Clone, Debug)]
pub(in crate::ui::library) enum PhotoSidecar {
    /// The platform lists no sidecars, so the photo's is read to find out.
    Unlisted,
    Absent,
    Summarized(SidecarSummary),
    /// Changed since it was summarised, or never was: read it, then record
    /// its summary with [`DeferredFolderMetadata::record_sidecar`].
    Unread(FileStamp),
}

/// One of the scan's folder caches, returned by
/// [`DeferredFolderMetadata::add_folder`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui::library) struct FolderSlot(usize);

/// Where a photo's cache entries are kept: its folder's cache, under its file
/// name.
#[derive(Clone, Debug)]
struct CacheKey {
    folder: FolderSlot,
    file_name: OsString,
}

pub(in crate::ui::library) struct DeferredFolderMetadata {
    state: Mutex<DeferredState>,
    /// The platform's sidecar write count before the folder was listed: a
    /// sidecar the app wrote after it is newer than its entry in `sidecars`.
    sidecar_writes_before_listing: u64,
}

struct DeferredState {
    caches: Vec<FolderDisplayMetadataCache>,
    /// Photos whose display metadata is unread, with the stamp that validates
    /// a cache entry; photos without a stamp are read but never cached.
    pending: HashMap<LibraryAssetId, (CacheKey, Option<FileStamp>)>,
    /// Media-library sidecars by their photo. Desktop reads sidecars directly
    /// and leaves this empty.
    sidecars: HashMap<LibraryAssetId, (CacheKey, PhotoSidecar)>,
    unread_sidecars: usize,
    unsaved: usize,
}

impl DeferredFolderMetadata {
    /// Nothing deferred yet, for a listing taken after the platform had made
    /// `sidecar_writes_before_listing` sidecar writes (0 where sidecars are
    /// not listed).
    pub(in crate::ui::library) fn new(sidecar_writes_before_listing: u64) -> Self {
        Self {
            state: Mutex::new(DeferredState {
                caches: Vec::new(),
                pending: HashMap::new(),
                sidecars: HashMap::new(),
                unread_sidecars: 0,
                unsaved: 0,
            }),
            sidecar_writes_before_listing,
        }
    }

    /// Takes over the cache of one scanned folder.
    pub(in crate::ui::library) fn add_folder(
        &mut self,
        cache: FolderDisplayMetadataCache,
    ) -> FolderSlot {
        let caches = &mut self.state_mut().caches;
        caches.push(cache);
        FolderSlot(caches.len() - 1)
    }

    /// The cache of `folder`, for the scan's lookups before workers start.
    pub(in crate::ui::library) fn cache_mut(
        &mut self,
        folder: FolderSlot,
    ) -> &mut FolderDisplayMetadataCache {
        &mut self.state_mut().caches[folder.0]
    }

    /// Leaves the display metadata of `asset`, called `file_name` in
    /// `folder`, to the workers.
    pub(in crate::ui::library) fn defer(
        &mut self,
        asset: LibraryAssetId,
        folder: FolderSlot,
        file_name: OsString,
        stamp: Option<FileStamp>,
    ) {
        self.state_mut()
            .pending
            .insert(asset, (CacheKey { folder, file_name }, stamp));
    }

    /// Records what the listing and the cache of `folder` say about the
    /// sidecar of `asset`, called `file_name` there.
    pub(in crate::ui::library) fn set_sidecar(
        &mut self,
        asset: LibraryAssetId,
        folder: FolderSlot,
        file_name: OsString,
        sidecar: PhotoSidecar,
    ) {
        let state = self.state_mut();
        let unread = matches!(sidecar, PhotoSidecar::Unread(_));
        let previous = state
            .sidecars
            .insert(asset, (CacheKey { folder, file_name }, sidecar));
        let was_unread = matches!(previous, Some((_, PhotoSidecar::Unread(_))));
        state.unread_sidecars =
            state.unread_sidecars + usize::from(unread) - usize::from(was_unread);
    }

    pub(in crate::ui::library) fn sidecar_writes_before_listing(&self) -> u64 {
        self.sidecar_writes_before_listing
    }

    fn state_mut(&mut self) -> &mut DeferredState {
        // Every update leaves the state consistent, so a poisoned lock is usable.
        self.state.get_mut().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock(&self) -> MutexGuard<'_, DeferredState> {
        // Every update leaves the state consistent, so a poisoned lock is usable.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(in crate::ui::library) fn is_pending(&self, asset: &LibraryAssetId) -> bool {
        self.lock().pending.contains_key(asset)
    }

    /// Records metadata read for a pending photo and returns it for delivery
    /// with the thumbnail. A failed read is not cached and retried next scan.
    pub(in crate::ui::library) fn complete(
        &self,
        asset: &LibraryAssetId,
        metadata: Option<RawDisplayMetadata>,
    ) -> Option<RawDisplayMetadata> {
        let mut state = self.lock();
        let Some((key, stamp)) = state.pending.remove(asset) else {
            return metadata;
        };
        if let (Some(stamp), Some(metadata)) = (stamp, metadata) {
            state.caches[key.folder.0].insert(&key.file_name, stamp, metadata);
            state.unsaved += 1;
        }
        state.save_when_due();
        metadata
    }

    /// Reads a pending folder photo's metadata on its own, for thumbnails
    /// served from a cache. Returns `None` when the photo is not pending.
    pub(in crate::ui::library) fn read_pending(
        &self,
        asset: &LibraryAssetId,
        path: &Path,
    ) -> Option<RawDisplayMetadata> {
        // The read runs unlocked so other workers are not held up by its I/O.
        if !self.is_pending(asset) {
            return None;
        }
        let metadata =
            calibraw_core::serialized_reads::run(|| load_raw_display_metadata(path)).ok();
        self.complete(asset, metadata)
    }

    pub(in crate::ui::library) fn sidecar(&self, asset: &LibraryAssetId) -> PhotoSidecar {
        self.lock()
            .sidecars
            .get(asset)
            .map(|(_, sidecar)| sidecar.clone())
            .unwrap_or(PhotoSidecar::Unlisted)
    }

    /// Records the summary of an unread sidecar, so it is not read again
    /// while its stamp stays the same.
    pub(in crate::ui::library) fn record_sidecar(
        &self,
        asset: &LibraryAssetId,
        summary: SidecarSummary,
    ) {
        let mut state = self.lock();
        let Some((key, PhotoSidecar::Unread(stamp))) = state.sidecars.get(asset).cloned() else {
            return;
        };
        state.caches[key.folder.0].insert_sidecar(&key.file_name, stamp, summary.clone());
        state
            .sidecars
            .insert(asset.clone(), (key, PhotoSidecar::Summarized(summary)));
        state.unread_sidecars -= 1;
        state.unsaved += 1;
        state.save_when_due();
    }

    /// Writes every folder cache that changed.
    pub(in crate::ui::library) fn save(&self) {
        self.lock().save();
    }
}

impl DeferredState {
    fn save_when_due(&mut self) {
        let complete = self.pending.is_empty() && self.unread_sidecars == 0;
        if complete || self.unsaved >= SAVE_INTERVAL {
            self.save();
        }
    }

    fn save(&mut self) {
        // A cache that did not change since its last save writes nothing.
        for cache in &mut self.caches {
            if let Err(error) = cache.save() {
                log::warn!("{error}");
            }
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

/// The part of a photo's display metadata the media library shows: its
/// dimensions. Its photos keep their file date and show no capture settings
/// (see [`LibraryAssetMetadata`]); the folder cache keeps all of it.
pub(in crate::ui::library) fn media_library_display_metadata(
    metadata: RawDisplayMetadata,
) -> RawDisplayMetadata {
    RawDisplayMetadata {
        dimensions: metadata.dimensions,
        ..RawDisplayMetadata::default()
    }
}
