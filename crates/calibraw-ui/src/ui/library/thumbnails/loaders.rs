//! Loading thumbnails and reference previews from desktop files and media-library photos.

use super::*;
use crate::i18n::{tr, tr_fmt};
use calibraw_core::pipeline::RawDisplayMetadata;
use std::ffi::OsStr;

pub(crate) fn load_desktop_reference_preview(
    path: &Path,
    maximum_edge: u32,
) -> Result<RawThumbnail, String> {
    if maximum_edge == 0 {
        return Err(tr("reference preview edge must be non-zero").to_owned());
    }

    match render_uncached_developed_thumbnail(path, maximum_edge) {
        Ok(Some(thumbnail)) => return Ok(thumbnail),
        Ok(None) => {}
        Err(error) => {
            log::warn!(
                "could not render developed reference preview for {}: {error}",
                path.display()
            );
        }
    }

    load_raw_thumbnail(path, maximum_edge).map_err(|error| {
        tr_fmt(
            "could not render reference preview: {error}",
            &[("error", &format!("{:#}", error))],
        )
    })
}

pub(crate) fn load_desktop_cached_thumbnail(
    path: &Path,
    maximum_edge: u32,
) -> Result<Option<RawThumbnail>, String> {
    match crate::sidecar::load_developed_thumbnail_cache(path, maximum_edge) {
        Ok(Some(thumbnail)) => return Ok(Some(thumbnail)),
        Ok(None) => {}
        Err(error) => log::warn!(
            "could not use developed loading thumbnail for {}: {error}",
            path.display()
        ),
    }
    calibraw_core::thumbnail_cache::load_desktop_raw_thumbnail(path, maximum_edge)
}

pub(in crate::ui::library) fn load_desktop_library_thumbnail(
    asset: &LibraryAsset,
    stage: ThumbnailLoadStage,
    render_edited_thumbnails_during_indexing: bool,
    deferred_metadata: &DeferredFolderMetadata,
) -> Result<LoadedLibraryThumbnail, String> {
    let Some(path) = asset.desktop_path() else {
        return Err(tr("invalid desktop thumbnail request").to_owned());
    };
    match stage {
        ThumbnailLoadStage::RawPreview => {
            match crate::sidecar::load_developed_thumbnail_cache(path, THUMBNAIL_EDGE) {
                Ok(Some(thumbnail)) => {
                    let mut loaded = loaded_library_thumbnail(thumbnail, true);
                    loaded.display_metadata = deferred_metadata.read_pending(path);
                    return Ok(loaded);
                }
                Ok(None) => {}
                Err(error) => log::warn!(
                    "could not use developed thumbnail cache for {}: {error}",
                    path.display()
                ),
            }

            let has_edits = crate::sidecar::sidecar_path_for_raw(path).is_file();
            load_desktop_raw_library_thumbnail(
                path,
                has_edits,
                render_edited_thumbnails_during_indexing,
                deferred_metadata,
            )
        }
        ThumbnailLoadStage::DevelopedPreview => {
            match render_uncached_developed_thumbnail(path, THUMBNAIL_EDGE) {
                Ok(Some(thumbnail)) => Ok(loaded_library_thumbnail(thumbnail, true)),
                Ok(None) => {
                    load_desktop_raw_library_thumbnail(path, false, false, deferred_metadata)
                }
                Err(error) => Err(tr_fmt(
                    "could not render the edited RAW thumbnail for {path}: {error}",
                    &[("path", &path.display()), ("error", &error)],
                )),
            }
        }
    }
}

