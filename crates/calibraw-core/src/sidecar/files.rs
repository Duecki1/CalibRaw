//! Reading and writing sidecars beside RAW files: locking, backups, atomic replacement.

use super::*;

pub fn load_desktop(raw_path: &Path) -> Result<Option<LoadedSidecar>, SidecarError> {
    let path = sidecar_path_for_raw(raw_path);
    match read_bounded(&path) {
        Ok(bytes) => decode(&bytes).map(Some),
        Err(SidecarError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

// Serialize read/modify/write operations so background development saves cannot lose
// a review update made from the gallery on another thread.
static SIDECAR_SAVE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn save_desktop(raw_path: &Path, edits: EditState) -> Result<PathBuf, SidecarError> {
    let _guard = SIDECAR_SAVE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let path = sidecar_path_for_raw(raw_path);
    let metadata = load_sidecar_metadata(raw_path)?;
    let uses_ai_denoise = edits.exposure.ai_denoise_enabled;
    let bytes =
        encode_with_review_and_editing_time(edits, metadata.review, metadata.editing_time_ms)?;
    atomic_write(&path, &bytes)?;
    remove_unused_ai_denoise_result(raw_path, uses_ai_denoise);
    Ok(path)
}

pub fn save_desktop_with_editing_time(
    raw_path: &Path,
    edits: EditState,
    editing_time_ms: u64,
) -> Result<PathBuf, SidecarError> {
    let _guard = SIDECAR_SAVE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let path = sidecar_path_for_raw(raw_path);
    let review = load_sidecar_metadata(raw_path)?.review;
    let uses_ai_denoise = edits.exposure.ai_denoise_enabled;
    let bytes = encode_with_review_and_editing_time(edits, review, editing_time_ms)?;
    atomic_write(&path, &bytes)?;
    remove_unused_ai_denoise_result(raw_path, uses_ai_denoise);
    Ok(path)
}

/// Preserve an unreadable or newer desktop sidecar before replacing it with a
/// current-schema document. The caller must explicitly request this recovery.
#[cfg(not(target_os = "android"))]
pub fn backup_and_replace_desktop_sidecar(
    raw_path: &Path,
    edits: EditState,
    editing_time_ms: u64,
) -> Result<PathBuf, SidecarError> {
    let _guard = SIDECAR_SAVE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let path = sidecar_path_for_raw(raw_path);
    let uses_ai_denoise = edits.exposure.ai_denoise_enabled;
    let bytes =
        encode_with_review_and_editing_time(edits, PhotoReview::default(), editing_time_ms)?;
    let backup = create_backup_copy(&path)?;
    atomic_write(&path, &bytes)?;
    remove_unused_ai_denoise_result(raw_path, uses_ai_denoise);
    Ok(backup)
}

/// Deletes the AI-denoise result next to a RAW once its saved edit no longer
/// uses AI denoise. A failure only leaves a stale file behind, so it is logged
/// instead of failing the save.
/// Android keeps AI-denoise results in app storage, never next to the RAW,
/// so there is nothing to remove here.
#[cfg(target_os = "android")]
fn remove_unused_ai_denoise_result(_raw_path: &Path, _uses_ai_denoise: bool) {}

#[cfg(not(target_os = "android"))]
fn remove_unused_ai_denoise_result(raw_path: &Path, uses_ai_denoise: bool) {
    if uses_ai_denoise {
        return;
    }
    let path = ai_denoise_path_for_raw(raw_path);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => log::warn!("could not remove {}: {error}", path.display()),
    }
}

/// Copies `path` to a new, uniquely named `<path>.backup-<pid>-<n>` file.
#[cfg(not(target_os = "android"))]
fn create_backup_copy(path: &Path) -> Result<PathBuf, SidecarError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_BACKUP_ID: AtomicU64 = AtomicU64::new(0);

    let mut source = File::open(path)?;
    loop {
        let mut name = path.as_os_str().to_owned();
        name.push(format!(
            ".backup-{}-{}",
            std::process::id(),
            NEXT_BACKUP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let backup = PathBuf::from(name);
        let mut destination = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup)
        {
            Ok(destination) => destination,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(SidecarError::Io(error)),
        };
        let copied =
            std::io::copy(&mut source, &mut destination).and_then(|_| destination.sync_all());
        drop(destination);
        if let Err(error) = copied {
            let _ = std::fs::remove_file(&backup);
            return Err(SidecarError::Io(error));
        }
        if let Some(parent) = backup.parent() {
            crate::file_ops::sync_parent_directory(parent)?;
        }
        return Ok(backup);
    }
}

#[cfg(not(target_os = "android"))]
pub fn reset_desktop_adjustments(raw_path: &Path) -> Result<bool, String> {
    reset_desktop_adjustments_impl(raw_path, None)
}

#[cfg(not(target_os = "android"))]
pub fn reset_desktop_adjustments_with_editing_time(
    raw_path: &Path,
    editing_time_ms: u64,
) -> Result<bool, String> {
    reset_desktop_adjustments_impl(raw_path, Some(editing_time_ms))
}

#[cfg(not(target_os = "android"))]
fn reset_desktop_adjustments_impl(
    raw_path: &Path,
    editing_time_override_ms: Option<u64>,
) -> Result<bool, String> {
    let metadata = load_sidecar_metadata(raw_path).map_err(|error| error.to_string())?;
    let review = metadata.review;
    let editing_time_ms = editing_time_override_ms.unwrap_or(metadata.editing_time_ms);
    if review == PhotoReview::default() && editing_time_ms == 0 {
        return remove_desktop_edits(raw_path);
    }
    save_desktop_with_editing_time(raw_path, default_edit_state(), editing_time_ms)
        .map_err(|error| error.to_string())?;
    invalidate_developed_thumbnail_cache(raw_path)?;
    Ok(true)
}

/// Read review/timer metadata without decoding embedded masks or development state.
pub fn decode_sidecar_metadata(bytes: &[u8]) -> Result<SidecarMetadata, SidecarError> {
    if bytes.len() as u64 > MAX_SIDECAR_BYTES {
        return Err(SidecarError::TooLarge(bytes.len() as u64));
    }
    #[derive(Deserialize)]
    struct MetadataHeader {
        format: String,
        schema_version: u32,
        #[serde(default)]
        review: PhotoReview,
        #[serde(default)]
        editing_time_ms: u64,
    }
    let header: MetadataHeader =
        serde_json::from_slice(bytes).map_err(|error| SidecarError::Invalid(error.to_string()))?;
    if header.format != SIDECAR_FORMAT || header.schema_version != SIDECAR_SCHEMA_VERSION {
        return Err(SidecarError::Unsupported(
            "unsupported review sidecar".to_owned(),
        ));
    }
    Ok(SidecarMetadata {
        review: PhotoReview {
            rating: header.review.rating.min(5),
            ..header.review
        },
        editing_time_ms: header.editing_time_ms,
    })
}

/// Read review/timer metadata from a desktop sidecar, returning defaults when absent.
pub fn load_sidecar_metadata(raw_path: &Path) -> Result<SidecarMetadata, SidecarError> {
    let bytes = match read_bounded(&sidecar_path_for_raw(raw_path)) {
        Ok(bytes) => bytes,
        Err(SidecarError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SidecarMetadata::default())
        }
        Err(error) => return Err(error),
    };
    decode_sidecar_metadata(&bytes)
}

/// Read review metadata without decoding embedded masks or development state.
pub fn load_photo_review(raw_path: &Path) -> Result<PhotoReview, SidecarError> {
    load_sidecar_metadata(raw_path).map(|metadata| metadata.review)
}

/// Inspect preview geometry and edit presence without decoding embedded image assets.
pub fn load_photo_preview_info(raw_path: &Path) -> Result<(GeometryTransform, bool), SidecarError> {
    #[derive(Deserialize)]
    struct PreviewDocument {
        edits: serde_json::Value,
    }
    let bytes = match read_bounded(&sidecar_path_for_raw(raw_path)) {
        Ok(bytes) => bytes,
        Err(SidecarError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((GeometryTransform::default(), false))
        }
        Err(error) => return Err(error),
    };
    let mut document: PreviewDocument =
        serde_json::from_slice(&bytes).map_err(|error| SidecarError::Invalid(error.to_string()))?;
    let geometry: GeometryTransform = document
        .edits
        .get("geometry")
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|error| SidecarError::Invalid(error.to_string()))?
        .unwrap_or_default();
    let mut default = serde_json::from_slice::<PreviewDocument>(&encode(default_edit_state())?)
        .map_err(|error| SidecarError::Invalid(error.to_string()))?
        .edits;
    // This bookkeeping flag alone is not a development adjustment.
    for value in [&mut document.edits, &mut default] {
        if let Some(object) = value.as_object_mut() {
            object.remove("ai_masks_need_update");
        }
    }
    Ok((geometry.sanitized(), document.edits != default))
}

