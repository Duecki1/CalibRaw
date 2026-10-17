//! The ONNX Runtime library: loading it into this process (`load`), its log
//! (`logger`), checking a user-selected one in a separate process first
//! (`probe`), and the automatic runtime packages (`package`).

mod load;
mod logger;
#[cfg(not(target_os = "android"))]
mod package;
#[cfg(not(target_os = "android"))]
mod probe;

#[cfg(target_os = "android")]
pub use builtin::{
    automatic_onnx_runtime_info, automatic_onnx_runtime_is_installed, bundled_onnx_runtime_path,
    probe_runtime_subprocess, retired_runtime_paths, run_runtime_probe_process,
    runtime_loaded_in_process,
};
pub use load::initialize_runtime;
#[cfg(not(target_os = "android"))]
pub use load::runtime_loaded_in_process;
#[cfg(not(target_os = "android"))]
pub use package::{
    ai_sessions_may_use_gpu, automatic_onnx_runtime_info, automatic_onnx_runtime_is_installed,
    bundled_onnx_runtime_path, retired_runtime_paths,
};
#[cfg(not(target_os = "android"))]
pub(crate) use package::{automatic_runtime_lacks_gpu_build, ensure_automatic_onnx_runtime};
#[cfg(not(target_os = "android"))]
pub use probe::{probe_runtime_subprocess, run_runtime_probe_process};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutomaticOnnxRuntimeInfo {
    pub platform: &'static str,
    pub version: &'static str,
    pub download_bytes: u64,
    /// `None` for a bundled runtime.
    pub download_url: Option<&'static str>,
}

/// Android links its ONNX Runtime into the app: there is no runtime to
/// download, select, probe or retire. These answer the desktop runtime API
/// so callers need no platform check.
#[cfg(target_os = "android")]
mod builtin {
    use super::AutomaticOnnxRuntimeInfo;
    use crate::config::Sha256;
    use anyhow::{bail, Result};
    use std::path::{Path, PathBuf};

    pub fn runtime_loaded_in_process() -> bool {
        false
    }

    pub fn automatic_onnx_runtime_info() -> Option<AutomaticOnnxRuntimeInfo> {
        None
    }

    pub fn automatic_onnx_runtime_is_installed() -> bool {
        true
    }

    pub fn bundled_onnx_runtime_path() -> Option<&'static Path> {
        None
    }

    pub fn retired_runtime_paths(_model_root: &Path) -> impl Iterator<Item = PathBuf> + '_ {
        std::iter::empty()
    }

    pub fn probe_runtime_subprocess(_runtime_path: &Path, _expected_sha256: &Sha256) -> Result<()> {
        bail!("a manually selected ONNX Runtime is not supported on Android")
    }

    pub fn run_runtime_probe_process(
        _runtime_path: &Path,
        _expected_sha256: &Sha256,
    ) -> Result<()> {
        bail!("a manually selected ONNX Runtime is not supported on Android")
    }
}

/// Whether new AI sessions may run on a GPU: never on Android, whose
/// built-in runtime offers no GPU provider.
#[cfg(target_os = "android")]
pub fn ai_sessions_may_use_gpu(_manual_runtime: bool) -> bool {
    false
}

/// Android's built-in runtime has no GPU provider (docs/GPU_REWORK.md P6.1).
#[cfg(target_os = "android")]
pub(crate) fn automatic_runtime_lacks_gpu_build() -> bool {
    true
}
