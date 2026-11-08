//! Library previews and the developed-thumbnail cache in app storage.
//!
//! Java owns the cache directory: it creates it, trims it to its size limit
//! and measures and clears it (`ThumbnailCache.java`). Rust names, reads and
//! writes the files in it, so a cached preview costs no call into Java. Files
//! are named by a SHA-256 of an identity string, which keeps the names
//! earlier versions stored. Trimming removes a `.jpg` with its `.fingerprint`
//! and takes a file's modification time as its last use, so reads mark files
//! used.

use super::*;
use calibraw_core::pipeline::{RawDisplayMetadata, RawThumbnail};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::OnceLock;
use std::time::SystemTime;

static CACHE_DIRECTORY: OnceLock<PathBuf> = OnceLock::new();

/// The persistent thumbnail cache directory, asked of Java once.
fn cache_directory(app: &AndroidApp) -> Result<&'static Path, String> {
    if let Some(directory) = CACHE_DIRECTORY.get() {
        return Ok(directory);
    }
    let path = with_storage_manager(app, |env, storage_manager| {
        let value = env.call_method(
            storage_manager,
            jni::jni_str!("thumbnailCacheDirectory"),
            jni::jni_sig!(() -> JString),
            &[],
        )?;
        java_string(env, value)
    })
    .map_err(|error| format!("could not locate Android thumbnail cache: {error:#}"))?;
    let directory = non_empty_path(path, "Android returned no thumbnail cache directory")?;
    Ok(CACHE_DIRECTORY.get_or_init(|| directory))
}

/// The cache file named by the SHA-256 of `identity`, in lowercase hex, and
/// `suffix`.
fn cache_file(app: &AndroidApp, identity: &str, suffix: &str) -> Result<PathBuf, String> {
    let digest = ring::digest::digest(&ring::digest::SHA256, identity.as_bytes());
    let mut name = String::with_capacity(digest.as_ref().len() * 2 + suffix.len());
    for byte in digest.as_ref() {
        // Writing to a String cannot fail.
        let _ = write!(name, "{byte:02x}");
    }
    name.push_str(suffix);
    Ok(cache_directory(app)?.join(name))
}

/// The preview extracted from one version of a photo. "raw-v2" dropped the
/// previews made before TIFF colour and tone handling settled; "raw-v3" drops
/// those left unrotated because the embedded preview stated no orientation.
fn raw_preview_path(
    app: &AndroidApp,
    uri: &str,
    bytes: u64,
    modified_seconds: u64,
    maximum_edge: u32,
) -> Result<PathBuf, String> {
    cache_file(
        app,
        &format!("raw-v3\n{uri}\n{bytes}\n{modified_seconds}\n{maximum_edge}"),
        ".raw.jpg",
    )
}

pub(super) fn developed_thumbnail_cache_path(
    app: &AndroidApp,
    uri: &str,
) -> Result<PathBuf, String> {
    cache_file(app, &format!("developed\n{uri}"), ".developed.jpg")
}

pub(super) fn fingerprint_path(cache_path: &Path) -> PathBuf {
    let mut path = cache_path.as_os_str().to_owned();
    path.push(".fingerprint");
    PathBuf::from(path)
}

/// Where the folder cache of `folder` within the linked folder `linked_folder`
/// (the library for an empty key) is kept.
pub fn library_folder_cache_path(
    app: &AndroidApp,
    linked_folder: &str,
    folder: &str,
) -> Result<PathBuf, String> {
    cache_file(
        app,
        &format!("folder-metadata\n{linked_folder}\n{folder}"),
        calibraw_core::display_metadata_cache::CACHE_SUFFIX,
    )
}

/// Marks a cached file and its fingerprint used now. Failing only makes the
/// file an earlier candidate for trimming.
fn mark_used(path: &Path) {
    let now = SystemTime::now();
    for path in [path.to_owned(), fingerprint_path(path)] {
        if let Ok(file) = fs::File::open(&path) {
            let _ = file.set_modified(now);
        }
    }
}

fn load_cached_jpeg(path: &Path, maximum_edge: u32) -> Result<Option<RawThumbnail>, String> {
    let thumbnail = calibraw_core::thumbnail_cache::load_jpeg(path, maximum_edge)?;
    if thumbnail.is_some() {
        mark_used(path);
    }
    Ok(thumbnail)
}

/// The stored preview of a library photo, or `None` when it has to be
/// extracted.
pub fn load_cached_library_preview(
    app: &AndroidApp,
    uri: &str,
    bytes: u64,
    modified_seconds: u64,
    maximum_edge: u32,
) -> Result<Option<RawThumbnail>, String> {
    load_cached_jpeg(
        &raw_preview_path(app, uri, bytes, modified_seconds, maximum_edge)?,
        maximum_edge,
    )
}

