//! Sidecars and AI results for content-URI documents.

use super::*;

/// The sidecar writes this process made: how many, and for each photo URI the
/// count its latest write made. A library listing taken before a write does
/// not show it.
#[derive(Default)]
struct SidecarWrites {
    count: u64,
    latest: HashMap<String, u64>,
}

static SIDECAR_WRITES: OnceLock<Mutex<SidecarWrites>> = OnceLock::new();

fn sidecar_writes() -> std::sync::MutexGuard<'static, SidecarWrites> {
    // Every update leaves the record consistent, so a poisoned lock is usable.
    SIDECAR_WRITES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Every change this process makes to a sidecar passes through here.
pub(super) fn note_sidecar_written(raw_uri: &str) {
    let mut writes = sidecar_writes();
    writes.count += 1;
    let count = writes.count;
    writes.latest.insert(raw_uri.to_owned(), count);
}

/// How many sidecar writes this process has made; taken before listing a
/// folder, for [`sidecar_written_since`].
pub fn sidecar_write_count() -> u64 {
    sidecar_writes().count
}

/// Whether this process wrote the photo's sidecar after `count` was taken.
pub fn sidecar_written_since(raw_uri: &str, count: u64) -> bool {
    sidecar_writes()
        .latest
        .get(raw_uri)
        .is_some_and(|&written| written > count)
}

/// Where the RAW's AI-denoise result lives, next to its sidecar.
pub fn ai_denoise_result_path(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
) -> Result<PathBuf, String> {
    with_storage_manager(app, |env, storage_manager| {
        let raw_uri = env.new_string(raw_uri)?;
        let display_name = env.new_string(display_name)?;
        let object = env
            .call_method(
                storage_manager,
                jni::jni_str!("aiDenoiseResultPath"),
                jni::jni_sig!((JString, JString) -> JString),
                &[JValue::Object(&raw_uri), JValue::Object(&display_name)],
            )?
            .l()?;
        let path = env.cast_local::<JString>(object)?;
        Ok(PathBuf::from(path.to_string()))
    })
    .map_err(|error| format!("could not locate the Android AI-denoise result: {error:#}"))
}

/// Where the RAW's people analysis lives, next to its sidecar.
pub fn people_analysis_path(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
) -> Result<PathBuf, String> {
    with_storage_manager(app, |env, storage_manager| {
        let raw_uri = env.new_string(raw_uri)?;
        let display_name = env.new_string(display_name)?;
        let object = env
            .call_method(
                storage_manager,
                jni::jni_str!("peopleAnalysisPath"),
                jni::jni_sig!((JString, JString) -> JString),
                &[JValue::Object(&raw_uri), JValue::Object(&display_name)],
            )?
            .l()?;
        let path = env.cast_local::<JString>(object)?;
        Ok(PathBuf::from(path.to_string()))
    })
    .map_err(|error| format!("could not locate the Android people analysis: {error:#}"))
}

fn call_companion_method(
    app: &AndroidApp,
    method: impl AsRef<jni::strings::JNIStr>,
    raw_uri: &str,
    display_name: &str,
    companion: calibraw_core::sidecar::PhotoCompanion,
) -> jni::errors::Result<()> {
    with_storage_manager(app, |env, storage_manager| {
        let raw_uri = env.new_string(raw_uri)?;
        let display_name = env.new_string(display_name)?;
        let suffix = env.new_string(companion.suffix())?;
        env.call_method(
            storage_manager,
            method,
            jni::jni_sig!((JString, JString, JString) -> void),
            &[
                JValue::Object(&raw_uri),
                JValue::Object(&display_name),
                JValue::Object(&suffix),
            ],
        )?;
        Ok(())
    })
}

/// Makes the companion's local path hold what the photo's folder has. May
/// copy a large file, so it is called off the UI thread.
pub fn fetch_photo_companion(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
    companion: calibraw_core::sidecar::PhotoCompanion,
) -> Result<(), String> {
    call_companion_method(
        app,
        jni::jni_str!("fetchPhotoCompanion"),
        raw_uri,
        display_name,
        companion,
    )
    .map_err(|error| format!("{error:#}"))
}

/// Stores what was written at the companion's local path in the photo's
/// folder. May copy a large file, so it is called off the UI thread.
pub fn publish_photo_companion(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
    companion: calibraw_core::sidecar::PhotoCompanion,
) -> Result<(), String> {
    call_companion_method(
        app,
        jni::jni_str!("publishPhotoCompanion"),
        raw_uri,
        display_name,
        companion,
    )
    .map_err(|error| format!("{error:#}"))
}

