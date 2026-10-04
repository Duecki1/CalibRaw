//! Library thumbnails and the developed-thumbnail cache in app storage.

use super::*;

pub fn load_library_thumbnail(
    app: &AndroidApp,
    uri: &str,
    display_name: &str,
    bytes: u64,
    modified_seconds: u64,
    maximum_edge: u32,
) -> Result<calibraw_core::pipeline::RawThumbnail, String> {
    let cache_path = raw_thumbnail_cache_path(app, uri, bytes, modified_seconds, maximum_edge)?;
    match calibraw_core::thumbnail_cache::load_jpeg(&cache_path, maximum_edge) {
        Ok(Some(thumbnail)) => return Ok(thumbnail),
        Ok(None) => {}
        Err(error) => log::warn!("discarding Android RAW thumbnail cache: {error}"),
    }

    let direct = load_library_thumbnail_from_fd(app, uri, maximum_edge);
    let thumbnail = match direct {
        Ok(thumbnail) => thumbnail,
        Err(direct_error) => {
            log::warn!(
                "direct Android RAW thumbnail extraction failed; retrying from private cache: {direct_error}"
            );
            let temporary = materialize_library_thumbnail(app, uri, display_name)?;
            let result = calibraw_core::pipeline::load_raw_thumbnail(&temporary, maximum_edge)
                .map_err(|error| format!("{error:#}"));
            if let Err(error) = fs::remove_file(&temporary) {
                log::warn!(
                    "could not remove Android thumbnail staging file {}: {error}",
                    temporary.display()
                );
            }
            result?
        }
    };
    match calibraw_core::thumbnail_cache::save_jpeg(&cache_path, &thumbnail) {
        Ok(()) => maintain_thumbnail_cache(app),
        Err(error) => log::warn!("could not persist Android RAW thumbnail: {error}"),
    }
    Ok(thumbnail)
}

pub fn copy_library_developed_thumbnail_cache(
    app: &AndroidApp,
    source_uri: &str,
    destination_uri: &str,
) -> Result<(), String> {
    with_storage_manager(app, |env, storage_manager| {
        let source_uri = env.new_string(source_uri)?;
        let destination_uri = env.new_string(destination_uri)?;
        env.call_method(
            storage_manager,
            jni::jni_str!("copyRawLibraryDevelopedThumbnail"),
            jni::jni_sig!((JString, JString) -> void),
            &[
                JValue::Object(&source_uri),
                JValue::Object(&destination_uri),
            ],
        )?;
        Ok(())
    })
    .map_err(|error| format!("could not preserve Android developed thumbnail: {error:#}"))
}

pub fn clear_thumbnail_cache(app: &AndroidApp) -> Result<(), String> {
    with_storage_manager(app, |env, storage_manager| {
        env.call_method(
            storage_manager,
            jni::jni_str!("clearThumbnailCache"),
            jni::jni_sig!(() -> void),
            &[],
        )?;
        Ok(())
    })
    .map_err(|error| format!("could not clear Android thumbnail cache: {error:#}"))
}

pub fn thumbnail_cache_size_bytes(app: &AndroidApp) -> Result<u64, String> {
    let bytes = with_storage_manager(app, |env, storage_manager| {
        env.call_method(
            storage_manager,
            jni::jni_str!("thumbnailCacheSizeBytes"),
            jni::jni_sig!(() -> i64),
            &[],
        )?
        .j()
    })
    .map_err(|error| format!("could not measure Android thumbnail cache: {error:#}"))?;
    u64::try_from(bytes).map_err(|_| "Android returned a negative thumbnail cache size".to_owned())
}

fn maintain_thumbnail_cache(app: &AndroidApp) {
    if let Err(error) = with_storage_manager(app, |env, storage_manager| {
        env.call_method(
            storage_manager,
            jni::jni_str!("maintainThumbnailCache"),
            jni::jni_sig!(() -> void),
            &[],
        )?;
        Ok(())
    }) {
        log::warn!("could not maintain Android thumbnail cache: {error:#}");
    }
}

pub fn load_library_display_dimensions(app: &AndroidApp, uri: &str) -> Result<[u32; 2], String> {
    let descriptor = open_library_descriptor(app, uri)?;
    calibraw_core::pipeline::load_raw_display_dimensions(&descriptor.proc_path())
        .map_err(|error| format!("{error:#}"))
}

fn load_library_thumbnail_from_fd(
    app: &AndroidApp,
    uri: &str,
    maximum_edge: u32,
) -> Result<calibraw_core::pipeline::RawThumbnail, String> {
    let descriptor = open_library_descriptor(app, uri)?;
    calibraw_core::pipeline::load_raw_thumbnail(&descriptor.proc_path(), maximum_edge)
        .map_err(|error| format!("{error:#}"))
}

fn open_library_descriptor(
    app: &AndroidApp,
    uri: &str,
) -> Result<TransferredFileDescriptor, String> {
    let raw_fd = with_storage_manager(app, |env, storage_manager| {
        let uri = env.new_string(uri)?;
        env.call_method(
            storage_manager,
            jni::jni_str!("openRawLibraryFd"),
            jni::jni_sig!((JString) -> i32),
            &[JValue::Object(&uri)],
        )?
        .i()
    })
    .map_err(|error| format!("could not open Android RAW library item: {error:#}"))?;
    TransferredFileDescriptor::from_java(raw_fd, "Android returned an invalid RAW file descriptor")
}

