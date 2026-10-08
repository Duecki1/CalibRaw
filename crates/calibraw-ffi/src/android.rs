mod replay;
pub use replay::ReplayVideoEncoder;

use android_activity::AndroidApp;
use jni::{
    errors::LogContextErrorAndDefault,
    objects::{JClass, JObject, JString},
    refs::Global,
    EnvUnowned, JValue, JavaVM,
};
use std::{
    collections::{HashMap, VecDeque},
    fs::{self, File},
    os::fd::FromRawFd,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicI32, Ordering},
        Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

mod callbacks;
mod export;
mod library;
mod notifications;
mod sidecars;
mod thumbnail_cache;
pub use callbacks::*;
pub use export::*;
pub use library::*;
pub use notifications::*;
pub use sidecars::*;
pub use thumbnail_cache::*;

#[derive(Debug)]
pub struct PickedDocument {
    pub path: PathBuf,
    pub display_name: String,
    pub library_uri: String,
    pub delete_after_decode: bool,
    pub raw_fd_guard: Option<File>,
}

#[derive(Clone, Debug)]
pub struct LibraryDocument {
    pub uri: String,
    pub display_name: String,
    pub display_path: String,
    pub bytes: u64,
    pub modified_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryFolder {
    pub path: String,
    pub name: String,
}

#[derive(Debug)]
pub enum PickerResult {
    Picked(PickedDocument),
    BatchImported {
        imported: usize,
        failed: usize,
        errors: String,
    },
    Cancelled,
    Failed(String),
}

/// A finished export in MediaStore.
#[derive(Debug)]
pub struct PublishedExport {
    /// Human-readable folder and name, for notices.
    pub location: String,
    /// Content URI other apps can be granted, or empty when Android did not report one.
    pub uri: String,
}

#[derive(Debug)]
pub enum ExportPublishResult {
    Published(PublishedExport),
    Failed(String),
}

#[derive(Debug)]
pub enum CameraProfileFolderResult {
    ImportStarted {
        label: String,
    },
    Picked {
        path: PathBuf,
        label: String,
        profiles: usize,
    },
    Cancelled,
    Failed(String),
}

static RESULTS: OnceLock<Mutex<VecDeque<PickerResult>>> = OnceLock::new();
/// Photos other apps sent ("Open with", Share). Kept apart from `RESULTS`,
/// whose entries answer an open the UI requested.
static EXTERNAL_OPEN_RESULTS: OnceLock<Mutex<VecDeque<PickerResult>>> = OnceLock::new();
static CAMERA_PROFILE_FOLDER_RESULTS: OnceLock<Mutex<VecDeque<CameraProfileFolderResult>>> =
    OnceLock::new();
static EXPORT_RESULTS: OnceLock<Mutex<VecDeque<ExportPublishResult>>> = OnceLock::new();
static DIRECT_EXPORTS: OnceLock<Mutex<HashMap<PathBuf, DirectExportTarget>>> = OnceLock::new();
/// Wakes the UI thread after a Java callback queued a result.
type RepaintNotifier = std::sync::Arc<dyn Fn() + Send + Sync>;
static REPAINT_NOTIFIER: Mutex<Option<RepaintNotifier>> = Mutex::new(None);
static BACK_NAVIGATION_ACTIVE: AtomicBool = AtomicBool::new(false);
static BACK_REQUESTED: AtomicBool = AtomicBool::new(false);
static SYSTEM_INSET_LEFT_PX: AtomicI32 = AtomicI32::new(0);
static SYSTEM_INSET_TOP_PX: AtomicI32 = AtomicI32::new(0);
static SYSTEM_INSET_RIGHT_PX: AtomicI32 = AtomicI32::new(0);
static SYSTEM_INSET_BOTTOM_PX: AtomicI32 = AtomicI32::new(0);

fn results() -> &'static Mutex<VecDeque<PickerResult>> {
    RESULTS.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn external_open_results() -> &'static Mutex<VecDeque<PickerResult>> {
    EXTERNAL_OPEN_RESULTS.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn camera_profile_folder_results() -> &'static Mutex<VecDeque<CameraProfileFolderResult>> {
    CAMERA_PROFILE_FOLDER_RESULTS.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn export_results() -> &'static Mutex<VecDeque<ExportPublishResult>> {
    EXPORT_RESULTS.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn direct_exports() -> &'static Mutex<HashMap<PathBuf, DirectExportTarget>> {
    DIRECT_EXPORTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn take_queued<T>(queue: &'static Mutex<VecDeque<T>>) -> Option<T> {
    queue.lock().ok()?.pop_front()
}

fn request_repaint() {
    // Call outside the lock so a notifier can never deadlock against attach.
    let notifier = REPAINT_NOTIFIER
        .lock()
        .ok()
        .and_then(|installed| installed.clone());
    if let Some(notify) = notifier {
        notify();
    }
}

/// Connects the running UI. Java callbacks queue their results and then call
/// `request_repaint`, so the UI thread polls them on its next frame. Clears
/// task-notification state left by a previous activity.
pub fn attach_ui(request_repaint: impl Fn() + Send + Sync + 'static) {
    if let Ok(mut installed) = REPAINT_NOTIFIER.lock() {
        *installed = Some(std::sync::Arc::new(request_repaint));
    }
    if let Ok(mut notification) = TASK_NOTIFICATION_STATE.lock() {
        *notification = None;
    }
}

/// Disconnects the UI before its activity goes away. Later callbacks still
/// queue results but wake nothing, and back navigation returns to the system.
pub fn detach_ui() {
    if let Ok(mut installed) = REPAINT_NOTIFIER.lock() {
        *installed = None;
    }
    BACK_NAVIGATION_ACTIVE.store(false, Ordering::Release);
    BACK_REQUESTED.store(false, Ordering::Release);
}

pub fn set_back_navigation_active(active: bool) {
    BACK_NAVIGATION_ACTIVE.store(active, Ordering::Release);
}

pub fn take_back_request() -> bool {
    BACK_REQUESTED.swap(false, Ordering::AcqRel)
}

pub fn system_bar_insets_points(pixels_per_point: f32) -> [f32; 4] {
    let scale = pixels_per_point.max(0.5);
    let to_points = |value: i32| value.max(0) as f32 / scale;
    [
        to_points(SYSTEM_INSET_LEFT_PX.load(Ordering::Acquire)),
        to_points(SYSTEM_INSET_TOP_PX.load(Ordering::Acquire)),
        to_points(SYSTEM_INSET_RIGHT_PX.load(Ordering::Acquire)),
        to_points(SYSTEM_INSET_BOTTOM_PX.load(Ordering::Acquire)),
    ]
}

pub fn take_picker_result() -> Option<PickerResult> {
    take_queued(results())
}

/// Next photo another app sent. Never `Cancelled`.
pub fn take_external_open_result() -> Option<PickerResult> {
    take_queued(external_open_results())
}

pub fn has_external_open_result() -> bool {
    external_open_results()
        .lock()
        .is_ok_and(|queue| !queue.is_empty())
}

pub fn take_camera_profile_folder_result() -> Option<CameraProfileFolderResult> {
    take_queued(camera_profile_folder_results())
}

pub fn take_export_publish_result() -> Option<ExportPublishResult> {
    take_queued(export_results())
}

pub fn set_light_system_bars(app: &AndroidApp, light: bool) -> Result<(), String> {
    with_activity(app, |env, activity| {
        env.call_method(
            activity,
            jni::jni_str!("setLightSystemBars"),
            jni::jni_sig!((i32) -> void),
            &[JValue::Int(i32::from(light))],
        )?;
        Ok(())
    })
    .map_err(|error| format!("could not update Android system-bar appearance: {error:#}"))
}

pub fn open_camera_profile_folder(app: &AndroidApp) -> Result<(), String> {
    with_activity(app, |env, activity| {
        env.call_method(
            activity,
            jni::jni_str!("openCameraProfileFolder"),
            jni::jni_sig!(() -> void),
            &[],
        )?;
        Ok(())
    })
    .map_err(|error| format!("could not open Android's camera-profile folder picker: {error:#}"))
}

pub fn remove_camera_profile_mirror(app: &AndroidApp, path: &Path) -> Result<(), String> {
    let path = path.to_string_lossy().into_owned();
    with_profile_importer(app, |env, profile_importer| {
        let path = env.new_string(&path)?;
        env.call_method(
            profile_importer,
            jni::jni_str!("removeCameraProfileMirror"),
            jni::jni_sig!((JString) -> void),
            &[JValue::Object(&path)],
        )?;
        Ok(())
    })
    .map_err(|error| format!("could not schedule Android camera-profile cleanup: {error:#}"))
}

pub fn scavenge_camera_profile_mirrors(
    app: &AndroidApp,
    active_mirror: Option<&Path>,
) -> Result<(), String> {
    let active_mirror = active_mirror
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    with_activity(app, |env, activity| {
        let active_mirror = env.new_string(&active_mirror)?;
        env.call_method(
            activity,
            jni::jni_str!("scavengeCameraProfileMirrors"),
            jni::jni_sig!((JString) -> void),
            &[JValue::Object(&active_mirror)],
        )?;
        Ok(())
    })
    .map_err(|error| format!("could not schedule Android camera-profile scavenging: {error:#}"))
}

pub fn clear_camera_profile_folder_picker_location(app: &AndroidApp) -> Result<(), String> {
    with_profile_importer(app, |env, profile_importer| {
        env.call_method(
            profile_importer,
            jni::jni_str!("clearFolderPickerLocation"),
            jni::jni_sig!(() -> void),
            &[],
        )?;
        Ok(())
    })
    .map_err(|error| format!("could not clear Android's camera-profile picker location: {error:#}"))
}

pub fn open_raw_document(app: &AndroidApp) -> Result<(), String> {
    with_activity(app, |env, activity| {
        env.call_method(
            activity,
            jni::jni_str!("openRawDocument"),
            jni::jni_sig!(() -> void),
            &[],
        )?;
        Ok(())
    })
    .map_err(|error| format!("could not open Android's file picker: {error:#}"))
}

pub fn device_diagnostics(app: &AndroidApp) -> Result<String, String> {
    with_activity(app, |env, activity| {
        let value = env.call_method(
            activity,
            jni::jni_str!("deviceDiagnostics"),
            jni::jni_sig!(() -> JString),
            &[],
        )?;
        java_string(env, value)
    })
    .map_err(|error| format!("could not read Android device diagnostics: {error:#}"))
}

pub fn copy_text_to_clipboard(app: &AndroidApp, label: &str, text: &str) -> Result<(), String> {
    with_activity(app, |env, activity| {
        let label = env.new_string(label)?;
        let text = env.new_string(text)?;
        env.call_method(
            activity,
            jni::jni_str!("copyTextToClipboard"),
            jni::jni_sig!((JString, JString) -> void),
            &[JValue::Object(&label), JValue::Object(&text)],
        )?;
        Ok(())
    })
    .map_err(|error| format!("could not copy diagnostics to Android clipboard: {error:#}"))
}

pub fn performance_settings_path(app: &AndroidApp) -> Result<PathBuf, String> {
    let path = with_activity(app, |env, activity| {
        let value = env.call_method(
            activity,
            jni::jni_str!("performanceSettingsPath"),
            jni::jni_sig!(() -> JString),
            &[],
        )?;
        java_string(env, value)
    })
    .map_err(|error| format!("could not locate Android performance settings: {error:#}"))?;
    non_empty_path(path, "Android returned no performance settings path")
}

pub fn gpu_pipeline_cache_dir(app: &AndroidApp) -> Result<PathBuf, String> {
    let path = with_activity(app, |env, activity| {
        let value = env.call_method(
            activity,
            jni::jni_str!("gpuPipelineCacheDir"),
            jni::jni_sig!(() -> JString),
            &[],
        )?;
        java_string(env, value)
    })
    .map_err(|error| format!("could not locate Android GPU pipeline cache: {error:#}"))?;
    non_empty_path(path, "Android returned no GPU pipeline cache directory")
}

pub fn lensfun_database_dir(app: &AndroidApp) -> Result<PathBuf, String> {
    let path = with_activity(app, |env, activity| {
        let value = env.call_method(
            activity,
            jni::jni_str!("lensfunDatabaseDir"),
            jni::jni_sig!(() -> JString),
            &[],
        )?;
        java_string(env, value)
    })
    .map_err(|error| format!("could not materialize bundled Lensfun database: {error:#}"))?;
    non_empty_path(path, "Android returned no Lensfun database directory")
}

/// The `String` a Java method returned; `null` is an error.
fn java_string<'local>(
    env: &mut jni::Env<'local>,
    value: jni::JValueOwned<'local>,
) -> jni::errors::Result<String> {
    let object = value.l()?;
    Ok(env.cast_local::<JString>(object)?.to_string())
}