pub fn photo_companion_exists(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
    companion: calibraw_core::sidecar::PhotoCompanion,
) -> Result<bool, String> {
    with_storage_manager(app, |env, storage_manager| {
        let raw_uri = env.new_string(raw_uri)?;
        let display_name = env.new_string(display_name)?;
        let suffix = env.new_string(companion.suffix())?;
        env.call_method(
            storage_manager,
            jni::jni_str!("hasPhotoCompanion"),
            jni::jni_sig!((JString, JString, JString) -> bool),
            &[
                JValue::Object(&raw_uri),
                JValue::Object(&display_name),
                JValue::Object(&suffix),
            ],
        )?
        .z()
    })
    .map_err(|error| format!("{error:#}"))
}

/// Deletes a file kept next to the RAW once the saved edit no longer uses it.
/// A failure only leaves a stale file behind, so it is logged.
fn remove_unused_companion(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
    companion: calibraw_core::sidecar::PhotoCompanion,
) {
    if let Err(error) = call_companion_method(
        app,
        jni::jni_str!("removePhotoCompanion"),
        raw_uri,
        display_name,
        companion,
    ) {
        log::warn!("could not remove {}: {error:#}", companion.suffix());
    }
}

pub fn reset_android_adjustments(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
) -> Result<(), String> {
    reset_android_adjustments_impl(app, raw_uri, display_name, None)
}

pub fn reset_android_adjustments_with_editing_time(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
    editing_time_ms: u64,
) -> Result<(), String> {
    reset_android_adjustments_impl(app, raw_uri, display_name, Some(editing_time_ms))
}

fn reset_android_adjustments_impl(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
    editing_time_override_ms: Option<u64>,
) -> Result<(), String> {
    let metadata = load_android_sidecar_metadata(app, raw_uri, display_name)
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let review = metadata.review;
    let editing_time_ms = editing_time_override_ms.unwrap_or(metadata.editing_time_ms);
    if review == calibraw_core::sidecar::PhotoReview::default() && editing_time_ms == 0 {
        return remove_raw_sidecar(app, raw_uri, display_name);
    }
    save_android_with_review_and_editing_time(
        app,
        raw_uri,
        display_name,
        calibraw_core::sidecar::default_edit_state(),
        review,
        editing_time_ms,
    )
    .map_err(|error| error.to_string())?;
    clear_developed_thumbnail_cache(app, raw_uri);
    Ok(())
}

pub(super) fn clear_developed_thumbnail_cache(app: &AndroidApp, raw_uri: &str) {
    if let Ok(cache_path) = developed_thumbnail_cache_path(app, raw_uri) {
        let fingerprint_path = fingerprint_path(&cache_path);
        let _ = fs::remove_file(cache_path);
        let _ = fs::remove_file(fingerprint_path);
    }
}

pub fn materialize_raw_sidecar(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
) -> Result<Option<PathBuf>, String> {
    let path = with_storage_manager(app, |env, storage_manager| {
        let raw_uri = env.new_string(raw_uri)?;
        let display_name = env.new_string(display_name)?;
        let value = env.call_method(
            storage_manager,
            jni::jni_str!("materializeRawSidecar"),
            jni::jni_sig!((JString, JString) -> JString),
            &[JValue::Object(&raw_uri), JValue::Object(&display_name)],
        )?;
        java_string(env, value)
    })
    .map_err(|error| format!("could not read Android RAW sidecar: {error:#}"))?;
    Ok((!path.is_empty()).then(|| PathBuf::from(path)))
}

pub fn create_raw_sidecar_cache(app: &AndroidApp) -> Result<PathBuf, String> {
    let path = with_storage_manager(app, |env, storage_manager| {
        let value = env.call_method(
            storage_manager,
            jni::jni_str!("createRawSidecarCache"),
            jni::jni_sig!(() -> JString),
            &[],
        )?;
        java_string(env, value)
    })
    .map_err(|error| format!("could not create Android sidecar cache: {error:#}"))?;
    non_empty_path(path, "Android returned no sidecar cache path")
}

pub fn publish_raw_sidecar(
    app: &AndroidApp,
    cached_path: &std::path::Path,
    raw_uri: &str,
    display_name: &str,
) -> Result<String, String> {
    let cached_path = cached_path
        .to_str()
        .ok_or_else(|| "Android sidecar cache path is not valid UTF-8".to_owned())?;
    let published = with_storage_manager(app, |env, storage_manager| {
        let cached_path = env.new_string(cached_path)?;
        let raw_uri = env.new_string(raw_uri)?;
        let display_name = env.new_string(display_name)?;
        let value = env.call_method(
            storage_manager,
            jni::jni_str!("publishRawSidecar"),
            jni::jni_sig!((JString, JString, JString) -> JString),
            &[
                JValue::Object(&cached_path),
                JValue::Object(&raw_uri),
                JValue::Object(&display_name),
            ],
        )?;
        java_string(env, value)
    })
    .map_err(|error| format!("could not publish Android RAW sidecar: {error:#}"));
    // Even a failed publish may have replaced the sidecar.
    note_sidecar_written(raw_uri);
    published
}

