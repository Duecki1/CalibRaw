//! Loading the ONNX Runtime library once per process. Desktop builds load a
//! library file pinned by its SHA-256, and register the automatic runtime's
//! WebGPU plugin execution provider beside it; Android links the runtime
//! into the app. Either way its log goes to `logger`.

use super::logger;
use crate::config::RuntimeSource;
use anyhow::Result;
use std::sync::{Mutex, OnceLock};
#[cfg(not(target_os = "android"))]
use {
    super::package::WebGpuPlugin,
    crate::config::Sha256,
    anyhow::Context,
    std::{fs, path::PathBuf},
};

#[cfg(not(target_os = "android"))]
static DESKTOP_RUNTIME_IDENTITY: OnceLock<(PathBuf, Sha256)> = OnceLock::new();
static RUNTIME_INITIALIZED: OnceLock<()> = OnceLock::new();
static RUNTIME_INIT_LOCK: Mutex<()> = Mutex::new(());

/// Loads the configured ONNX Runtime unless this process already has. A
/// different library than the loaded one fails until CalibRaw restarts.
pub fn initialize_runtime() -> Result<()> {
    load(&crate::config::runtime_source())
}

#[cfg(not(target_os = "android"))]
pub(super) fn load(source: &RuntimeSource) -> Result<()> {
    // A manual runtime runs only the providers built into it: a plugin beside
    // it would not be covered by the SHA-256 the user approved.
    let ((selected, expected_sha256), webgpu) = match source {
        RuntimeSource::Manual { path, sha256 } => ((path.clone(), sha256.clone()), None),
        RuntimeSource::Automatic => {
            let libraries = super::ensure_automatic_onnx_runtime()
                .context("prepare the automatic ONNX Runtime")?;
            (libraries.core, libraries.webgpu)
        }
    };
    let runtime_path = fs::canonicalize(&selected)
        .with_context(|| format!("resolve selected ONNX Runtime {}", selected.display()))?;
    let metadata = fs::metadata(&runtime_path)
        .with_context(|| format!("inspect selected ONNX Runtime {}", runtime_path.display()))?;
    anyhow::ensure!(
        metadata.is_file(),
        "selected ONNX Runtime is not a regular file"
    );
    anyhow::ensure!(
        (1_000_000..=1_000_000_000).contains(&metadata.len()),
        "selected ONNX Runtime has an implausible size of {} bytes",
        metadata.len()
    );
    let actual_sha256 =
        Sha256::of_file(&runtime_path).context("verify selected ONNX Runtime before loading")?;
    anyhow::ensure!(
        actual_sha256 == expected_sha256,
        "selected ONNX Runtime changed after approval: expected SHA-256 {expected_sha256}, found {actual_sha256}; select it again only if you trust the replacement"
    );
    if let Some((loaded_path, loaded_sha256)) = DESKTOP_RUNTIME_IDENTITY.get() {
        anyhow::ensure!(
            loaded_path == &runtime_path && loaded_sha256 == &actual_sha256,
            "a different ONNX Runtime is already active in this process; restart CalibRaw before changing the pinned runtime"
        );
    }
    let _init_guard = RUNTIME_INIT_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("ONNX Runtime initialization lock was poisoned"))?;
    if RUNTIME_INITIALIZED.get().is_none() {
        calibraw_core::diagnostics::record(format!(
            "ONNX Runtime library loading from {}",
            runtime_path.display()
        ));
        let started = std::time::Instant::now();
        let builder = ort::init_from(&runtime_path).map_err(|error| {
            anyhow::anyhow!(
                "could not load ONNX Runtime from {}: {error}",
                runtime_path.display()
            )
        })?;
        anyhow::ensure!(
            builder
                .with_name("CalibRaw")
                .with_logger(logger::logger())
                .commit(),
            "ONNX Runtime was already initialized before the selected pinned library could be committed"
        );
        set_log_level();
        calibraw_core::diagnostics::record(format!(
            "ONNX Runtime library loaded in {:.3}s",
            started.elapsed().as_secs_f64()
        ));
        if let Some(plugin) = &webgpu {
            register_webgpu_plugin(plugin);
        }
        DESKTOP_RUNTIME_IDENTITY
            .set((runtime_path.clone(), actual_sha256.clone()))
            .map_err(|_| anyhow::anyhow!("ONNX Runtime identity was initialized concurrently"))?;
        RUNTIME_INITIALIZED
            .set(())
            .map_err(|_| anyhow::anyhow!("ONNX Runtime state was initialized concurrently"))?;
    }
    let (loaded_path, loaded_sha256) = DESKTOP_RUNTIME_IDENTITY
        .get()
        .context("ONNX Runtime initialized without a pinned desktop identity")?;
    anyhow::ensure!(
        loaded_path == &runtime_path && loaded_sha256 == &actual_sha256,
        "a different ONNX Runtime is already active in this process; restart CalibRaw before changing the pinned runtime"
    );
    Ok(())
}

