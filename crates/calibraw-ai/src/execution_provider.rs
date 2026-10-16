use anyhow::{Context, Result};
use ort::{
    ep::{ExecutionProvider, ExecutionProviderDispatch},
    session::{
        builder::{GraphOptimizationLevel, SessionBuilder},
        Session,
    },
};
#[cfg(not(target_os = "android"))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

#[cfg(not(target_os = "android"))]
static AI_ACCELERATION_ENABLED: AtomicBool = AtomicBool::new(true);
#[cfg(not(target_os = "android"))]
static AI_GPU_MEMORY_QUARANTINED: AtomicBool = AtomicBool::new(false);
#[cfg(not(target_os = "android"))]
static AI_GPU_MEMORY_FAILURE: AtomicBool = AtomicBool::new(false);

#[cfg(not(target_os = "android"))]
pub fn set_ai_acceleration_enabled(enabled: bool) {
    if enabled {
        AI_GPU_MEMORY_QUARANTINED.store(false, Ordering::Release);
        AI_GPU_MEMORY_FAILURE.store(false, Ordering::Release);
    }
    if AI_ACCELERATION_ENABLED.swap(enabled, Ordering::AcqRel) != enabled {
        if let Ok(mut statuses) = provider_statuses().lock() {
            statuses.clear();
        }
        calibraw_core::diagnostics::record(format!(
            "AI GPU acceleration {} in Settings",
            if enabled { "enabled" } else { "disabled" }
        ));
        crate::model_runtime::invalidate_for_provider_change();
    }
}

pub fn ai_acceleration_enabled() -> bool {
    #[cfg(not(target_os = "android"))]
    {
        AI_ACCELERATION_ENABLED.load(Ordering::Acquire)
            && !AI_GPU_MEMORY_QUARANTINED.load(Ordering::Acquire)
    }
    #[cfg(target_os = "android")]
    {
        true
    }
}

#[cfg(not(target_os = "android"))]
pub fn take_ai_gpu_memory_failure() -> bool {
    AI_GPU_MEMORY_FAILURE.swap(false, Ordering::AcqRel)
}

#[cfg(target_os = "android")]
pub fn take_ai_gpu_memory_failure() -> bool {
    false
}

#[cfg(not(target_os = "android"))]
fn quarantine_ai_gpu_after_memory_failure(operation: &str, provider: &str, error: &anyhow::Error) {
    AI_GPU_MEMORY_QUARANTINED.store(true, Ordering::Release);
    AI_GPU_MEMORY_FAILURE.store(true, Ordering::Release);
    let message = format!(
        "AI {operation} exhausted {provider} GPU memory; cancelled the AI job and disabled GPU AI until it is explicitly re-enabled"
    );
    log::error!("{message}: {error:#}");
    calibraw_core::diagnostics::record(message);
}

#[cfg(target_os = "android")]
fn quarantine_ai_gpu_after_memory_failure(
    _operation: &str,
    _provider: &str,
    _error: &anyhow::Error,
) {
}