/// Decodes the photo's sidecar from a private copy, which is removed after;
/// `None` when the photo has no sidecar.
fn read_android_sidecar<T>(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
    decode: impl FnOnce(&[u8]) -> Result<T, calibraw_core::sidecar::SidecarError>,
) -> Result<Option<T>, calibraw_core::sidecar::SidecarError> {
    let Some(path) = materialize_raw_sidecar(app, raw_uri, display_name)
        .map_err(calibraw_core::sidecar::SidecarError::Platform)?
    else {
        return Ok(None);
    };
    let result = calibraw_core::sidecar::read_bounded(&path).and_then(|bytes| decode(&bytes));
    if let Err(error) = fs::remove_file(&path) {
        log::warn!(
            "could not remove Android sidecar cache {}: {error}",
            path.display()
        );
    }
    result.map(Some)
}

pub fn load_android(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
) -> Result<Option<calibraw_core::sidecar::LoadedSidecar>, calibraw_core::sidecar::SidecarError> {
    read_android_sidecar(app, raw_uri, display_name, calibraw_core::sidecar::decode)
}

fn load_android_sidecar_metadata(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
) -> Result<Option<calibraw_core::sidecar::SidecarMetadata>, calibraw_core::sidecar::SidecarError> {
    read_android_sidecar(
        app,
        raw_uri,
        display_name,
        calibraw_core::sidecar::decode_sidecar_metadata,
    )
}

pub fn load_android_review(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
) -> Result<Option<calibraw_core::sidecar::PhotoReview>, calibraw_core::sidecar::SidecarError> {
    load_android_sidecar_metadata(app, raw_uri, display_name)
        .map(|metadata| metadata.map(|metadata| metadata.review))
}

/// What the library grid needs from the photo's sidecar; `None` when it has
/// none.
pub fn load_android_sidecar_summary(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
) -> Result<Option<calibraw_core::sidecar::SidecarSummary>, calibraw_core::sidecar::SidecarError> {
    read_android_sidecar(
        app,
        raw_uri,
        display_name,
        calibraw_core::sidecar::SidecarSummary::from_bytes,
    )
}

pub fn save_android(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
    edits: calibraw_core::sidecar::EditState,
) -> Result<String, calibraw_core::sidecar::SidecarError> {
    let metadata = load_android_sidecar_metadata(app, raw_uri, display_name)?.unwrap_or_default();
    let review = metadata.review;
    let editing_time_ms = metadata.editing_time_ms;
    save_android_with_review_and_editing_time(
        app,
        raw_uri,
        display_name,
        edits,
        review,
        editing_time_ms,
    )
}

pub fn save_android_with_review_and_editing_time(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
    edits: calibraw_core::sidecar::EditState,
    review: calibraw_core::sidecar::PhotoReview,
    editing_time_ms: u64,
) -> Result<String, calibraw_core::sidecar::SidecarError> {
    let uses_ai_denoise = edits.exposure.ai_denoise_enabled;
    let uses_people = calibraw_core::sidecar::uses_people_analysis(&edits);
    let bytes = calibraw_core::sidecar::encode_with_review_and_editing_time(
        edits,
        review,
        editing_time_ms,
    )?;
    let path =
        create_raw_sidecar_cache(app).map_err(calibraw_core::sidecar::SidecarError::Platform)?;
    let result = calibraw_core::sidecar::write_synced(&path, &bytes).and_then(|()| {
        publish_raw_sidecar(app, &path, raw_uri, display_name)
            .map_err(calibraw_core::sidecar::SidecarError::Platform)
    });
    if let Err(error) = fs::remove_file(&path) {
        log::warn!(
            "could not remove Android sidecar cache {}: {error}",
            path.display()
        );
    }
    if result.is_ok() && !uses_ai_denoise {
        remove_unused_companion(
            app,
            raw_uri,
            display_name,
            calibraw_core::sidecar::PhotoCompanion::AiDenoise,
        );
    }
    if result.is_ok() && !uses_people {
        remove_unused_companion(
            app,
            raw_uri,
            display_name,
            calibraw_core::sidecar::PhotoCompanion::PeopleAnalysis,
        );
    }
    result
}
