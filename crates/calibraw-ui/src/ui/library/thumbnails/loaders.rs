//! Loading thumbnails and reference previews from desktop files and Android documents.

use super::*;

#[cfg(not(target_os = "android"))]
pub(crate) fn load_desktop_reference_preview(
    path: &Path,
    maximum_edge: u32,
) -> Result<RawThumbnail, String> {
    if maximum_edge == 0 {
        return Err("reference preview edge must be non-zero".to_owned());
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

    load_raw_thumbnail(path, maximum_edge)
        .map_err(|error| format!("could not render reference preview: {error:#}"))
}

#[cfg(not(target_os = "android"))]
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

#[cfg(not(target_os = "android"))]
pub(in crate::ui::library) fn load_desktop_library_thumbnail(
    asset: &LibraryAsset,
    stage: ThumbnailLoadStage,
    render_edited_thumbnails_during_indexing: bool,
) -> Result<LoadedLibraryThumbnail, String> {
    let Some(path) = asset.desktop_path() else {
        return Err("invalid desktop thumbnail request".to_owned());
    };
    match stage {
        ThumbnailLoadStage::RawPreview => {
            match crate::sidecar::load_developed_thumbnail_cache(path, THUMBNAIL_EDGE) {
                Ok(Some(thumbnail)) => return Ok(loaded_library_thumbnail(thumbnail, true)),
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
            )
        }
        ThumbnailLoadStage::DevelopedPreview => {
            match render_uncached_developed_thumbnail(path, THUMBNAIL_EDGE) {
                Ok(Some(thumbnail)) => Ok(loaded_library_thumbnail(thumbnail, true)),
                Ok(None) => load_desktop_raw_library_thumbnail(path, false, false),
                Err(error) => Err(format!(
                    "could not render the edited RAW thumbnail for {}: {error}",
                    path.display()
                )),
            }
        }
    }
}

#[cfg(not(target_os = "android"))]
fn load_desktop_raw_library_thumbnail(
    path: &Path,
    has_edits: bool,
    render_edited_thumbnails_during_indexing: bool,
) -> Result<LoadedLibraryThumbnail, String> {
    let (geometry, has_edits) =
        crate::sidecar::load_photo_preview_info(path).unwrap_or_else(|error| {
            log::warn!(
                "Could not read preview edits for {}: {error}",
                path.display()
            );
            (crate::pipeline::GeometryTransform::default(), has_edits)
        });
    match calibraw_core::thumbnail_cache::load_desktop_raw_thumbnail(path, THUMBNAIL_EDGE) {
        Ok(Some(thumbnail)) => {
            let thumbnail = crate::pipeline::transform_thumbnail_geometry(&thumbnail, geometry);
            return Ok(if has_edits && render_edited_thumbnails_during_indexing {
                loaded_library_raw_preview_pending_development(thumbnail)
            } else if has_edits {
                loaded_library_raw_preview_with_stale_edits(thumbnail)
            } else {
                loaded_library_thumbnail(thumbnail, false)
            });
        }
        Ok(None) => {}
        Err(error) => log::warn!(
            "could not use RAW thumbnail cache for {}: {error}",
            path.display()
        ),
    }

    let thumbnail = load_raw_thumbnail(path, THUMBNAIL_EDGE)
        .map_err(|error| format!("could not render a RAW preview: {error:#}"))?;
    if let Err(error) = calibraw_core::thumbnail_cache::save_desktop_raw_thumbnail(path, &thumbnail)
    {
        log::warn!(
            "could not persist RAW thumbnail cache for {}: {error}",
            path.display()
        );
    }
    let thumbnail = crate::pipeline::transform_thumbnail_geometry(&thumbnail, geometry);
    Ok(if has_edits && render_edited_thumbnails_during_indexing {
        loaded_library_raw_preview_pending_development(thumbnail)
    } else if has_edits {
        loaded_library_raw_preview_with_stale_edits(thumbnail)
    } else {
        loaded_library_thumbnail(thumbnail, false)
    })
}

#[cfg(target_os = "android")]
pub(in crate::ui::library) fn load_android_library_thumbnail(
    app: &calibraw_ffi::AndroidApp,
    asset: &LibraryAsset,
    _stage: ThumbnailLoadStage,
    _render_edited_thumbnails_during_indexing: bool,
) -> Result<LoadedLibraryThumbnail, String> {
    let Some(uri) = asset.android_uri() else {
        return Err("invalid Android thumbnail request".to_owned());
    };
    let display_name = asset.display_name.as_str();
    let bytes = asset.metadata.bytes;
    let modified_seconds = asset.metadata.modified_seconds;
    match calibraw_ffi::load_developed_thumbnail_cache(app, uri, display_name, THUMBNAIL_EDGE) {
        Ok(Some(thumbnail)) => {
            let mut loaded = loaded_library_thumbnail(thumbnail, true);
            loaded.review = match crate::sidecar::load_android_review(app, uri, display_name) {
                Ok(review) => review,
                Err(error) => {
                    log::warn!(
                        "could not inspect Android review sidecar for {display_name}: {error}"
                    );
                    None
                }
            };
            return Ok(loaded);
        }
        Ok(None) => {}
        Err(error) => log::warn!(
            "could not use Android developed-thumbnail cache for {display_name}: {error}"
        ),
    }
    let sidecar = match crate::sidecar::load_android(app, uri, display_name) {
        Ok(sidecar) => sidecar,
        Err(error) => {
            log::warn!("could not inspect Android edit sidecar for {display_name}: {error}");
            None
        }
    };
    let review = sidecar.as_ref().map(|sidecar| sidecar.review);
    let mut thumbnail = calibraw_ffi::load_library_thumbnail(
        app,
        uri,
        display_name,
        bytes,
        modified_seconds,
        THUMBNAIL_EDGE,
    )?;
    let has_edits = match sidecar {
        Some(sidecar) => {
            let has_edits = crate::sidecar::edit_state_has_adjustments(&sidecar.edits);
            thumbnail =
                crate::pipeline::transform_thumbnail_geometry(&thumbnail, sidecar.edits.geometry);
            has_edits
        }
        None => false,
    };
    let mut loaded = if has_edits {
        loaded_library_raw_preview_with_stale_edits(thumbnail)
    } else {
        loaded_library_thumbnail(thumbnail, false)
    };
    loaded.review = review;
    Ok(loaded)
}
