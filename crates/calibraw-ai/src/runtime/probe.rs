//! Loading a user-selected ONNX Runtime in a child process first, so a
//! library that crashes on load ends the probe instead of CalibRaw.

use super::load::load;
use crate::config::{RuntimeSource, Sha256};
use anyhow::{Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

type RuntimeProbeResult = (PathBuf, Sha256);
static RUNTIME_PROBE_CACHE: OnceLock<Mutex<Option<RuntimeProbeResult>>> = OnceLock::new();

/// Loads `runtime_path` in a child process started with
/// `--calibraw-onnx-runtime-probe`, which calls `run_runtime_probe_process`.
/// A passing library is remembered for this process.
pub fn probe_runtime_subprocess(runtime_path: &Path, expected_sha256: &Sha256) -> Result<()> {
    let runtime_path = fs::canonicalize(runtime_path)
        .with_context(|| format!("resolve selected ONNX Runtime {}", runtime_path.display()))?;
    let cache = RUNTIME_PROBE_CACHE.get_or_init(|| Mutex::new(None));
    {
        let cached = cache
            .lock()
            .map_err(|_| anyhow::anyhow!("ONNX Runtime probe cache lock was poisoned"))?;
        if let Some((cached_path, cached_sha256)) = cached.as_ref() {
            if cached_path == &runtime_path && cached_sha256 == expected_sha256 {
                return Ok(());
            }
        }
    }

    let executable =
        std::env::current_exe().context("locate CalibRaw executable for ONNX Runtime probe")?;
    let status = std::process::Command::new(&executable)
        .arg("--calibraw-onnx-runtime-probe")
        .arg(&runtime_path)
        .arg(expected_sha256.as_str())
        .status()
        .with_context(|| {
            format!(
                "start isolated ONNX Runtime probe with {}",
                executable.display()
            )
        })?;
    if !status.success() {
        return Err(anyhow::anyhow!(
            "the selected ONNX Runtime failed CalibRaw's isolated compatibility probe ({status}). Use a matching native {} ONNX Runtime library",
            std::env::consts::ARCH
        ));
    }
    let mut cached = cache
        .lock()
        .map_err(|_| anyhow::anyhow!("ONNX Runtime probe cache lock was poisoned"))?;
    *cached = Some((runtime_path, expected_sha256.clone()));
    Ok(())
}

/// The child process's side of `probe_runtime_subprocess`.
pub fn run_runtime_probe_process(runtime_path: &Path, expected_sha256: &Sha256) -> Result<()> {
    load(&RuntimeSource::Manual {
        path: runtime_path.to_path_buf(),
        sha256: expected_sha256.clone(),
    })
}
