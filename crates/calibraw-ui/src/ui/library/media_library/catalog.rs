//! A listed media-library folder made into the catalog, with what its folder
//! cache knows, so the grid shows its final tile shapes and ratings before a
//! single preview loads.

use super::super::*;
use crate::platform::{LibraryDocument, ListedSidecar};
use calibraw_core::display_metadata_cache::{FileStamp, FolderDisplayMetadataCache};

pub(in crate::ui::library) struct MediaFolderScan {
    pub(in crate::ui::library) assets: Vec<LibraryAsset>,
    pub(in crate::ui::library) truncated: bool,
    pub(in crate::ui::library) deferred_metadata: DeferredFolderMetadata,
}

/// The catalog of `documents`, the listing of `folder` (and, when it
/// included them, of its subfolders) within the linked folder
/// `linked_folder` (the library for an empty key), taken after the platform
/// had made `sidecar_writes` sidecar writes. What the folder caches lack is
/// left to the thumbnail workers.
pub(in crate::ui::library) fn catalog_media_folder(
    platform: &dyn Platform,
    linked_folder: &str,
    folder: &str,
    documents: Vec<LibraryDocument>,
    sidecar_writes: u64,
) -> MediaFolderScan {
    let load_cache = |folder: &str| {
        platform
            .library_folder_cache_path(linked_folder, folder)
            .map_or_else(
                FolderDisplayMetadataCache::in_memory,
                FolderDisplayMetadataCache::load_file,
            )
    };
    let truncated = documents.len() > MAX_LIBRARY_FILES;
    let mut deferred = DeferredFolderMetadata::new(sidecar_writes);
    // The listed folder's cache is always loaded, so it drops the entries of
    // its removed photos even when none are left.
    let listed_slot = deferred.add_folder(load_cache(folder));
    let mut subfolder_slots = HashMap::new();
    let mut assets = Vec::with_capacity(documents.len().min(MAX_LIBRARY_FILES));
    for document in documents.into_iter().take(MAX_LIBRARY_FILES) {
        let slot = if document.subfolder.is_empty() {
            listed_slot
        } else {
            *subfolder_slots
                .entry(document.subfolder.clone())
                .or_insert_with(|| {
                    deferred.add_folder(load_cache(&android_child_folder(
                        folder,
                        &document.subfolder,
                    )))
                })
        };
        let listed_sidecar = document.sidecar;
        let mut asset = LibraryAsset::from_android_document(document);
        let file_name = OsString::from(&asset.display_name);
        let stamp = photo_stamp(&asset.metadata);
        let cache = deferred.cache_mut(slot);
        match cache.get(&file_name, stamp) {
            Some(metadata) => {
                apply_display_metadata(
                    &mut asset.metadata,
                    media_library_display_metadata(metadata),
                );
            }
            None => {
                deferred.defer(asset.id.clone(), slot, file_name.clone(), Some(stamp));
            }
        }
        let sidecar = match listed_sidecar {
            ListedSidecar::Unlisted => PhotoSidecar::Unlisted,
            ListedSidecar::Absent => PhotoSidecar::Absent,
            ListedSidecar::Present(stamp) => {
                match deferred.cache_mut(slot).sidecar(&file_name, stamp) {
                    Some(summary) => {
                        asset.metadata.review = summary.review;
                        asset.metadata.edited = summary.edited;
                        PhotoSidecar::Summarized(summary)
                    }
                    None => PhotoSidecar::Unread(stamp),
                }
            }
        };
        deferred.set_sidecar(asset.id.clone(), slot, file_name, sidecar);
        assets.push(asset);
    }
    // Drops the entries of removed photos now; the workers save what they read.
    deferred.save();
    MediaFolderScan {
        assets,
        truncated,
        deferred_metadata: deferred,
    }
}

/// The library path of `subfolder`, a path below `folder`.
fn android_child_folder(folder: &str, subfolder: &str) -> String {
    if folder.is_empty() {
        subfolder.to_owned()
    } else {
        format!("{folder}/{subfolder}")
    }
}

/// The photo's version as the listing gives it, to the second.
fn photo_stamp(metadata: &LibraryAssetMetadata) -> FileStamp {
    FileStamp::new(
        metadata.bytes,
        metadata.modified_seconds.saturating_mul(1_000_000_000),
    )
}
