//! Creating a session on the preferred provider, and rebuilding it on the
//! CPU when the accelerated one fails.

use super::failure::{quarantine_ai_gpu, AiGpuFailure};
use super::job_record::GpuJobRecord;
use super::providers::{
    cpu_provider, gpu_memory_shortfall, preferred_execution_providers, ProviderCandidate,
};
use super::status::publish_status;
use anyhow::{Context, Result};
use ort::session::{
    builder::{GraphOptimizationLevel, SessionBuilder},
    Session,
};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CpuFallbackProfile {
    #[default]
    Default,
    WindowsSamEncoder,
    MobileDepth,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SessionOptions {
    pub(crate) model_name: &'static str,
    pub(crate) allow_acceleration: bool,
    pub(crate) cpu_fallback_profile: CpuFallbackProfile,
    /// Peak GPU memory a session of this model needs. Where a provider can
    /// measure what is free (DirectML), it is skipped when less is: Windows
    /// pages overcommitted video memory instead of failing allocations, and
    /// the slowdown ends in a driver reset.
    pub(crate) gpu_memory_bytes: Option<u64>,
}

impl SessionOptions {
    pub(crate) const fn new(model_name: &'static str) -> Self {
        Self {
            model_name,
            allow_acceleration: true,
            cpu_fallback_profile: CpuFallbackProfile::Default,
            gpu_memory_bytes: None,
        }
    }

    pub(crate) const fn with_gpu_memory(mut self, bytes: u64) -> Self {
        self.gpu_memory_bytes = Some(bytes);
        self
    }

    pub(crate) const fn with_cpu_fallback_profile(mut self, profile: CpuFallbackProfile) -> Self {
        self.cpu_fallback_profile = profile;
        self
    }

    pub(crate) const fn cpu_only(mut self) -> Self {
        self.allow_acceleration = false;
        self
    }
}

fn configure_common_builder(mut builder: SessionBuilder) -> Result<SessionBuilder> {
    builder = builder
        .with_memory_pattern(false)
        .map_err(|error| anyhow::anyhow!("disable ONNX Runtime memory pattern: {error}"))?;
    Ok(builder)
}

fn configure_cpu_builder(
    mut builder: SessionBuilder,
    options: &SessionOptions,
) -> Result<SessionBuilder> {
    if options.cpu_fallback_profile == CpuFallbackProfile::MobileDepth {
        // Depth runs alongside Android's RAW preview. Bound the thread pool and
        // avoid retaining a second, packed copy of the model's weights.
        builder = builder
            .with_parallel_execution(false)
            .map_err(|error| anyhow::anyhow!("force sequential mobile depth execution: {error}"))?
            .with_intra_threads(2)
            .map_err(|error| anyhow::anyhow!("limit mobile depth inference threads: {error}"))?
            .with_intra_op_spinning(false)
            .map_err(|error| anyhow::anyhow!("disable mobile depth thread spinning: {error}"))?
            .with_prepacking(false)
            .map_err(|error| anyhow::anyhow!("disable mobile depth weight prepacking: {error}"))?;
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

pub(crate) struct FallbackSession {
    session: Session,
    source: PathBuf,
    options: SessionOptions,
    active_provider: &'static str,
    accelerated: bool,
}

impl FallbackSession {
    /// Records this session's GPU job while the returned value lives; `None`
    /// for CPU sessions.
    pub(crate) fn record_gpu_job(&self) -> Option<GpuJobRecord> {
        self.accelerated
            .then(|| GpuJobRecord::begin(self.options.model_name, self.active_provider))
    }

    pub(crate) fn run_with_fallback<T, F>(&mut self, operation: &str, mut run: F) -> Result<T>
    where
        F: FnMut(&mut Session, bool) -> Result<T>,
    {
        match run(&mut self.session, self.accelerated) {
            Ok(value) => Ok(value),
            Err(accelerated_error) if self.accelerated => {
                let failed_provider = self.active_provider;
                if let Some(failure) = AiGpuFailure::of_error(&accelerated_error) {
                    quarantine_ai_gpu(failure, operation, failed_provider, &accelerated_error);
                    return Err(accelerated_error.context(format!(
                        "{operation} failed on {failed_provider}; the AI job was stopped before CPU fallback"
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

                let cpu_session = create_cpu_session_inner(&self.source, &self.options)
                    .with_context(|| {
                        format!(
                            "rebuild {} on CPU after {failed_provider} inference failure: {accelerated_error:#}",
                            self.options.model_name
                        )
                    })?;
                self.session = cpu_session;
                self.active_provider = "CPU (fallback)";
                self.accelerated = false;
                publish_status(self.options.model_name, self.active_provider, true);

                log::info!(
                    "{} now uses CPU fallback after {failed_provider} runtime failure",
                    self.options.model_name
                );
                run(&mut self.session, false).with_context(|| {
                    format!(
                        "{operation} failed on {failed_provider} ({accelerated_error:#}) and again on CPU fallback"
                    )
                })
            }
            Err(error) => Err(error),
        }
    }
}

pub(crate) fn create_session_with_fallback(
    model: impl AsRef<Path>,
    options: SessionOptions,
) -> Result<FallbackSession> {
    let source = model.as_ref().to_path_buf();
    let source_description = source.display().to_string();
    let mut attempted_acceleration = false;
    let mut setup_failures = Vec::new();

    let acceleration_enabled = options.allow_acceleration && super::ai_acceleration_enabled();
    if acceleration_enabled {
        let providers = preferred_execution_providers();
        attempted_acceleration = !providers.is_empty();
        for candidate in providers {
            let provider_name = candidate.name;
            if let Some(shortfall) =
                gpu_memory_shortfall(options.gpu_memory_bytes, candidate.available_memory)
            {
                log::warn!(
                    "AI {} skips {provider_name}: {shortfall}",
                    options.model_name
                );
                calibraw_core::diagnostics::record(format!(
                    "AI {} skips {provider_name}: {shortfall}",
                    options.model_name
                ));
                setup_failures.push(format!("{provider_name}: {shortfall}"));
                continue;
            }
            calibraw_core::diagnostics::record(format!(
                "AI {} creating {provider_name} session",
                options.model_name
            ));
            let started = std::time::Instant::now();
            let _record = GpuJobRecord::begin(options.model_name, provider_name);
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
                        session,
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
        session,
        source,
        options,
        active_provider,
        accelerated: false,
    })
}
