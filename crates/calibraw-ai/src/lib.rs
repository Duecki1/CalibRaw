pub mod pipeline {
    pub use calibraw_gpu::pipeline::*;
}

mod config;
mod jobs;
pub mod models;
mod placement;
mod registry;
pub mod runtime;
mod slot;

#[cfg(not(target_os = "android"))]
pub use config::desktop_model_root;
pub use config::{configure, update, AiConfig, AiConfigChange, GpuIdentity, RuntimeSource, Sha256};
pub use jobs::AiJobEvent;
pub use placement::{
    ai_acceleration_enabled, ai_session_statuses, end_gpu_job_records, gpu_ai_activity,
    gpu_job_in_progress, take_ai_gpu_device_loss, AiSessionStatus, CpuReason, GpuAiActivity,
    GpuProvider, Placement,
};
pub use registry::install::ModelDownloadProgress;
pub use registry::{AiFeature, InputShape, ModelInfo};
pub use slot::{release_idle_gpu_session, set_warm_ai_features, AiFeatureSet};
