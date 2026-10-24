//! The ONNX Runtime library: loading it into this process (`load`), its log
//! (`logger`), checking a user-selected one in a separate process first
//! (`probe`), and the automatic runtime packages (`package`).

mod load;
mod logger;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod package;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod probe;

#[cfg(any(target_os = "android", target_os = "ios"))]
pub use builtin::{
    automatic_onnx_runtime_info, automatic_onnx_runtime_is_installed, bundled_onnx_runtime_path,
    probe_runtime_subprocess, retired_runtime_paths, run_runtime_probe_process,
    runtime_loaded_in_process,
};
pub use load::initialize_runtime;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use load::runtime_loaded_in_process;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use package::{
    ai_sessions_may_use_gpu, automatic_onnx_runtime_info, automatic_onnx_runtime_is_installed,
    bundled_onnx_runtime_path, retired_runtime_paths,
};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) use package::{automatic_runtime_lacks_gpu_build, ensure_automatic_onnx_runtime};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use probe::{probe_runtime_subprocess, run_runtime_probe_process};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutomaticOnnxRuntimeInfo {
    pub platform: &'static str,
    pub version: &'static str,
    pub download_bytes: u64,
    /// `None` for a bundled runtime.
    pub download_url: Option<&'static str>,
}

/// Mobile builds link ONNX Runtime into the app: there is no runtime to
/// download, select, probe or retire. These answer the desktop runtime API
/// so callers need no platform check.
#[cfg(any(target_os = "android", target_os = "ios"))]
mod builtin {
    use super::AutomaticOnnxRuntimeInfo;
    use crate::config::Sha256;
    use anyhow::{bail, Result};
    use std::path::{Path, PathBuf};

    const PLATFORM_NAME: &str = if cfg!(target_os = "android") {
        "Android"
    } else {
        "iOS"
    };

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
        bail!("a manually selected ONNX Runtime is not supported on {PLATFORM_NAME}")
    }

    pub fn run_runtime_probe_process(
        _runtime_path: &Path,
        _expected_sha256: &Sha256,
    ) -> Result<()> {
        bail!("a manually selected ONNX Runtime is not supported on {PLATFORM_NAME}")
    }
}

/// Mobile sessions use the constrained CPU configuration. iOS CoreML
/// acceleration is not enabled until its numerical and fallback paths are tested.
#[cfg(any(target_os = "android", target_os = "ios"))]
pub fn ai_sessions_may_use_gpu(_manual_runtime: bool) -> bool {
    false
}

/// The built-in mobile runtime is not used with the desktop GPU pipeline.
#[cfg(any(target_os = "android", target_os = "ios"))]
pub(crate) fn automatic_runtime_lacks_gpu_build() -> bool {
    true
}
