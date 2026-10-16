pub mod pipeline {
    pub use calibraw_gpu::pipeline::*;
}

pub mod ai_denoise;
pub mod ai_masks;
#[cfg(target_os = "windows")]
mod directml_adapter;
pub mod execution_provider;
mod gpu_job_record;
mod model_artifact;
mod model_install;
mod model_runtime;
#[cfg(not(target_os = "android"))]
mod onnx_runtime_artifact;
pub mod remove;
#[cfg(target_os = "windows")]
pub use directml_adapter::set_directml_adapter;
pub use gpu_job_record::{clear_gpu_job_record, gpu_job_in_progress, set_gpu_job_record_path};
pub use model_install::ModelDownloadProgress;
pub use model_runtime::{set_warm_ai_features, AiFeature, AiFeatureSet};

#[cfg(not(target_os = "android"))]
pub use model_artifact::desktop_model_cache_root;
#[cfg(not(target_os = "android"))]
pub use onnx_runtime_artifact::{
    automatic_onnx_runtime_info, automatic_onnx_runtime_is_installed,
    automatic_onnx_runtime_kind_selectable, bundled_onnx_runtime_path,
    ensure_automatic_onnx_runtime, set_automatic_onnx_runtime_kind, AutomaticOnnxRuntimeInfo,
    AutomaticOnnxRuntimeKind,
};

#[cfg(not(target_os = "android"))]
pub use execution_provider::set_ai_acceleration_enabled;
pub use execution_provider::{
    active_execution_providers, ai_acceleration_enabled, create_session_with_fallback,
    take_ai_gpu_failure, AiGpuFailure, CpuFallbackProfile, ExecutionProviderStatus,
    FallbackSession, SessionOptions,
};