fn is_gpu_memory_failure(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_ascii_lowercase();
    (message.contains("out of memory")
        || message.contains("failed to allocate memory")
        || message.contains("not enough memory resources"))
        && (message.contains("cuda")
            || message.contains("directml")
            || message.contains("dml")
            || message.contains("tensorrt")
            || message.contains("rocm")
            || message.contains("gpu")
            || message.contains("bfc_arena"))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CpuFallbackProfile {
    #[default]
    Default,
    WindowsSamEncoder,
    MobileDepth,
}

#[derive(Clone, Debug)]
pub struct SessionOptions {
    pub model_name: &'static str,
    pub allow_acceleration: bool,
    pub cpu_fallback_profile: CpuFallbackProfile,
}

impl SessionOptions {
    pub const fn new(model_name: &'static str) -> Self {
        Self {
            model_name,
            allow_acceleration: true,
            cpu_fallback_profile: CpuFallbackProfile::Default,
        }
    }

    pub const fn with_cpu_fallback_profile(mut self, profile: CpuFallbackProfile) -> Self {
        self.cpu_fallback_profile = profile;
        self
    }

    pub const fn cpu_only(mut self) -> Self {
        self.allow_acceleration = false;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionProviderStatus {
    pub model_name: String,
    pub active_provider: String,
    pub degraded: bool,
}

fn provider_statuses() -> &'static Mutex<BTreeMap<String, ExecutionProviderStatus>> {
    static STATUSES: OnceLock<Mutex<BTreeMap<String, ExecutionProviderStatus>>> = OnceLock::new();
    STATUSES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn publish_status(model_name: &str, active_provider: &str, degraded: bool) {
    if let Ok(mut statuses) = provider_statuses().lock() {
        statuses.insert(
            model_name.to_owned(),
            ExecutionProviderStatus {
                model_name: model_name.to_owned(),
                active_provider: active_provider.to_owned(),
                degraded,
            },
        );
    }
}

pub fn active_execution_providers() -> Vec<ExecutionProviderStatus> {
    provider_statuses()
        .lock()
        .map(|statuses| statuses.values().cloned().collect())
        .unwrap_or_default()
}

struct ProviderCandidate {
    name: &'static str,
    provider: ExecutionProviderDispatch,
    force_sequential: bool,
}

/// Adds `provider` if the loaded ONNX Runtime was built with it. A provider the
/// runtime lacks is expected (CPU-only packages) and is not a degradation.
fn push_if_runtime_available<E>(
    providers: &mut Vec<ProviderCandidate>,
    name: &'static str,
    provider: E,
    force_sequential: bool,
) where
    E: ExecutionProvider + Into<ExecutionProviderDispatch>,
{
    match provider.is_available() {
        Ok(true) => providers.push(ProviderCandidate {
            name,
            provider: provider.into(),
            force_sequential,
        }),
        Ok(false) => {
            log::info!("AI execution provider {name} is not built into the selected ONNX Runtime")
        }
        Err(error) => {
            log::warn!(
                "AI execution provider {name} skipped: could not query availability: {error}"
            )
        }
    }
}

fn preferred_execution_providers() -> Vec<ProviderCandidate> {
    let mut providers = Vec::new();

    #[cfg(target_os = "windows")]
    {
        push_if_runtime_available(
            &mut providers,
            "DirectML",
            ort::ep::DirectML::default(),
            true,
        );
        push_if_runtime_available(&mut providers, "CUDA", ort::ep::CUDA::default(), false);
        push_if_runtime_available(
            &mut providers,
            "TensorRT",
            ort::ep::TensorRT::default(),
            false,
        );
    }

    #[cfg(target_os = "macos")]
    {
        push_if_runtime_available(&mut providers, "CoreML", ort::ep::CoreML::default(), false);
    }

    #[cfg(target_os = "linux")]
    {
        push_if_runtime_available(&mut providers, "CUDA", ort::ep::CUDA::default(), false);
        push_if_runtime_available(
            &mut providers,
            "TensorRT",
            ort::ep::TensorRT::default(),
            false,
        );
        push_if_runtime_available(&mut providers, "ROCm", ort::ep::ROCm::default(), false);
    }

    #[cfg(target_os = "android")]
    {
        push_if_runtime_available(&mut providers, "NNAPI", ort::ep::NNAPI::default(), false);
        push_if_runtime_available(
            &mut providers,
            "XNNPACK",
            ort::ep::XNNPACK::default(),
            false,
        );
    }

    providers
}

fn configure_common_builder(mut builder: SessionBuilder) -> Result<SessionBuilder> {
    builder = builder
        .with_memory_pattern(false)
        .map_err(|error| anyhow::anyhow!("disable ONNX Runtime memory pattern: {error}"))?;
    if cfg!(target_os = "android") {
        // Android kills the app when memory runs out, so do not keep a second,
        // packed copy of the model's weights; inference is somewhat slower.
        builder = builder
            .with_prepacking(false)
            .map_err(|error| anyhow::anyhow!("disable Android weight prepacking: {error}"))?;
    }
    Ok(builder)
}

fn configure_cpu_builder(
    mut builder: SessionBuilder,
    options: &SessionOptions,
) -> Result<SessionBuilder> {
    if options.cpu_fallback_profile == CpuFallbackProfile::MobileDepth {
        // Depth runs alongside Android's RAW preview, so bound the thread
        // pool. Every Android session already skips weight prepacking.
        builder = builder
            .with_parallel_execution(false)
            .map_err(|error| anyhow::anyhow!("force sequential mobile depth execution: {error}"))?
            .with_intra_threads(2)
            .map_err(|error| anyhow::anyhow!("limit mobile depth inference threads: {error}"))?
            .with_intra_op_spinning(false)
            .map_err(|error| anyhow::anyhow!("disable mobile depth thread spinning: {error}"))?;
    }
    if options.cpu_fallback_profile == CpuFallbackProfile::WindowsSamEncoder
        && cfg!(target_os = "windows")
    {
        // Some Windows CPU builds produce non-finite Hiera features after graph
        // fusions. Thread count does not affect this, so keep the thread pool.
        builder = builder
            .with_optimization_level(GraphOptimizationLevel::Disable)
            .map_err(|error| {
                anyhow::anyhow!("disable Windows SAM CPU graph optimizations: {error}")
            })?;
    }
    Ok(builder)
}

fn cpu_provider(options: &SessionOptions) -> ExecutionProviderDispatch {
    let disable_arena = cfg!(target_os = "android")
        || options.cpu_fallback_profile == CpuFallbackProfile::MobileDepth
        || (cfg!(target_os = "windows")
            && options.cpu_fallback_profile == CpuFallbackProfile::WindowsSamEncoder);
    ort::ep::CPU::default()
        .with_arena_allocator(!disable_arena)
        .build()
}

fn commit_model(builder: &mut SessionBuilder, source: &Path) -> Result<Session> {
    builder
        .commit_from_file(source)
        .with_context(|| format!("load ONNX model from {}", source.display()))
}

fn create_cpu_session_inner(source: &Path, options: &SessionOptions) -> Result<Session> {
    let builder = Session::builder().context("create CPU ONNX Runtime session")?;
    let builder = configure_common_builder(builder)?;
    let mut builder = configure_cpu_builder(builder, options)?
        .with_execution_providers([cpu_provider(options).error_on_failure()])
        .map_err(|error| anyhow::anyhow!("configure ONNX CPU execution provider: {error}"))?;
    commit_model(&mut builder, source)
}

fn create_accelerated_session(
    source: &Path,
    options: &SessionOptions,
    candidate: ProviderCandidate,
) -> Result<Session> {
    let builder = Session::builder()
        .with_context(|| format!("create {} ONNX Runtime session", candidate.name))?;
    let mut builder = configure_common_builder(builder)?;
    if candidate.force_sequential {
        builder = builder.with_parallel_execution(false).map_err(|error| {
            anyhow::anyhow!("configure {} sequential execution: {error}", candidate.name)
        })?;
    }
    let mut builder = builder
        .with_execution_providers([candidate.provider.error_on_failure(), cpu_provider(options)])
        .map_err(|error| {
            anyhow::anyhow!("configure {} execution provider: {error}", candidate.name)
        })?;
    commit_model(&mut builder, source)
}

pub struct FallbackSession {
    /// `None` only after a CPU rebuild failed; the next run tries it again.
    session: Option<Session>,
    source: PathBuf,
    options: SessionOptions,
    active_provider: &'static str,
    accelerated: bool,
}

impl FallbackSession {
    pub fn run_with_fallback<T, F>(&mut self, operation: &str, mut run: F) -> Result<T>
    where
        F: FnMut(&mut Session, bool) -> Result<T>,
    {
        let accelerated = self.accelerated;
        match run(self.session()?, accelerated) {
            Ok(value) => Ok(value),
            Err(accelerated_error) if accelerated => {
                let failed_provider = self.active_provider;
                if is_gpu_memory_failure(&accelerated_error) {
                    quarantine_ai_gpu_after_memory_failure(
                        operation,
                        failed_provider,
                        &accelerated_error,
                    );
                    return Err(accelerated_error.context(format!(
                        "{operation} exceeded the GPU memory safety budget; the AI job was stopped before CPU fallback"
                    )));
                }
                log::warn!(
                    "{operation} failed on {failed_provider}; rebuilding {} on CPU: {accelerated_error:#}",
                    self.options.model_name
                );
                calibraw_core::diagnostics::record(format!(
                    "AI {}: {failed_provider} inference failed; switching to CPU fallback",
                    self.options.model_name
                ));
                publish_status(self.options.model_name, failed_provider, true);

                // Drop the failed session before building its replacement, so
                // the two sessions' weights and buffers are never held at once.
                self.session = None;
                self.active_provider = "CPU (fallback)";
                self.accelerated = false;
                let model_name = self.options.model_name;
                let session = self.session().with_context(|| {
                    format!(
                        "rebuild {model_name} on CPU after {failed_provider} inference failure: {accelerated_error:#}"
                    )
                })?;
                publish_status(model_name, "CPU (fallback)", true);

                log::info!(
                    "{model_name} now uses CPU fallback after {failed_provider} runtime failure"
                );
                run(session, false).with_context(|| {
                    format!(
                        "{operation} failed on {failed_provider} ({accelerated_error:#}) and again on CPU fallback"
                    )
                })
            }
            Err(error) => Err(error),
        }
    }

    /// The session, built on the CPU when an earlier rebuild left none.
    fn session(&mut self) -> Result<&mut Session> {
        let session = match self.session.take() {
            Some(session) => session,
            None => create_cpu_session_inner(&self.source, &self.options)
                .with_context(|| format!("rebuild {} on CPU", self.options.model_name))?,
        };
        Ok(self.session.insert(session))
    }
}

pub fn create_session_with_fallback(
    model: impl AsRef<Path>,
    options: SessionOptions,
) -> Result<FallbackSession> {
    let source = model.as_ref().to_path_buf();
    let source_description = source.display().to_string();
    let mut attempted_acceleration = false;
    let mut setup_failures = Vec::new();

    let acceleration_enabled = options.allow_acceleration && ai_acceleration_enabled();
    if acceleration_enabled {
        let providers = preferred_execution_providers();
        attempted_acceleration = !providers.is_empty();
        for candidate in providers {
            let provider_name = candidate.name;
            calibraw_core::diagnostics::record(format!(
                "AI {} creating {provider_name} session",
                options.model_name
            ));
            let started = std::time::Instant::now();
            match create_accelerated_session(&source, &options, candidate) {
                Ok(session) => {
                    log::info!(
                        "AI {} uses {provider_name} for {}",
                        options.model_name,
                        source_description
                    );
                    calibraw_core::diagnostics::record(format!(
                        "AI {} execution provider: {provider_name} (session created in {:.3}s)",
                        options.model_name,
                        started.elapsed().as_secs_f64()
                    ));
                    publish_status(options.model_name, provider_name, false);
                    return Ok(FallbackSession {
                        session: Some(session),
                        source,
                        options,
                        active_provider: provider_name,
                        accelerated: true,
                    });
                }
                Err(error) => {
                    log::warn!(
                        "AI {} could not initialize {provider_name}; trying the next provider: {error:#}",
                        options.model_name
                    );
                    setup_failures.push(format!("{provider_name}: {error:#}"));
                }
            }
        }
    }

    calibraw_core::diagnostics::record(format!("AI {} creating CPU session", options.model_name));
    let started = std::time::Instant::now();
    let session = create_cpu_session_inner(&source, &options).with_context(|| {
        if setup_failures.is_empty() {
            format!("create CPU ONNX session for {}", options.model_name)
        } else {
            format!(
                "create CPU fallback for {} after accelerated providers failed: {}",
                options.model_name,
                setup_failures.join(" | ")
            )
        }
    })?;

    let active_provider = if attempted_acceleration {
        "CPU (fallback)"
    } else {
        "CPU"
    };
    if attempted_acceleration {
        log::info!(
            "AI {} uses CPU fallback for {} after accelerator setup failures",
            options.model_name,
            source_description
        );
        calibraw_core::diagnostics::record(format!(
            "AI {} execution provider: CPU (fallback, session created in {:.3}s)",
            options.model_name,
            started.elapsed().as_secs_f64()
        ));
    } else {
        let reason = if !options.allow_acceleration {
            "CPU-only session"
        } else if !acceleration_enabled {
            "GPU acceleration disabled in Settings"
        } else {
            "the ONNX Runtime has no GPU execution provider for this target"
        };
        log::info!(
            "AI {} uses CPU for {} ({reason})",
            options.model_name,
            source_description
        );
        calibraw_core::diagnostics::record(format!(
            "AI {} execution provider: CPU (session created in {:.3}s)",
            options.model_name,
            started.elapsed().as_secs_f64()
        ));
    }
    publish_status(options.model_name, active_provider, attempted_acceleration);

    Ok(FallbackSession {
        session: Some(session),
        source,
        options,
        active_provider,
        accelerated: false,
    })
}

#[cfg(test)]
mod tests {
    use super::is_gpu_memory_failure;

    #[test]
    fn gpu_allocation_failures_are_quarantined_instead_of_retried() {
        let error =
            anyhow::anyhow!("BFCArena::AllocateRawInternal failed to allocate memory on CUDA GPU");
        assert!(is_gpu_memory_failure(&error));
        assert!(is_gpu_memory_failure(&anyhow::anyhow!(
            r"providers\dml\DmlExecutionProvider\src\DmlCommandRecorder.cpp(372) 8007000E Not enough memory resources are available to complete this operation."
        )));
        assert!(!is_gpu_memory_failure(&anyhow::anyhow!(
            "invalid tensor shape"
        )));
    }
}
