//! Sidecars and AI results for content-URI documents.

use super::*;

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

/// Deletes the AI-denoise result once the saved edit no longer uses it. A
/// failure only leaves a stale file behind, so it is logged.
fn remove_unused_ai_denoise_result(app: &AndroidApp, raw_uri: &str, display_name: &str) {
    let path = match ai_denoise_result_path(app, raw_uri, display_name) {
        Ok(path) => path,
        Err(error) => {
            log::warn!("{error}");
            return;
        }
    };
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => log::warn!("could not remove {}: {error}", path.display()),
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
        let fingerprint_path = developed_thumbnail_fingerprint_path(&cache_path);
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
    with_storage_manager(app, |env, storage_manager| {
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
    .map_err(|error| format!("could not publish Android RAW sidecar: {error:#}"))
}

pub fn load_android(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
) -> Result<Option<calibraw_core::sidecar::LoadedSidecar>, calibraw_core::sidecar::SidecarError> {
    let Some(path) = materialize_raw_sidecar(app, raw_uri, display_name)
        .map_err(calibraw_core::sidecar::SidecarError::Platform)?
    else {
        return Ok(None);
    };
    let result = calibraw_core::sidecar::read_bounded(&path)
        .and_then(|bytes| calibraw_core::sidecar::decode(&bytes));
    if let Err(error) = fs::remove_file(&path) {
        log::warn!(
            "could not remove Android sidecar cache {}: {error}",
            path.display()
        );
    }
    result.map(Some)
}

fn load_android_sidecar_metadata(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
) -> Result<Option<calibraw_core::sidecar::SidecarMetadata>, calibraw_core::sidecar::SidecarError> {
    let Some(path) = materialize_raw_sidecar(app, raw_uri, display_name)
        .map_err(calibraw_core::sidecar::SidecarError::Platform)?
    else {
        return Ok(None);
    };
    let result = calibraw_core::sidecar::read_bounded(&path)
        .and_then(|bytes| calibraw_core::sidecar::decode_sidecar_metadata(&bytes));
    if let Err(error) = fs::remove_file(&path) {
        log::warn!(
            "could not remove Android sidecar cache {}: {error}",
            path.display()
        );
    }
    result.map(Some)
}

pub fn load_android_review(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
) -> Result<Option<calibraw_core::sidecar::PhotoReview>, calibraw_core::sidecar::SidecarError> {
    load_android_sidecar_metadata(app, raw_uri, display_name)
        .map(|metadata| metadata.map(|metadata| metadata.review))
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
        remove_unused_ai_denoise_result(app, raw_uri, display_name);
    }
    result
}
