//! The one loaded model session, and which features may keep theirs loaded
//! between jobs.

use crate::placement::{
    ai_acceleration_enabled, create_session_with_fallback, provider_generation, FallbackSession,
    GpuJobRecord, SessionOptions,
};
use crate::registry::{AiFeature, AiModel};
use anyhow::Result;
use std::{
    ops::{Deref, DerefMut},
    path::Path,
    sync::{
        atomic::{AtomicU8, Ordering},
        Mutex, MutexGuard, OnceLock, TryLockError,
    },
};

/// Features whose models may stay loaded between jobs, typically those whose
/// tools are on screen, while a GPU model also leaves the editor room
/// (`FallbackSession::may_stay_loaded`). It only governs memory: a running
/// job always finishes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AiFeatureSet(u8);

impl AiFeatureSet {
    pub const EMPTY: Self = Self(0);

    #[must_use]
    pub const fn with(self, feature: AiFeature) -> Self {
        Self(self.0 | Self::bit(feature))
    }

    pub const fn contains(self, feature: AiFeature) -> bool {
        self.0 & Self::bit(feature) != 0
    }

    const fn bit(feature: AiFeature) -> u8 {
        1 << feature as u8
    }
}

impl FromIterator<AiFeature> for AiFeatureSet {
    fn from_iter<I: IntoIterator<Item = AiFeature>>(features: I) -> Self {
        features.into_iter().fold(Self::EMPTY, Self::with)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelRetention {
    /// Stays loaded after a job while the model's feature is warm.
    WhileWarm,
    /// Unloads as soon as its job releases the session.
    OneShot,
}

struct ActiveModel<S> {
    model: AiModel,
    retention: ModelRetention,
    provider_generation: u64,
    acceleration_enabled: bool,
    session: S,
}

struct RuntimeSlot<S> {
    active: Option<ActiveModel<S>>,
}

impl<S> Default for RuntimeSlot<S> {
    fn default() -> Self {
        Self { active: None }
    }
}

impl<S> RuntimeSlot<S> {
    /// The loaded session of `model`, or a new one from `create`. A loaded
    /// session is reused while the providers are unchanged and `reusable`
    /// accepts it.
    fn ensure_model<E>(
        &mut self,
        model: AiModel,
        retention: ModelRetention,
        provider_generation: u64,
        acceleration_enabled: bool,
        reusable: impl FnOnce(&S) -> bool,
        create: impl FnOnce() -> std::result::Result<S, E>,
    ) -> std::result::Result<&mut S, E> {
        let reusable = self.active.as_ref().is_some_and(|active| {
            active.model == model
                && active.provider_generation == provider_generation
                && active.acceleration_enabled == acceleration_enabled
                && reusable(&active.session)
        });
        if !reusable {
            if let Some(active) = self.active.take() {
                log::info!("unloading cached AI model session: {:?}", active.model);
                drop(active);
            }
            let session = create()?;
            log::info!("loaded AI model session: {model:?}");
            self.active = Some(ActiveModel {
                model,
                retention,
                provider_generation,
                acceleration_enabled,
                session,
            });
        } else if let Some(active) = self.active.as_mut() {
            active.retention = retention;
        }
        Ok(&mut self
            .active
            .as_mut()
            .expect("AI session was just created")
            .session)
    }

    fn reconcile(
        &mut self,
        warm: AiFeatureSet,
        provider_generation: u64,
        acceleration_enabled: bool,
    ) {
        let retain = self.active.as_ref().is_some_and(|active| {
            active.provider_generation == provider_generation
                && active.acceleration_enabled == acceleration_enabled
                && active.retention == ModelRetention::WhileWarm
                && warm.contains(active.model.spec().feature)
        });
        if !retain {
            if let Some(active) = self.active.take() {
                log::info!("unloading AI model session: {:?}", active.model);
                drop(active);
            }
        }
    }

    /// Unloads the loaded session if `unload` says so.
    fn unload_if(&mut self, unload: impl FnOnce(&S) -> bool) -> Option<AiModel> {
        let active = self.active.take_if(|active| unload(&active.session))?;
        Some(active.model)
    }
}

fn runtime() -> &'static Mutex<RuntimeSlot<FallbackSession>> {
    static RUNTIME: OnceLock<Mutex<RuntimeSlot<FallbackSession>>> = OnceLock::new();
    RUNTIME.get_or_init(|| Mutex::new(RuntimeSlot::default()))
}

static WARM_FEATURES: AtomicU8 = AtomicU8::new(0);

fn warm_features() -> AiFeatureSet {
    AiFeatureSet(WARM_FEATURES.load(Ordering::Acquire))
}

fn lock_runtime() -> MutexGuard<'static, RuntimeSlot<FallbackSession>> {
    runtime()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn try_reconcile<S>(
    runtime: &Mutex<RuntimeSlot<S>>,
    warm: AiFeatureSet,
    provider_generation: u64,
    acceleration_enabled: bool,
) -> bool {
    match runtime.try_lock() {
        Ok(mut slot) => {
            slot.reconcile(warm, provider_generation, acceleration_enabled);
            true
        }
        Err(TryLockError::Poisoned(error)) => {
            let mut slot = error.into_inner();
            slot.reconcile(warm, provider_generation, acceleration_enabled);
            true
        }
        Err(TryLockError::WouldBlock) => false,
    }
}

/// Declares which features may keep their model loaded between jobs. Models
/// of other features unload now, or once their running job releases them.
pub fn set_warm_ai_features(features: AiFeatureSet) {
    WARM_FEATURES.store(features.0, Ordering::Release);
    unload_cold_session();
}

/// Unloads the loaded session unless it may stay warm with the current warm
/// features and providers, or leaves that to its running job's release.
pub(crate) fn unload_cold_session() {
    let _ = try_reconcile(
        runtime(),
        warm_features(),
        provider_generation(),
        ai_acceleration_enabled(),
    );
}

/// Unloads the loaded session if it runs on a GPU and no job holds it, so its
/// GPU memory returns after the editor ran out. A held session is left to its
/// job; the next job of an unloaded model places it again.
pub fn release_idle_gpu_session() {
    let mut slot = match runtime().try_lock() {
        Ok(slot) => slot,
        Err(TryLockError::Poisoned(error)) => error.into_inner(),
        Err(TryLockError::WouldBlock) => return,
    };
    if let Some(model) = slot.unload_if(FallbackSession::on_gpu) {
        log::info!("unloaded idle AI model session {model:?} to free GPU memory");
    }
}

pub(crate) struct ModelSessionGuard {
    runtime: Option<MutexGuard<'static, RuntimeSlot<FallbackSession>>>,
    /// Marks the lease as GPU work until it ends; see `placement::job_record`.
    gpu_job: Option<GpuJobRecord>,
}

impl Deref for ModelSessionGuard {
    type Target = FallbackSession;

