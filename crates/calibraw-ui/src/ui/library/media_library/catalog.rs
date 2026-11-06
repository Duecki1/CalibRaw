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

/// The catalog of `documents`, the listing of `folder` within the linked
/// folder `linked_folder` (the library for an empty key), taken after the
/// platform had made `sidecar_writes` sidecar writes. What the folder cache
/// lacks is left to the thumbnail workers.
pub(in crate::ui::library) fn catalog_media_folder(
    platform: &dyn Platform,
    linked_folder: &str,
    folder: &str,
    documents: Vec<LibraryDocument>,
    sidecar_writes: u64,
) -> MediaFolderScan {
    let mut cache = platform
        .library_folder_cache_path(linked_folder, folder)
        .map_or_else(
            FolderDisplayMetadataCache::in_memory,
            FolderDisplayMetadataCache::load_file,
        );
    let truncated = documents.len() > MAX_LIBRARY_FILES;
    let mut pending = HashMap::new();
    let mut sidecars = HashMap::new();
    let mut assets = Vec::with_capacity(documents.len().min(MAX_LIBRARY_FILES));
    for document in documents.into_iter().take(MAX_LIBRARY_FILES) {
        let listed_sidecar = document.sidecar;
        let mut asset = LibraryAsset::from_android_document(document);
        let file_name = OsString::from(&asset.display_name);
        let stamp = photo_stamp(&asset.metadata);
        match cache.get(&file_name, stamp) {
            Some(metadata) => {
                apply_display_metadata(
                    &mut asset.metadata,
                    media_library_display_metadata(metadata),
                );
            }
            None => {
                pending.insert(file_name.clone(), Some(stamp));
            }
        }
        let sidecar = match listed_sidecar {
            ListedSidecar::Unlisted => PhotoSidecar::Unlisted,
            ListedSidecar::Absent => PhotoSidecar::Absent,
            ListedSidecar::Present(stamp) => match cache.sidecar(&file_name, stamp) {
                Some(summary) => {
                    asset.metadata.review = summary.review;
                    PhotoSidecar::Summarized(summary)
                }
                None => PhotoSidecar::Unread(stamp),
            },
        };
        sidecars.insert(file_name, sidecar);
        assets.push(asset);
    }
    // Drops the entries of removed photos now; the workers save what they read.
    if let Err(error) = cache.save() {
        log::warn!("{error}");
    }
    MediaFolderScan {
        assets,
        truncated,
        deferred_metadata: DeferredFolderMetadata::with_sidecars(
            cache,
            pending,
            sidecars,
            sidecar_writes,
        ),
    }
}

/// The photo's version as the listing gives it, to the second.
fn photo_stamp(metadata: &LibraryAssetMetadata) -> FileStamp {
    FileStamp::new(
        metadata.bytes,
        metadata.modified_seconds.saturating_mul(1_000_000_000),
    )
}