/// Android reports a missing directory or file as an empty path.
fn non_empty_path(path: String, missing: &str) -> Result<PathBuf, String> {
    if path.is_empty() {
        Err(missing.to_owned())
    } else {
        Ok(PathBuf::from(path))
    }
}

fn with_activity<T>(
    app: &AndroidApp,
    operation: impl FnOnce(&mut jni::Env<'_>, &JObject) -> jni::errors::Result<T>,
) -> jni::errors::Result<T> {
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env| {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&raw_activity)? };
        operation(env, activity.as_ref())
    })
}

fn with_storage_manager<T>(
    app: &AndroidApp,
    operation: impl FnOnce(&mut jni::Env<'_>, &JObject) -> jni::errors::Result<T>,
) -> jni::errors::Result<T> {
    with_activity(app, |env, activity| {
        let storage_manager = env
            .get_field(
                activity,
                jni::jni_str!("storageManager"),
                jni::jni_sig!(de.duecki.calibraw.StorageManager),
            )?
            .l()?;
        operation(env, &storage_manager)
    })
}

fn with_profile_importer<T>(
    app: &AndroidApp,
    operation: impl FnOnce(&mut jni::Env<'_>, &JObject) -> jni::errors::Result<T>,
) -> jni::errors::Result<T> {
    with_activity(app, |env, activity| {
        let profile_importer = env
            .get_field(
                activity,
                jni::jni_str!("profileImporter"),
                jni::jni_sig!(de.duecki.calibraw.ProfileImporter),
            )?
            .l()?;
        operation(env, &profile_importer)
    })
}

fn with_export_publisher<T>(
    app: &AndroidApp,
    operation: impl FnOnce(&mut jni::Env<'_>, &JObject) -> jni::errors::Result<T>,
) -> jni::errors::Result<T> {
    with_activity(app, |env, activity| {
        let export_publisher = env
            .get_field(
                activity,
                jni::jni_str!("exportPublisher"),
                jni::jni_sig!(de.duecki.calibraw.ExportPublisher),
            )?
            .l()?;
        operation(env, &export_publisher)
    })
}

fn decode_uri_component(encoded: &str) -> Result<String, String> {
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return Err("truncated percent escape in Android library record".to_owned());
        }
        let high = hex_digit(bytes[index + 1])?;
        let low = hex_digit(bytes[index + 2])?;
        decoded.push((high << 4) | low);
        index += 3;
    }
    String::from_utf8(decoded)
        .map_err(|error| format!("Android library record is not valid UTF-8: {error}"))
}

fn hex_digit(value: u8) -> Result<u8, String> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err("invalid percent escape in Android library record".to_owned()),
    }
}