/// Registers the WebGPU plugin execution provider with the loaded runtime.
/// A plugin that fails to register leaves sessions on the CPU; the reason is
/// recorded in diagnostics. Registration lasts as long as the process.
#[cfg(not(target_os = "android"))]
fn register_webgpu_plugin(plugin: &WebGpuPlugin) {
    let result = (|| -> Result<()> {
        // Checked again right before loading, as the core library is.
        let sha256 = Sha256::of_file(&plugin.path).context("verify the WebGPU provider")?;
        anyhow::ensure!(
            sha256 == plugin.sha256,
            "the WebGPU provider changed after it was installed: expected SHA-256 {}, found {sha256}",
            plugin.sha256
        );
        #[cfg(target_os = "windows")]
        preload_plugin_imports();
        let environment = ort::environment::current()
            .map_err(|error| anyhow::anyhow!("get the ONNX Runtime environment: {error}"))?;
        environment
            .register_ep_library("CalibRaw WebGPU", &plugin.path)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        Ok(())
    })();
    calibraw_core::diagnostics::record(match result {
        Ok(()) => format!(
            "ONNX Runtime WebGPU provider {} registered from {}",
            plugin.version,
            plugin.path.display()
        ),
        Err(error) => {
            log::warn!("could not register the WebGPU provider: {error:#}");
            format!(
                "ONNX Runtime WebGPU provider {} not registered, AI runs on the CPU: {error:#}",
                plugin.version
            )
        }
    });
}

/// The Visual C++ runtime libraries the WebGPU plugin imports and the core
/// library does not. `packaging/onnxruntime/package_webgpu.py` reads this
/// list to check the Windows archives.
#[cfg(target_os = "windows")]
const PRELOADED_PLUGIN_IMPORTS: &[&str] = &["msvcp140_atomic_wait.dll"];

/// Loads `PRELOADED_PLUGIN_IMPORTS` from CalibRaw's folder, where the Windows
/// build ships them (`vc_runtime_dlls` in `build-windows.yml`). ONNX Runtime
/// loads a plugin with `LOAD_WITH_ALTERED_SEARCH_PATH`, which searches the
/// plugin's folder and System32 but not CalibRaw's, so without them the
/// plugin needs a recent Visual C++ redistributable installed. Windows
/// resolves an import to a loaded library of the same name first. Where a
/// file is missing, as in a development build, System32's copy is left to
/// serve.
#[cfg(target_os = "windows")]
fn preload_plugin_imports() {
    let Some(folder) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
    else {
        return;
    };
    for name in PRELOADED_PLUGIN_IMPORTS {
        let path = folder.join(name);
        if !path.is_file() {
            continue;
        }
        if let Err(error) = ort::util::preload_dylib(&path) {
            log::warn!("could not load {}: {error}", path.display());
            calibraw_core::diagnostics::record(format!(
                "could not load {} for the WebGPU provider: {error}",
                path.display()
            ));
        }
    }
}

/// Lowers ONNX Runtime's log level from the verbose one an environment with
/// a custom logger starts at (`logger::level`). An environment that cannot
/// be created now fails again, with its error, where a session needs it.
fn set_log_level() {
    match ort::environment::current() {
        Ok(environment) => environment.set_log_level(logger::level()),
        Err(error) => log::warn!("could not set the ONNX Runtime log level: {error}"),
    }
}

/// Whether this process has already loaded an ONNX Runtime library. The
/// library stays loaded until the process exits, so choosing a different one
/// only takes effect after a restart.
#[cfg(not(target_os = "android"))]
pub fn runtime_loaded_in_process() -> bool {
    DESKTOP_RUNTIME_IDENTITY.get().is_some()
}

#[cfg(target_os = "android")]
fn load(_source: &RuntimeSource) -> Result<()> {
    if RUNTIME_INITIALIZED.get().is_some() {
        return Ok(());
    }
    let _init_guard = RUNTIME_INIT_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("ONNX Runtime initialization lock was poisoned"))?;
    if RUNTIME_INITIALIZED.get().is_none() {
        anyhow::ensure!(
            ort::init()
                .with_name("CalibRaw")
                .with_logger(logger::logger())
                .commit(),
            "ONNX Runtime was already initialized before CalibRaw could configure it"
        );
        set_log_level();
        RUNTIME_INITIALIZED
            .set(())
            .map_err(|_| anyhow::anyhow!("ONNX Runtime state was initialized concurrently"))?;
    }
    Ok(())
}
