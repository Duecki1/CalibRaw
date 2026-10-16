//! The ONNX Runtime library: loading it into this process (`load`), checking
//! a user-selected one in a separate process first (`probe`), and the
//! automatic runtime packages (`package`).

mod load;
#[cfg(not(target_os = "android"))]
mod package;
#[cfg(not(target_os = "android"))]
mod probe;

pub use load::initialize_runtime;
#[cfg(not(target_os = "android"))]
pub use load::runtime_loaded_in_process;
#[cfg(not(target_os = "android"))]
pub use package::{
    ai_sessions_may_use_gpu, automatic_onnx_runtime_info, automatic_onnx_runtime_is_installed,
    bundled_onnx_runtime_path, retired_runtime_paths, AutomaticOnnxRuntimeInfo,
};
#[cfg(not(target_os = "android"))]
pub(crate) use package::{automatic_runtime_lacks_gpu_build, ensure_automatic_onnx_runtime};
#[cfg(not(target_os = "android"))]
pub use probe::{probe_runtime_subprocess, run_runtime_probe_process};

/// Whether new AI sessions may run on a GPU. Android's built-in runtime may
/// offer NNAPI, and GPU AI cannot be switched off there.
#[cfg(target_os = "android")]
pub fn ai_sessions_may_use_gpu(_manual_runtime: bool) -> bool {
    true
}

/// Android's built-in runtime offers NNAPI and XNNPACK.
#[cfg(target_os = "android")]
pub(crate) fn automatic_runtime_lacks_gpu_build() -> bool {
    false
}
