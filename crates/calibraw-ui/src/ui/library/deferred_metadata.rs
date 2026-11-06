//! What a library scan did not find in its folder's cache, filled in by the
//! thumbnail workers: display metadata, and in the media library the
//! summaries of sidecars that changed.
//!
//! A scan reads only the folder listing and the folder cache, so the catalog
//! appears at once. A worker reads what is missing from the same open as the
//! embedded preview where it can, and records it here.

use super::*;
use crate::sidecar::SidecarSummary;
use calibraw_core::display_metadata_cache::{FileStamp, FolderDisplayMetadataCache};
use calibraw_core::pipeline::RawDisplayMetadata;
use std::ffi::OsStr;
use std::sync::{MutexGuard, PoisonError};

/// Entries recorded since the folder cache was last written; bounds the work
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

pub(in crate::ui::library) struct DeferredFolderMetadata {
    state: Mutex<DeferredState>,
    /// The platform's sidecar write count before the folder was listed: a
    /// sidecar the app wrote after it is newer than its entry in `sidecars`.
    sidecar_writes_before_listing: u64,
}

struct DeferredState {
    cache: FolderDisplayMetadataCache,
    /// Photos whose display metadata is unread, by file name, with the stamp
    /// that validates a cache entry; photos without a stamp are read but never
    /// cached.
    pending: HashMap<OsString, Option<FileStamp>>,
    /// Media-library sidecars by their photo's file name. Desktop reads
    /// sidecars directly and leaves this empty.
    sidecars: HashMap<OsString, PhotoSidecar>,
    unread_sidecars: usize,
    unsaved: usize,
}

impl DeferredFolderMetadata {
    pub(in crate::ui::library) fn new(
        cache: FolderDisplayMetadataCache,
        pending: HashMap<OsString, Option<FileStamp>>,
    ) -> Self {
        Self::with_sidecars(cache, pending, HashMap::new(), 0)
    }

    pub(in crate::ui::library) fn with_sidecars(
        cache: FolderDisplayMetadataCache,
        pending: HashMap<OsString, Option<FileStamp>>,
        sidecars: HashMap<OsString, PhotoSidecar>,
        sidecar_writes_before_listing: u64,
    ) -> Self {
        let unread_sidecars = sidecars
            .values()
            .filter(|sidecar| matches!(sidecar, PhotoSidecar::Unread(_)))
            .count();
        Self {
            state: Mutex::new(DeferredState {
                cache,
                pending,
                sidecars,
                unread_sidecars,
                unsaved: 0,
            }),
            sidecar_writes_before_listing,
        }
    }

    pub(in crate::ui::library) fn sidecar_writes_before_listing(&self) -> u64 {
        self.sidecar_writes_before_listing
    }

    fn lock(&self) -> MutexGuard<'_, DeferredState> {
        // Every update leaves the state consistent, so a poisoned lock is usable.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(in crate::ui::library) fn is_pending(&self, file_name: &OsStr) -> bool {
        self.lock().pending.contains_key(file_name)
    }

    /// Records metadata read for a pending photo and returns it for delivery
    /// with the thumbnail. A failed read is not cached and retried next scan.
    pub(in crate::ui::library) fn complete(
        &self,
        file_name: &OsStr,
        metadata: Option<RawDisplayMetadata>,
    ) -> Option<RawDisplayMetadata> {
        let mut state = self.lock();
        let Some(stamp) = state.pending.remove(file_name) else {
            return metadata;
        };
        if let (Some(stamp), Some(metadata)) = (stamp, metadata) {
            state.cache.insert(file_name, stamp, metadata);
            state.unsaved += 1;
        }
        state.save_when_due();
        metadata
    }

    /// Reads a pending folder photo's metadata on its own, for thumbnails
    /// served from a cache. Returns `None` when the photo is not pending.
    pub(in crate::ui::library) fn read_pending(&self, path: &Path) -> Option<RawDisplayMetadata> {
        let file_name = path.file_name().unwrap_or_default();
        // The read runs unlocked so other workers are not held up by its I/O.
        if !self.is_pending(file_name) {
            return None;
        }
        let metadata =
            calibraw_core::serialized_reads::run(|| load_raw_display_metadata(path)).ok();
        self.complete(file_name, metadata)
    }

    pub(in crate::ui::library) fn sidecar(&self, file_name: &OsStr) -> PhotoSidecar {
        self.lock()
            .sidecars
            .get(file_name)
            .cloned()
            .unwrap_or(PhotoSidecar::Unlisted)
    }

    /// Records the summary of an unread sidecar, so it is not read again
    /// while its stamp stays the same.
    pub(in crate::ui::library) fn record_sidecar(
        &self,
        file_name: &OsStr,
        summary: SidecarSummary,
    ) {
        let mut state = self.lock();
        let Some(PhotoSidecar::Unread(stamp)) = state.sidecars.get(file_name).cloned() else {
            return;
        };
        state
            .cache
            .insert_sidecar(file_name, stamp, summary.clone());
        state
            .sidecars
            .insert(file_name.to_owned(), PhotoSidecar::Summarized(summary));
        state.unread_sidecars -= 1;
        state.unsaved += 1;
        state.save_when_due();
    }

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