/// Reads a library photo's preview, and with `read_display_metadata` its
/// display metadata from the same read, and stores the preview.
pub fn extract_library_preview(
    app: &AndroidApp,
    uri: &str,
    display_name: &str,
    bytes: u64,
    modified_seconds: u64,
    maximum_edge: u32,
    read_display_metadata: bool,
) -> Result<(RawThumbnail, Option<RawDisplayMetadata>), String> {
    let extracted = read_library_photo(app, uri, display_name, "preview", |path| {
        if read_display_metadata {
            calibraw_core::pipeline::load_raw_thumbnail_with_display_metadata(path, maximum_edge)
        } else {
            calibraw_core::pipeline::load_raw_thumbnail(path, maximum_edge)
                .map(|thumbnail| (thumbnail, None))
        }
        .map_err(|error| format!("{error:#}"))
    })?;
    let cache_path = raw_preview_path(app, uri, bytes, modified_seconds, maximum_edge)?;
    match calibraw_core::thumbnail_cache::save_jpeg(&cache_path, &extracted.0) {
        Ok(()) => maintain_thumbnail_cache(app),
        Err(error) => log::warn!("could not persist Android RAW thumbnail: {error}"),
    }
    Ok(extracted)
}

/// A library photo's display metadata, read on its own.
pub fn load_library_display_metadata(
    app: &AndroidApp,
    uri: &str,
    display_name: &str,
) -> Result<RawDisplayMetadata, String> {
    read_library_photo(app, uri, display_name, "display metadata", |path| {
        calibraw_core::serialized_reads::run(|| {
            calibraw_core::pipeline::load_raw_display_metadata(path)
        })
        .map_err(|error| format!("{error:#}"))
    })
}

/// Runs `read` on the photo through its descriptor. The decoders reopen it by
/// path, which a document provider may refuse or answer with an unseekable
/// pipe; then `read` runs again on a private copy. `what` names the read in
/// the log line that reports the copy.
pub fn read_library_photo<T>(
    app: &AndroidApp,
    uri: &str,
    display_name: &str,
    what: &str,
    mut read: impl FnMut(&Path) -> Result<T, String>,
) -> Result<T, String> {
    let direct =
        open_library_descriptor(app, uri).and_then(|descriptor| read(&descriptor.proc_path()));
    let direct_error = match direct {
        Ok(value) => return Ok(value),
        Err(error) => error,
    };
    log::warn!(
        "reading the {what} of {display_name} directly failed; reading a copy: {direct_error}"
    );
    let temporary = materialize_library_thumbnail(app, uri, display_name)?;
    let result = read(&temporary);
    if let Err(error) = fs::remove_file(&temporary) {
        log::warn!(
            "could not remove Android thumbnail staging file {}: {error}",
            temporary.display()
        );
    }
    result
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

/// The line the fingerprint file of a developed thumbnail holds.
fn developed_fingerprint_line(render_fingerprint: u64) -> String {
    format!(
        "{:016x}\n",
        render_fingerprint ^ calibraw_core::sidecar::DEVELOPED_THUMBNAIL_CACHE_VERSION_SALT
    )
}

/// Whether the developed thumbnail at `cache_path` was saved while the
/// sidecar had `render_fingerprint`. False when it has no fingerprint.
fn developed_fingerprint_matches(
    cache_path: &Path,
    render_fingerprint: u64,
) -> Result<bool, String> {
    let fingerprint_path = fingerprint_path(cache_path);
    match fs::read_to_string(&fingerprint_path) {
        Ok(saved) => Ok(saved.trim() == developed_fingerprint_line(render_fingerprint).trim()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "could not read Android developed-thumbnail fingerprint {}: {error}",
            fingerprint_path.display()
        )),
    }
}

fn remove_developed_thumbnail(cache_path: &Path) {
    let _ = fs::remove_file(cache_path);
    let _ = fs::remove_file(fingerprint_path(cache_path));
}

/// The [`calibraw_core::sidecar::render_fingerprint`] of the photo's sidecar,
/// or `None` when it has none.
fn sidecar_render_fingerprint(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
) -> Result<Option<u64>, String> {
    let Some(sidecar_path) = materialize_raw_sidecar(app, raw_uri, display_name)? else {
        return Ok(None);
    };
    let fingerprint = calibraw_core::sidecar::read_bounded(&sidecar_path)
        .map_err(|error| error.to_string())
        .and_then(|bytes| calibraw_core::sidecar::render_fingerprint(&bytes));
    let _ = fs::remove_file(&sidecar_path);
    fingerprint.map(Some)
}