fn load_desktop_raw_library_thumbnail(
    path: &Path,
    has_edits: bool,
    render_edited_thumbnails_during_indexing: bool,
    deferred_metadata: &DeferredFolderMetadata,
) -> Result<LoadedLibraryThumbnail, String> {
    let (geometry, has_edits) =
        crate::sidecar::load_photo_preview_info(path).unwrap_or_else(|error| {
            log::warn!(
                "Could not read preview edits for {}: {error}",
                path.display()
            );
            (crate::pipeline::GeometryTransform::default(), has_edits)
        });
    let (thumbnail, display_metadata) =
        match calibraw_core::thumbnail_cache::load_desktop_raw_thumbnail(path, THUMBNAIL_EDGE) {
            Ok(Some(thumbnail)) => (thumbnail, deferred_metadata.read_pending(path)),
            cached => {
                if let Err(error) = cached {
                    log::warn!(
                        "could not use RAW thumbnail cache for {}: {error}",
                        path.display()
                    );
                }
                let file_name = path.file_name().unwrap_or_default();
                let (thumbnail, display_metadata) = if deferred_metadata.is_pending(file_name) {
                    let (thumbnail, metadata) =
                        load_raw_thumbnail_with_display_metadata(path, THUMBNAIL_EDGE).map_err(
                            |error| {
                                tr_fmt(
                                    "could not render a RAW preview: {error}",
                                    &[("error", &format!("{:#}", error))],
                                )
                            },
                        )?;
                    (thumbnail, deferred_metadata.complete(file_name, metadata))
                } else {
                    let thumbnail = load_raw_thumbnail(path, THUMBNAIL_EDGE).map_err(|error| {
                        tr_fmt(
                            "could not render a RAW preview: {error}",
                            &[("error", &format!("{:#}", error))],
                        )
                    })?;
                    (thumbnail, None)
                };
                if let Err(error) =
                    calibraw_core::thumbnail_cache::save_desktop_raw_thumbnail(path, &thumbnail)
                {
                    log::warn!(
                        "could not persist RAW thumbnail cache for {}: {error}",
                        path.display()
                    );
                }
                (thumbnail, display_metadata)
            }
        };
    let thumbnail = crate::pipeline::transform_thumbnail_geometry(&thumbnail, geometry);
    let mut loaded = if has_edits && render_edited_thumbnails_during_indexing {
        loaded_library_raw_preview_pending_development(thumbnail)
    } else if has_edits {
        loaded_library_raw_preview_with_stale_edits(thumbnail)
    } else {
        loaded_library_thumbnail(thumbnail, false)
    };
    loaded.display_metadata = display_metadata;
    Ok(loaded)
}

/// A media-library photo's thumbnail: the developed one while it matches the
/// sidecar, otherwise the camera's preview with the sidecar's crop and
/// rotation. Cached previews come straight from the platform's cache; reading
/// a photo waits for one of `extraction`.
pub(in crate::ui::library) fn load_media_library_thumbnail(
    platform: &dyn Platform,
    folder: &DeferredFolderMetadata,
    extraction: &ExtractionPermits,
    asset: &LibraryAsset,
) -> Result<LoadedLibraryThumbnail, String> {
    let Some(uri) = asset.android_uri() else {
        return Err(tr("invalid Android thumbnail request").to_owned());
    };
    let photo = MediaPhoto {
        platform,
        folder,
        extraction,
        asset,
        uri,
        file_name: OsStr::new(&asset.display_name),
    };
    let written_since_listing =
        platform.sidecar_written_since(uri, folder.sidecar_writes_before_listing());
    let sidecar = match folder.sidecar(photo.file_name) {
        PhotoSidecar::Unlisted => return photo.load_unlisted(),
        // A rating, paste or edit since the listing: what it says is out of date.
        _ if written_since_listing => photo.read_sidecar_summary(false),
        PhotoSidecar::Absent => None,
        PhotoSidecar::Summarized(summary) => Some(summary),
        PhotoSidecar::Unread(_) => photo.read_sidecar_summary(true),
    };
    if let Some(summary) = &sidecar {
        match platform.load_current_developed_thumbnail(
            uri,
            summary.render_fingerprint,
            THUMBNAIL_EDGE,
        ) {
            Ok(Some(thumbnail)) => return Ok(photo.developed(thumbnail, Some(summary.review))),
            Ok(None) => {}
            Err(error) => log::warn!(
                "could not use Android developed-thumbnail cache for {}: {error}",
                asset.display_name
            ),
        }
    }
    photo.camera_preview(
        sidecar.as_ref().map(|summary| summary.geometry),
        sidecar.as_ref().is_some_and(|summary| summary.has_edits),
        sidecar.map(|summary| summary.review),
    )
}

/// One media-library photo while its thumbnail loads.
struct MediaPhoto<'a> {
    platform: &'a dyn Platform,
    folder: &'a DeferredFolderMetadata,
    extraction: &'a ExtractionPermits,
    asset: &'a LibraryAsset,
    uri: &'a str,
    file_name: &'a OsStr,
}