    fn deref(&self) -> &Self::Target {
        &self
            .runtime
            .as_ref()
            .expect("AI model session lease was already released")
            .active
            .as_ref()
            .expect("AI model session lease has no active session")
            .session
    }
}

impl DerefMut for ModelSessionGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self
            .runtime
            .as_mut()
            .expect("AI model session lease was already released")
            .active
            .as_mut()
            .expect("AI model session lease has no active session")
            .session
    }
}

impl Drop for ModelSessionGuard {
    fn drop(&mut self) {
        let Some(mut guard) = self.runtime.take() else {
            return;
        };
        guard.reconcile(
            warm_features(),
            provider_generation(),
            ai_acceleration_enabled(),
        );
        // A warm GPU session holds its peak; it stays only while the GPU
        // keeps room for the editor beside it.
        if let Some(model) = guard.unload_if(|session| !session.may_stay_loaded()) {
            log::info!("unloaded AI model session {model:?} after its job to free GPU memory");
        }
        // End the record while the lease is held: once it is released, the
        // next lease may begin its own record, which this one must not clear.
        drop(self.gpu_job.take());
        drop(guard);

        let _ = try_reconcile(
            runtime(),
            warm_features(),
            provider_generation(),
            ai_acceleration_enabled(),
        );
    }
}

pub(crate) fn acquire_model_session(
    model: AiModel,
    source: impl AsRef<Path>,
    options: SessionOptions,
    retention: ModelRetention,
) -> Result<ModelSessionGuard> {
    let mut runtime = lock_runtime();
    let provider_generation = provider_generation();
    let acceleration_enabled = ai_acceleration_enabled();
    runtime.ensure_model(
        model,
        retention,
        provider_generation,
        acceleration_enabled,
        // A session on the CPU for lack of GPU memory moves to the GPU once
        // the GPU has room.
        |session| !session.gpu_now_fits(),
        || create_session_with_fallback(source, options),
    )?;
    let gpu_job = runtime
        .active
        .as_ref()
        .and_then(|active| active.session.record_gpu_job());
    Ok(ModelSessionGuard {
        runtime: Some(runtime),
        gpu_job,
    })
}

pub(crate) fn with_model_session<T>(
    model: AiModel,
    source: impl AsRef<Path>,
    options: SessionOptions,
    retention: ModelRetention,
    run: impl FnOnce(&mut FallbackSession) -> Result<T>,
) -> Result<T> {
    let mut session = acquire_model_session(model, source, options, retention)?;
    run(&mut session)
}
