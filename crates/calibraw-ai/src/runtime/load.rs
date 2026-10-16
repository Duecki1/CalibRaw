//! Loading the ONNX Runtime library once per process. Desktop builds load a
//! library file pinned by its SHA-256; Android links the runtime into the app.

use crate::config::RuntimeSource;
use anyhow::Result;
use std::sync::{Mutex, OnceLock};
#[cfg(not(target_os = "android"))]
use {
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
    let (selected, expected_sha256) = match source {
        RuntimeSource::Manual { path, sha256 } => (path.clone(), sha256.clone()),
        RuntimeSource::Automatic => {
            super::ensure_automatic_onnx_runtime().context("prepare the automatic ONNX Runtime")?
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
            builder.with_name("CalibRaw").commit(),
            "ONNX Runtime was already initialized before the selected pinned library could be committed"
        );
        calibraw_core::diagnostics::record(format!(
            "ONNX Runtime library loaded in {:.3}s",
            started.elapsed().as_secs_f64()
        ));
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
            ort::init().with_name("CalibRaw").commit(),
            "ONNX Runtime was already initialized before CalibRaw could configure it"
        );
        RUNTIME_INITIALIZED
            .set(())
            .map_err(|_| anyhow::anyhow!("ONNX Runtime state was initialized concurrently"))?;
    }
    Ok(())
}
