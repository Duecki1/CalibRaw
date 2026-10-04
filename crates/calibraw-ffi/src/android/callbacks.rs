//! JNI entry points called by `CalibRawActivity`. They only record results
//! and request a repaint; the UI thread consumes them.

use super::*;

#[unsafe(no_mangle)]
pub extern "system" fn Java_de_duecki_calibraw_CalibRawActivity_nativeOnBackRequested<'local>(
    _unowned_env: EnvUnowned<'local>,
    _class: JClass<'local>,
) -> jni::sys::jboolean {
    if !BACK_NAVIGATION_ACTIVE.load(Ordering::Acquire) {
        return false;
    }
    BACK_REQUESTED.store(true, Ordering::Release);
    request_repaint();
    true
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_de_duecki_calibraw_CalibRawActivity_nativeOnSystemInsetsChanged<
    'local,
>(
    _unowned_env: EnvUnowned<'local>,
    _class: JClass<'local>,
    left: jni::sys::jint,
    top: jni::sys::jint,
    right: jni::sys::jint,
    bottom: jni::sys::jint,
) {
    SYSTEM_INSET_LEFT_PX.store(left.max(0), Ordering::Release);
    SYSTEM_INSET_TOP_PX.store(top.max(0), Ordering::Release);
    SYSTEM_INSET_RIGHT_PX.store(right.max(0), Ordering::Release);
    SYSTEM_INSET_BOTTOM_PX.store(bottom.max(0), Ordering::Release);
    request_repaint();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_de_duecki_calibraw_CalibRawActivity_nativeOnFilePicked<'local>(
    mut unowned_env: EnvUnowned<'local>,
    _class: JClass<'local>,
    path: JString<'local>,
    display_name: JString<'local>,
    library_uri: JString<'local>,
    error: JString<'local>,
    temporary: jni::sys::jboolean,
) {
    unowned_env
        .with_env(|_env| -> jni::errors::Result<()> {
            let path = path.to_string();
            let display_name = display_name.to_string();
            let library_uri = library_uri.to_string();
            let error = error.to_string();

            let result = if !error.is_empty() {
                PickerResult::Failed(error)
            } else if path.is_empty() {
                PickerResult::Cancelled
            } else {
                PickerResult::Picked(PickedDocument {
                    path: PathBuf::from(path),
                    display_name,
                    library_uri,
                    delete_after_decode: temporary,
                    raw_fd_guard: None,
                })
            };

            if let Ok(mut queue) = results().lock() {
                queue.push_back(result);
            }
            request_repaint();
            Ok(())
        })
        .resolve_with::<LogContextErrorAndDefault, _>(|| {
            "CalibRawActivity.nativeOnFilePicked".to_owned()
        });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_de_duecki_calibraw_CalibRawActivity_nativeOnFilePickedFd<'local>(
    mut unowned_env: EnvUnowned<'local>,
    _class: JClass<'local>,
    fd: jni::sys::jint,
    display_name: JString<'local>,
    library_uri: JString<'local>,
    error: JString<'local>,
) {
    unowned_env
        .with_env(|_env| -> jni::errors::Result<()> {
            let display_name = display_name.to_string();
            let library_uri = library_uri.to_string();
            let error = error.to_string();

            let result = if !error.is_empty() {
                if fd >= 0 {
                    drop(unsafe { File::from_raw_fd(fd) });
                }
                PickerResult::Failed(error)
            } else if fd < 0 {
                PickerResult::Failed("Android returned an invalid RAW file descriptor".to_owned())
            } else {
                let guard = unsafe { File::from_raw_fd(fd) };
                PickerResult::Picked(PickedDocument {
                    path: PathBuf::from(format!("/proc/self/fd/{fd}")),
                    display_name,
                    library_uri,
                    delete_after_decode: false,
                    raw_fd_guard: Some(guard),
                })
            };

            if let Ok(mut queue) = results().lock() {
                queue.push_back(result);
            }
            request_repaint();
            Ok(())
        })
        .resolve_with::<LogContextErrorAndDefault, _>(|| {
            "CalibRawActivity.nativeOnFilePickedFd".to_owned()
        });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_de_duecki_calibraw_CalibRawActivity_nativeOnCameraProfileFolderImportStarted<
    'local,
>(
    mut unowned_env: EnvUnowned<'local>,
    _class: JClass<'local>,
    label: JString<'local>,
) {
    unowned_env
        .with_env(|_env| -> jni::errors::Result<()> {
            let label = label.to_string();
            if let Ok(mut queue) = camera_profile_folder_results().lock() {
                queue.push_back(CameraProfileFolderResult::ImportStarted { label });
            }
            request_repaint();
            Ok(())
        })
        .resolve_with::<LogContextErrorAndDefault, _>(|| {
            "CalibRawActivity.nativeOnCameraProfileFolderImportStarted".to_owned()
        });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_de_duecki_calibraw_CalibRawActivity_nativeOnCameraProfileFolderPicked<
    'local,
>(
    mut unowned_env: EnvUnowned<'local>,
    _class: JClass<'local>,
    path: JString<'local>,
    label: JString<'local>,
    profile_count: jni::sys::jint,
    error: JString<'local>,
) {
    unowned_env
        .with_env(|_env| -> jni::errors::Result<()> {
            let path = path.to_string();
            let label = label.to_string();
            let error = error.to_string();
            let result = if !error.is_empty() {
                CameraProfileFolderResult::Failed(error)
            } else if path.is_empty() {
                CameraProfileFolderResult::Cancelled
            } else {
                CameraProfileFolderResult::Picked {
                    path: PathBuf::from(path),
                    label,
                    profiles: usize::try_from(profile_count.max(0)).unwrap_or(0),
                }
            };
            if let Ok(mut queue) = camera_profile_folder_results().lock() {
                queue.push_back(result);
            }
            request_repaint();
            Ok(())
        })
        .resolve_with::<LogContextErrorAndDefault, _>(|| {
            "CalibRawActivity.nativeOnCameraProfileFolderPicked".to_owned()
        });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_de_duecki_calibraw_CalibRawActivity_nativeOnImportBatchFinished<
    'local,
>(
    mut unowned_env: EnvUnowned<'local>,
    _class: JClass<'local>,
    imported_count: jni::sys::jint,
    failed_count: jni::sys::jint,
    errors: JString<'local>,
) {
    unowned_env
        .with_env(|_env| -> jni::errors::Result<()> {
            let result = PickerResult::BatchImported {
                imported: usize::try_from(imported_count.max(0)).unwrap_or(0),
                failed: usize::try_from(failed_count.max(0)).unwrap_or(0),
                errors: errors.to_string(),
            };
            if let Ok(mut queue) = results().lock() {
                queue.push_back(result);
            }
            request_repaint();
            Ok(())
        })
        .resolve_with::<LogContextErrorAndDefault, _>(|| {
            "CalibRawActivity.nativeOnImportBatchFinished".to_owned()
        });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_de_duecki_calibraw_CalibRawActivity_nativeOnExportPublished<'local>(
    mut unowned_env: EnvUnowned<'local>,
    _class: JClass<'local>,
    location: JString<'local>,
    error: JString<'local>,
) {
    unowned_env
        .with_env(|_env| -> jni::errors::Result<()> {
            let location = location.to_string();
            let error = error.to_string();
            let result = if error.is_empty() {
                ExportPublishResult::Published(location)
            } else {
                ExportPublishResult::Failed(error)
            };
            if let Ok(mut queue) = export_results().lock() {
                queue.push_back(result);
            }
            request_repaint();
            Ok(())
        })
        .resolve_with::<LogContextErrorAndDefault, _>(|| {
            "CalibRawActivity.nativeOnExportPublished".to_owned()
        });
}
