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
pub(crate) use package::ensure_automatic_onnx_runtime;
#[cfg(not(target_os = "android"))]
pub use package::{
    ai_sessions_may_use_gpu, automatic_onnx_runtime_info, automatic_onnx_runtime_is_installed,
    automatic_onnx_runtime_kind_selectable, bundled_onnx_runtime_path, AutomaticOnnxRuntimeInfo,
};
#[cfg(not(target_os = "android"))]
pub use probe::{probe_runtime_subprocess, run_runtime_probe_process};

/// Whether new AI sessions may run on a GPU. Android's built-in runtime may
/// offer NNAPI, and GPU AI cannot be switched off there.
#[cfg(target_os = "android")]
pub fn ai_sessions_may_use_gpu(_manual_runtime: bool) -> bool {
    true
}

use serde::{Deserialize, Serialize};

/// Which automatic runtime to download. Only Linux x86_64 has a choice: its
/// CPU package has no GPU provider, and Microsoft's CUDA builds need the
/// matching CUDA toolkit and cuDNN 9 installed on the system. Serialized in
/// the performance settings; keep the names stable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomaticOnnxRuntimeKind {
    #[default]
    Cpu,
    Cuda12,
    Cuda13,
}

impl AutomaticOnnxRuntimeKind {
    pub const ALL: [Self; 3] = [Self::Cpu, Self::Cuda12, Self::Cuda13];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Cuda12 => "NVIDIA CUDA 12",
            Self::Cuda13 => "NVIDIA CUDA 13",
        }
    }
}