fn raw_thumbnail_cache_path(
    app: &AndroidApp,
    uri: &str,
    bytes: u64,
    modified_seconds: u64,
    maximum_edge: u32,
) -> Result<PathBuf, String> {
    let path = with_storage_manager(app, |env, storage_manager| {
        let uri = env.new_string(uri)?;
        let value = env.call_method(
            storage_manager,
            jni::jni_str!("rawThumbnailCachePath"),
            jni::jni_sig!((JString, i64, i64, i32) -> JString),
            &[
                JValue::Object(&uri),
                JValue::Long(bytes as i64),
                JValue::Long(modified_seconds as i64),
                JValue::Int(maximum_edge as i32),
            ],
        )?;
        java_string(env, value)
    })
    .map_err(|error| format!("could not locate Android thumbnail cache: {error:#}"))?;
    non_empty_path(path, "Android returned no thumbnail cache path")
}

pub(super) fn developed_thumbnail_cache_path(
    app: &AndroidApp,
    uri: &str,
) -> Result<PathBuf, String> {
    let path = with_storage_manager(app, |env, storage_manager| {
        let uri = env.new_string(uri)?;
        let value = env.call_method(
            storage_manager,
            jni::jni_str!("developedThumbnailCachePath"),
            jni::jni_sig!((JString) -> JString),
            &[JValue::Object(&uri)],
        )?;
        java_string(env, value)
    })
    .map_err(|error| format!("could not locate Android developed-thumbnail cache: {error:#}"))?;
    non_empty_path(path, "Android returned no developed-thumbnail cache path")
}

pub(super) fn developed_thumbnail_fingerprint_path(cache_path: &std::path::Path) -> PathBuf {
    let mut path = cache_path.as_os_str().to_owned();
    path.push(".fingerprint");
    PathBuf::from(path)
}

pub fn load_developed_thumbnail_cache(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
    maximum_edge: u32,
) -> Result<Option<calibraw_core::pipeline::RawThumbnail>, String> {
    let cache_path = developed_thumbnail_cache_path(app, raw_uri)?;
    let fingerprint_path = developed_thumbnail_fingerprint_path(&cache_path);
    if !cache_path.is_file() || !fingerprint_path.is_file() {
        return Ok(None);
    }
    let Some(sidecar_path) = materialize_raw_sidecar(app, raw_uri, display_name)? else {
        let _ = fs::remove_file(&cache_path);
        let _ = fs::remove_file(&fingerprint_path);
        return Ok(None);
    };
    let fingerprint = calibraw_core::sidecar::read_bounded(&sidecar_path)
        .map_err(|error| error.to_string())
        .and_then(|bytes| calibraw_core::sidecar::render_fingerprint(&bytes));
    let _ = fs::remove_file(&sidecar_path);
    let fingerprint = fingerprint?;
    let cached = fs::read_to_string(&fingerprint_path).map_err(|error| {
        format!(
            "could not read Android developed-thumbnail fingerprint {}: {error}",
            fingerprint_path.display()
        )
    })?;
    if cached.trim()
        != format!(
            "{:016x}",
            fingerprint ^ calibraw_core::sidecar::DEVELOPED_THUMBNAIL_CACHE_VERSION_SALT
        )
    {
        let _ = fs::remove_file(&cache_path);
        let _ = fs::remove_file(&fingerprint_path);
        return Ok(None);
    }
    calibraw_core::thumbnail_cache::load_jpeg(&cache_path, maximum_edge)
}

pub fn save_developed_thumbnail_cache(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
    thumbnail: &calibraw_core::pipeline::RawThumbnail,
) -> Result<(), String> {
    let Some(sidecar_path) = materialize_raw_sidecar(app, raw_uri, display_name)? else {
        return Err("edit sidecar disappeared before thumbnail capture".to_owned());
    };
    let fingerprint = calibraw_core::sidecar::read_bounded(&sidecar_path)
        .map_err(|error| error.to_string())
        .and_then(|bytes| calibraw_core::sidecar::render_fingerprint(&bytes));
    let _ = fs::remove_file(&sidecar_path);
    let fingerprint = fingerprint?;
    let cache_path = developed_thumbnail_cache_path(app, raw_uri)?;
    let fingerprint_path = developed_thumbnail_fingerprint_path(&cache_path);
    calibraw_core::thumbnail_cache::save_jpeg(&cache_path, thumbnail)?;
    calibraw_core::file_ops::write_bytes_atomically(
        &fingerprint_path,
        format!(
            "{:016x}\n",
            fingerprint ^ calibraw_core::sidecar::DEVELOPED_THUMBNAIL_CACHE_VERSION_SALT
        )
        .as_bytes(),
    )
    .map_err(|error| {
        format!(
            "could not write Android developed-thumbnail fingerprint {}: {error}",
            fingerprint_path.display()
        )
    })?;

    let Some(sidecar_path) = materialize_raw_sidecar(app, raw_uri, display_name)? else {
        let _ = fs::remove_file(&cache_path);
        let _ = fs::remove_file(&fingerprint_path);
        return Err("edit sidecar changed while its thumbnail was being cached".to_owned());
    };
    let latest = calibraw_core::sidecar::read_bounded(&sidecar_path)
        .map_err(|error| error.to_string())
        .and_then(|bytes| calibraw_core::sidecar::render_fingerprint(&bytes));
    let _ = fs::remove_file(&sidecar_path);
    if latest? != fingerprint {
        let _ = fs::remove_file(&cache_path);
        let _ = fs::remove_file(&fingerprint_path);
        return Err("edit sidecar changed while its thumbnail was being cached".to_owned());
    }
    maintain_thumbnail_cache(app);
    Ok(())
}