pub fn save_photo_review(raw_path: &Path, review: PhotoReview) -> Result<(), SidecarError> {
    save_photo_review_impl(raw_path, review, None)
}

pub fn save_photo_review_with_editing_time(
    raw_path: &Path,
    review: PhotoReview,
    editing_time_ms: u64,
) -> Result<(), SidecarError> {
    save_photo_review_impl(raw_path, review, Some(editing_time_ms))
}

fn save_photo_review_impl(
    raw_path: &Path,
    review: PhotoReview,
    editing_time_override_ms: Option<u64>,
) -> Result<(), SidecarError> {
    let _guard = SIDECAR_SAVE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if review.rating > 5 {
        return Err(SidecarError::Invalid(
            "rating must be between 0 and 5".to_owned(),
        ));
    }
    // Validate the existing document before modifying it, preserving all adjustment assets.
    load_sidecar_metadata(raw_path)?;
    let path = sidecar_path_for_raw(raw_path);
    let bytes = match read_bounded(&path) {
        Ok(bytes) => bytes,
        Err(SidecarError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            encode(default_edit_state())?
        }
        Err(error) => return Err(error),
    };
    let mut document: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| SidecarError::Invalid(error.to_string()))?;
    document["review"] =
        serde_json::to_value(review).map_err(|error| SidecarError::Invalid(error.to_string()))?;
    if let Some(editing_time_ms) = editing_time_override_ms {
        if editing_time_ms == 0 {
            if let Some(object) = document.as_object_mut() {
                object.remove("editing_time_ms");
            }
        } else {
            document["editing_time_ms"] = editing_time_ms.into();
        }
    }
    let bytes =
        serde_json::to_vec(&document).map_err(|error| SidecarError::Invalid(error.to_string()))?;
    if bytes.len() as u64 > MAX_SIDECAR_BYTES {
        return Err(SidecarError::TooLarge(bytes.len() as u64));
    }
    atomic_write(&path, &bytes)
}