/// The developed thumbnail saved while the photo's sidecar had
/// `render_fingerprint`; `None` when there is none or it is out of date.
pub fn load_current_developed_thumbnail(
    app: &AndroidApp,
    raw_uri: &str,
    render_fingerprint: u64,
    maximum_edge: u32,
) -> Result<Option<RawThumbnail>, String> {
    let cache_path = developed_thumbnail_cache_path(app, raw_uri)?;
    if !developed_fingerprint_matches(&cache_path, render_fingerprint)? {
        return Ok(None);
    }
    load_cached_jpeg(&cache_path, maximum_edge)
}

/// The developed thumbnail, checked against the photo's sidecar as it is now.
/// A thumbnail that no longer matches is removed.
pub fn load_developed_thumbnail_cache(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
    maximum_edge: u32,
) -> Result<Option<RawThumbnail>, String> {
    let cache_path = developed_thumbnail_cache_path(app, raw_uri)?;
    if !cache_path.is_file() || !fingerprint_path(&cache_path).is_file() {
        return Ok(None);
    }
    let current = match sidecar_render_fingerprint(app, raw_uri, display_name)? {
        Some(fingerprint) => developed_fingerprint_matches(&cache_path, fingerprint)?,
        None => false,
    };
    if !current {
        remove_developed_thumbnail(&cache_path);
        return Ok(None);
    }
    load_cached_jpeg(&cache_path, maximum_edge)
}

pub fn save_developed_thumbnail_cache(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
    thumbnail: &RawThumbnail,
) -> Result<(), String> {
    let Some(fingerprint) = sidecar_render_fingerprint(app, raw_uri, display_name)? else {
        return Err("edit sidecar disappeared before thumbnail capture".to_owned());
    };
    if store_developed_thumbnail(app, raw_uri, display_name, thumbnail, fingerprint)? {
        Ok(())
    } else {
        Err("edit sidecar changed while its thumbnail was being cached".to_owned())
    }
}

/// Saves a thumbnail rendered from the photo's sidecar while it had
/// `render_fingerprint`. False, and nothing kept, when the sidecar no longer
/// has that fingerprint.
pub fn save_rendered_developed_thumbnail(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
    thumbnail: &RawThumbnail,
    render_fingerprint: u64,
) -> Result<bool, String> {
    if sidecar_render_fingerprint(app, raw_uri, display_name)? != Some(render_fingerprint) {
        return Ok(false);
    }
    store_developed_thumbnail(app, raw_uri, display_name, thumbnail, render_fingerprint)
}

/// Writes the developed thumbnail with `render_fingerprint`. The sidecar is
/// read again afterwards: an edit saved meanwhile makes the thumbnail out of
/// date the moment it is written, so it is removed and false returned.
fn store_developed_thumbnail(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
    thumbnail: &RawThumbnail,
    render_fingerprint: u64,
) -> Result<bool, String> {
    let cache_path = developed_thumbnail_cache_path(app, raw_uri)?;
    let fingerprint_path = fingerprint_path(&cache_path);
    calibraw_core::thumbnail_cache::save_jpeg(&cache_path, thumbnail)?;
    calibraw_core::file_ops::write_bytes_atomically(
        &fingerprint_path,
        developed_fingerprint_line(render_fingerprint).as_bytes(),
    )
    .map_err(|error| {
        format!(
            "could not write Android developed-thumbnail fingerprint {}: {error}",
            fingerprint_path.display()
        )
    })?;
    if sidecar_render_fingerprint(app, raw_uri, display_name)? != Some(render_fingerprint) {
        remove_developed_thumbnail(&cache_path);
        return Ok(false);
    }
    maintain_thumbnail_cache(app);
    Ok(true)
}

/// Gives the photo at `destination_uri` the developed thumbnail of the one at
/// `source_uri`, when that has one.
pub fn copy_library_developed_thumbnail_cache(
    app: &AndroidApp,
    source_uri: &str,
    destination_uri: &str,
) -> Result<(), String> {
    let source = developed_thumbnail_cache_path(app, source_uri)?;
    let source_fingerprint = fingerprint_path(&source);
    if !source.is_file() || !source_fingerprint.is_file() {
        return Ok(());
    }
    let destination = developed_thumbnail_cache_path(app, destination_uri)?;
    let copied = fs::copy(&source, &destination)
        .and_then(|_| fs::copy(&source_fingerprint, fingerprint_path(&destination)));
    if let Err(error) = copied {
        remove_developed_thumbnail(&destination);
        return Err(format!(
            "could not preserve Android developed thumbnail: {error}"
        ));
    }
    maintain_thumbnail_cache(app);
    Ok(())
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