impl MediaPhoto<'_> {
    /// For platforms that list no sidecars: the sidecar is read every time.
    fn load_unlisted(&self) -> Result<LoadedLibraryThumbnail, String> {
        let display_name = self.asset.display_name.as_str();
        match self
            .platform
            .load_developed_thumbnail_cache(self.uri, display_name, THUMBNAIL_EDGE)
        {
            Ok(Some(thumbnail)) => {
                let review = self
                    .platform
                    .load_sidecar_review(self.uri, display_name)
                    .unwrap_or_else(|error| {
                        log::warn!(
                            "could not inspect Android review sidecar for {display_name}: {error}"
                        );
                        None
                    });
                return Ok(self.developed(thumbnail, review));
            }
            Ok(None) => {}
            Err(error) => log::warn!(
                "could not use Android developed-thumbnail cache for {display_name}: {error}"
            ),
        }
        let sidecar = self
            .platform
            .load_sidecar(self.uri, display_name)
            .unwrap_or_else(|error| {
                log::warn!("could not inspect Android edit sidecar for {display_name}: {error}");
                None
            });
        self.camera_preview(
            sidecar.as_ref().map(|sidecar| sidecar.edits.geometry),
            sidecar
                .as_ref()
                .is_some_and(|sidecar| crate::sidecar::edit_state_has_adjustments(&sidecar.edits)),
            sidecar.map(|sidecar| sidecar.review),
        )
    }

    /// Reads the photo's sidecar. With `record`, its summary is kept under the
    /// stamp the listing gave, which it must still have: not after the app
    /// wrote it since.
    fn read_sidecar_summary(&self, record: bool) -> Option<crate::sidecar::SidecarSummary> {
        match self
            .platform
            .load_sidecar_summary(self.uri, &self.asset.display_name)
        {
            Ok(Some(summary)) => {
                if record {
                    self.folder.record_sidecar(self.file_name, summary.clone());
                }
                Some(summary)
            }
            // Removed since the folder was listed.
            Ok(None) => None,
            Err(error) => {
                log::warn!(
                    "could not inspect Android edit sidecar for {}: {error}",
                    self.asset.display_name
                );
                None
            }
        }
    }

    fn developed(
        &self,
        thumbnail: RawThumbnail,
        review: Option<crate::sidecar::PhotoReview>,
    ) -> LoadedLibraryThumbnail {
        let mut loaded = loaded_library_thumbnail(thumbnail, true);
        loaded.review = review;
        loaded.display_metadata = self.pending_display_metadata();
        loaded
    }

    /// The camera's embedded preview, from the cache or read out of the photo.
    fn camera_preview(
        &self,
        geometry: Option<crate::pipeline::GeometryTransform>,
        has_edits: bool,
        review: Option<crate::sidecar::PhotoReview>,
    ) -> Result<LoadedLibraryThumbnail, String> {
        let (thumbnail, display_metadata) = self.embedded_preview()?;
        let thumbnail = match geometry {
            Some(geometry) => crate::pipeline::transform_thumbnail_geometry(&thumbnail, geometry),
            None => thumbnail,
        };
        let mut loaded = if has_edits {
            loaded_library_raw_preview_with_stale_edits(thumbnail)
        } else {
            loaded_library_thumbnail(thumbnail, false)
        };
        loaded.review = review;
        loaded.display_metadata = display_metadata;
        Ok(loaded)
    }

    fn embedded_preview(&self) -> Result<(RawThumbnail, Option<RawDisplayMetadata>), String> {
        let metadata = &self.asset.metadata;
        match self.platform.load_cached_library_preview(
            self.uri,
            metadata.bytes,
            metadata.modified_seconds,
            THUMBNAIL_EDGE,
        ) {
            Ok(Some(thumbnail)) => return Ok((thumbnail, self.pending_display_metadata())),
            Ok(None) => {}
            Err(error) => log::warn!(
                "discarding the cached preview of {}: {error}",
                self.asset.display_name
            ),
        }
        let pending = self.folder.is_pending(self.file_name);
        let (thumbnail, read) = {
            let _permit = self.extraction.acquire();
            self.platform.extract_library_preview(
                self.uri,
                &self.asset.display_name,
                metadata.bytes,
                metadata.modified_seconds,
                THUMBNAIL_EDGE,
                pending,
            )?
        };
        let display_metadata = if pending {
            self.folder.complete(self.file_name, read)
        } else {
            read
        };
        Ok((
            thumbnail,
            display_metadata.map(media_library_display_metadata),
        ))
    }

    /// Reads the display metadata the folder cache lacks, on its own; `None`
    /// when the cache has it.
    fn pending_display_metadata(&self) -> Option<RawDisplayMetadata> {
        if !self.folder.is_pending(self.file_name) {
            return None;
        }
        let read = {
            let _permit = self.extraction.acquire();
            self.platform
                .load_library_display_metadata(self.uri, &self.asset.display_name)
        };
        let read = read.map_err(|error| log::warn!("{error}")).ok();
        self.folder
            .complete(self.file_name, read)
            .map(media_library_display_metadata)
    }
}