pub fn read_bounded(path: &Path) -> Result<Vec<u8>, SidecarError> {
    let file = File::open(path)?;
    let declared = file.metadata()?.len();
    if declared > MAX_SIDECAR_BYTES {
        return Err(SidecarError::TooLarge(declared));
    }
    let mut bytes = Vec::with_capacity(declared as usize);
    file.take(MAX_SIDECAR_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SIDECAR_BYTES {
        return Err(SidecarError::TooLarge(bytes.len() as u64));
    }
    Ok(bytes)
}

/// Fingerprint only sidecar content that can affect rendered pixels.
/// Review metadata and accumulated editing time intentionally do not invalidate thumbnails.
pub fn render_fingerprint(bytes: &[u8]) -> Result<u64, String> {
    let canonical = if let Ok(mut document) = serde_json::from_slice::<serde_json::Value>(bytes) {
        if let Some(object) = document.as_object_mut() {
            object.remove("review");
            object.remove("editing_time_ms");
        }
        serde_json::to_vec(&document)
            .map_err(|error| format!("could not fingerprint edit sidecar: {error}"))?
    } else {
        bytes.to_vec()
    };
    let mut fingerprint = 0xcbf2_9ce4_8422_2325u64;
    for byte in canonical {
        fingerprint ^= u64::from(byte);
        fingerprint = fingerprint.wrapping_mul(0x0000_0100_0000_01b3);
    }
    Ok(fingerprint)
}

#[cfg(target_os = "android")]
pub fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), SidecarError> {
    if bytes.len() as u64 > MAX_SIDECAR_BYTES {
        return Err(SidecarError::TooLarge(bytes.len() as u64));
    }
    let mut file = OpenOptions::new().write(true).truncate(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub(super) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), SidecarError> {
    write_atomically(path, |file| file.write_all(bytes)).map_err(SidecarError::Io)
}
