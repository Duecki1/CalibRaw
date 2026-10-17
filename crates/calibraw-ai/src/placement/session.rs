//! Creating a session where `place` puts it, and rebuilding it on the CPU
//! when its GPU provider fails.

use super::failure::{
    quarantine_after_device_loss, GpuFailure, LoggedGpuErrors, RunFailure, SetupFailure,
};
use super::job_record::{GpuJobRecord, GpuSessionRecord};
use super::out_of_memory;
use super::place::{CpuReason, GpuProvider, Placement, ProviderOffer};
use super::providers::{cpu_provider, offered_providers, with_gpu_provider};
use super::quirks::{self, WebGpuCpuNodes};
use super::status::publish_status;
use anyhow::{Context, Result};
use ort::session::{
    builder::{GraphOptimizationLevel, SessionBuilder},
    Session,
};
use std::path::{Path, PathBuf};

const MIB: u64 = 1 << 20;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CpuFallbackProfile {
    #[default]
    Default,
    /// Where `quirks::sam_encoder_cpu_needs_unfused_graph`: no graph
    /// optimizations and no arena allocator.
    SamEncoder,
    /// Two sequential threads, no weight prepacking and no arena allocator,
    /// for a model beside the RAW preview on `MemoryClass::Constrained`.
    LowMemory,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SessionOptions {
    pub(crate) model_name: &'static str,
    pub(crate) allow_acceleration: bool,
    pub(crate) cpu_fallback_profile: CpuFallbackProfile,
    /// Peak GPU memory of one inference, in bytes, as `registry` measured it.
    /// `place` keeps the session off a GPU whose adapter has less free,
    /// margin included: allocations beyond it fail, or on Windows overcommit
    /// video memory, which pages until the slowdown ends in a driver reset.
    pub(crate) gpu_memory_bytes: Option<u64>,
    /// Nodes its WebGPU sessions run on the CPU (`quirks::WebGpuCpuNodes`).
    pub(crate) webgpu_cpu_nodes: WebGpuCpuNodes,
}

impl SessionOptions {
    pub(crate) const fn new(model_name: &'static str) -> Self {
        Self {
            model_name,
            allow_acceleration: true,
            cpu_fallback_profile: CpuFallbackProfile::Default,
            gpu_memory_bytes: None,
            webgpu_cpu_nodes: WebGpuCpuNodes::None,
        }
    }

    pub(crate) const fn with_gpu_memory(mut self, bytes: u64) -> Self {
        self.gpu_memory_bytes = Some(bytes);
        self
    }

    pub(crate) const fn with_webgpu_cpu_nodes(mut self, nodes: WebGpuCpuNodes) -> Self {
        self.webgpu_cpu_nodes = nodes;
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

/// Where a session of `options` for the model at `source` runs now, and the
/// providers on offer. The providers are only queried when the session may
/// use one.
fn current_placement(source: &Path, options: &SessionOptions) -> (Placement, Vec<ProviderOffer>) {
    let settings = super::acceleration_settings();
    let offers = if options.allow_acceleration && settings.usable() {
        offered_providers(super::editor_gpu())
    } else {
        Vec::new()
    };
    let placement = place_on(source, options, settings, &offers);
    (placement, offers)
}

fn place_on(
    source: &Path,
    options: &SessionOptions,
    settings: super::AccelerationSettings,
    offers: &[ProviderOffer],
) -> Placement {
    super::place(
        options,
        &settings.with_providers(offers, out_of_memory::lookup(source)),
    )
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
    if options.cpu_fallback_profile == CpuFallbackProfile::LowMemory {
        // The model runs alongside the RAW preview. Bound the thread pool and
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
    if options.cpu_fallback_profile == CpuFallbackProfile::SamEncoder
        && quirks::sam_encoder_cpu_needs_unfused_graph()
    {
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
    offer: &ProviderOffer,
) -> Result<Session> {
    let name = offer.provider.name();
    let builder =
        Session::builder().with_context(|| format!("create {name} ONNX Runtime session"))?;
    let builder = configure_common_builder(builder)?;
    let mut builder = with_gpu_provider(builder, offer, options, super::editor_gpu())?;
    commit_model(&mut builder, source)
}

pub(crate) struct FallbackSession {
    session: Session,
    source: PathBuf,
    options: SessionOptions,
    placement: Placement,
    /// Bytes free on the GPU provider's adapter when the session was placed
    /// on it, where known; `None` on the CPU.
    gpu_free_before: Option<u64>,
    /// The WebGPU errors ONNX Runtime had logged when this GPU session was
    /// created; it is checked against them after each run, so an error
    /// logged after a run returned fails the next one.
    logged_gpu_errors: LoggedGpuErrors,
    /// Records the session while it is loaded on a GPU; `None` on the CPU.
    /// Declared after `session`, so it ends after the session was dropped.
    gpu_record: Option<GpuSessionRecord>,
}

impl FallbackSession {
    /// Records this session's GPU job while the returned value lives; `None`
    /// for CPU sessions.
    pub(crate) fn record_gpu_job(&self) -> Option<GpuJobRecord> {
        match self.placement {
            Placement::Gpu { provider } => Some(GpuJobRecord::begin(
                self.options.model_name,
                provider.name(),
            )),
            Placement::Cpu { .. } => None,
        }
    }

    pub(crate) fn on_gpu(&self) -> bool {
        matches!(self.placement, Placement::Gpu { .. })
    }

    /// Whether this session is on the CPU only because GPU memory was short,
    /// and the GPU now has room for it, so the next job should get a new
    /// session instead of reusing this one. After running out of memory,
    /// "room" is what `out_of_memory` allows.
    pub(crate) fn gpu_now_fits(&self) -> bool {
        match self.placement {
            Placement::Cpu { reason } if reason.memory_bound() => matches!(
                current_placement(&self.source, &self.options).0,
                Placement::Gpu { .. }
            ),
            _ => false,
        }
    }

    /// Whether this session may stay loaded after its job. A WebGPU session
    /// keeps its peak in cached buffers, so it stays only while its adapter
    /// keeps the margin `place` asked for beside it (`may_stay_loaded`),
    /// which leaves the editor room. CPU sessions may stay. Records why when
    /// it may not.
    pub(crate) fn may_stay_loaded(&self) -> bool {
        let Placement::Gpu { provider } = self.placement else {
            return true;
        };
        let peak = self.options.gpu_memory_bytes;
        let free = offered_providers(super::editor_gpu())
            .into_iter()
            .find(|offer| offer.provider == provider)
            .and_then(|offer| offer.free_memory);
        if super::may_stay_loaded(free, peak) {
            return true;
        }
        calibraw_core::diagnostics::record(format!(
            "AI {}: unloading its {} session after the job, {} MiB of GPU memory free, {} MiB kept free beside it",
            self.options.model_name,
            provider.name(),
            free.unwrap_or_default() / MIB,
            peak.map_or(0, |peak| super::needed_memory(peak) - peak) / MIB
        ));
        false
    }

    pub(crate) fn run_with_fallback<T, F>(&mut self, operation: &str, mut run: F) -> Result<T>
    where
        F: FnMut(&mut Session, bool) -> Result<T>,
    {
        let Placement::Gpu { provider } = self.placement else {
            return run(&mut self.session, false);
        };
        let result = run(&mut self.session, true);
        let failure = match GpuFailure::check(operation, result, self.logged_gpu_errors.since()) {
            Ok(value) => {
                out_of_memory::forget(&self.source);
                return Ok(value);
            }
            Err(failure) => failure,
        };
        let failed = provider.name();
        let model_name = self.options.model_name;
        let run_failure = RunFailure::of(&failure);
        let gpu_error = failure.into_error();
        let reason = match run_failure {
            RunFailure::StopJob => {
                quarantine_after_device_loss(operation, failed, &gpu_error);
                return Err(gpu_error.context(format!(
                    "{operation} failed on {failed}; the AI job was stopped before CPU fallback"
                )));
            }
            RunFailure::RerunOnCpu(reason) => reason,
        };
        if reason == CpuReason::OutOfMemory {
            out_of_memory::record(&self.source, self.gpu_free_before);
        }
        log::warn!("{operation} failed on {failed}; rebuilding {model_name} on CPU: {gpu_error:#}");
        calibraw_core::diagnostics::record(format!(
            "AI {model_name}: {failed} inference failed ({reason}); switching to CPU fallback"
        ));

        let cpu_session =
            create_cpu_session_inner(&self.source, &self.options).with_context(|| {
                format!(
                    "rebuild {model_name} on CPU after {failed} inference failure: {gpu_error:#}"
                )
            })?;
        self.session = cpu_session;
        self.placement = Placement::Cpu { reason };
        self.gpu_free_before = None;
        self.gpu_record = None;
        publish_status(model_name, self.placement, None);

        log::info!("{model_name} now uses CPU fallback after {failed} runtime failure");
        run(&mut self.session, false).with_context(|| {
            format!("{operation} failed on {failed} ({gpu_error:#}) and again on CPU fallback")
        })
    }
}

pub(crate) fn create_session_with_fallback(
    model: impl AsRef<Path>,
    options: SessionOptions,
) -> Result<FallbackSession> {
    let source = model.as_ref().to_path_buf();
    let (mut placement, mut offers) = current_placement(&source, &options);
    let mut setup_failures = Vec::new();

    let reason = loop {
        let provider = match placement {
            Placement::Gpu { provider } => provider,
            Placement::Cpu { reason } if setup_failures.is_empty() => break reason,
            Placement::Cpu { .. } => break CpuReason::GpuFailed,
        };
        let offer = take_offer(&mut offers, provider);
        match try_gpu_session(&source, &options, &offer) {
            Ok((session, logged_gpu_errors, gpu_record)) => {
                let gpu_free_before = offer.free_memory;
                publish_status(options.model_name, placement, offer.adapter);
                return Ok(FallbackSession {
                    session,
                    source,
                    options,
                    placement,
                    gpu_free_before,
                    logged_gpu_errors,
                    gpu_record: Some(gpu_record),
                });
            }
            Err(failure) => {
                let name = provider.name();
                let error = failure.error();
                setup_failures.push(format!("{name}: {error:#}"));
                if let SetupFailure::Cpu(reason) = SetupFailure::of(&failure) {
                    if reason == CpuReason::OutOfMemory {
                        out_of_memory::record(&source, offer.free_memory);
                    }
                    log::warn!(
                        "AI {} ran out of GPU memory creating its {name} session; using the CPU for this job: {error:#}",
                        options.model_name
                    );
                    break reason;
                }
                log::warn!(
                    "AI {} could not initialize {name}; trying the next provider: {error:#}",
                    options.model_name
                );
                placement = place_on(&source, &options, super::acceleration_settings(), &offers);
            }
        }
    };

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
    log::info!(
        "AI {} uses CPU for {} ({reason})",
        options.model_name,
        source.display()
    );
    calibraw_core::diagnostics::record(format!(
        "AI {} execution provider: CPU, {reason} (session created in {:.3}s)",
        options.model_name,
        started.elapsed().as_secs_f64()
    ));
    let placement = Placement::Cpu { reason };
    publish_status(options.model_name, placement, None);

    Ok(FallbackSession {
        session,
        source,
        options,
        placement,
        gpu_free_before: None,
        logged_gpu_errors: LoggedGpuErrors::now(),
        gpu_record: None,
    })
}

/// Removes and returns `provider`'s offer, so a provider that failed is not
/// placed on again.
fn take_offer(offers: &mut Vec<ProviderOffer>, provider: GpuProvider) -> ProviderOffer {
    let index = offers
        .iter()
        .position(|offer| offer.provider == provider)
        .expect("`place` only picks an offered provider");
    offers.remove(index)
}

/// A session on `offer`'s provider, with the WebGPU errors logged before it
/// was created and its record as a loaded model, which takes over from the
/// record of its creation as a job.
fn try_gpu_session(
    source: &Path,
    options: &SessionOptions,
    offer: &ProviderOffer,
) -> std::result::Result<(Session, LoggedGpuErrors, GpuSessionRecord), GpuFailure> {
    let name = offer.provider.name();
    let memory = match (offer.free_memory, options.gpu_memory_bytes) {
        (Some(free), Some(peak)) => format!(
            ", {} MiB free, {} MiB needed",
            free / MIB,
            super::needed_memory(peak) / MIB
        ),
        _ => String::new(),
    };
    calibraw_core::diagnostics::record(format!(
        "AI {} creating {name} session{memory}",
        options.model_name
    ));
    let started = std::time::Instant::now();
    let _record = GpuJobRecord::begin(options.model_name, name);
    let logged_gpu_errors = LoggedGpuErrors::now();
    // Creating the session uploads the model's weights.
    let created = create_accelerated_session(source, options, offer);
    let session = GpuFailure::check("creating the session", created, logged_gpu_errors.since())?;
    log::info!(
        "AI {} uses {name} for {}",
        options.model_name,
        source.display()
    );
    calibraw_core::diagnostics::record(format!(
        "AI {} execution provider: {name} (session created in {:.3}s)",
        options.model_name,
        started.elapsed().as_secs_f64()
    ));
    let gpu_record = GpuSessionRecord::begin(options.model_name, name);
    Ok((session, logged_gpu_errors, gpu_record))
}
