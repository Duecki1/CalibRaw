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
/// tools are on screen. It only governs memory: a running job always finishes.
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
    fn ensure_model<E>(
        &mut self,
        model: AiModel,
        retention: ModelRetention,
        provider_generation: u64,
        acceleration_enabled: bool,
        create: impl FnOnce() -> std::result::Result<S, E>,
    ) -> std::result::Result<&mut S, E> {
        let reusable = self.active.as_ref().is_some_and(|active| {
            active.model == model
                && active.provider_generation == provider_generation
                && active.acceleration_enabled == acceleration_enabled
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

    #[cfg(test)]
    fn active_model(&self) -> Option<AiModel> {
        self.active.as_ref().map(|active| active.model)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{Arc, Barrier},
        thread,
        time::{Duration, Instant},
    };

    #[derive(Clone)]
    struct DropLog {
        label: &'static str,
        events: Arc<Mutex<Vec<String>>>,
    }

    impl Drop for DropLog {
        fn drop(&mut self) {
            self.events
                .lock()
                .unwrap()
                .push(format!("drop {}", self.label));
        }
    }

    fn warm(feature: AiFeature) -> AiFeatureSet {
        AiFeatureSet::EMPTY.with(feature)
    }

    #[test]
    fn same_model_is_reused() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut slot = RuntimeSlot::default();
        slot.ensure_model(
            AiModel::BiRefNet,
            ModelRetention::WhileWarm,
            0,
            true,
            || {
                events.lock().unwrap().push("create low".to_owned());
                Ok::<_, ()>(DropLog {
                    label: "low",
                    events: Arc::clone(&events),
                })
            },
        )
        .unwrap();
        slot.ensure_model(
            AiModel::BiRefNet,
            ModelRetention::WhileWarm,
            0,
            true,
            || {
                events.lock().unwrap().push("create low again".to_owned());
                Ok::<_, ()>(DropLog {
                    label: "low again",
                    events: Arc::clone(&events),
                })
            },
        )
        .unwrap();
        assert_eq!(slot.active_model(), Some(AiModel::BiRefNet));
        assert_eq!(&*events.lock().unwrap(), &["create low".to_owned()]);
    }

    #[test]
    fn different_model_drops_previous_before_create() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut slot = RuntimeSlot::default();
        slot.ensure_model(
            AiModel::BiRefNet,
            ModelRetention::WhileWarm,
            0,
            true,
            || {
                Ok::<_, ()>(DropLog {
                    label: "low",
                    events: Arc::clone(&events),
                })
            },
        )
        .unwrap();
        slot.ensure_model(
            AiModel::SamEncoder,
            ModelRetention::WhileWarm,
            0,
            true,
            || {
                events.lock().unwrap().push("create encoder".to_owned());
                Ok::<_, ()>(DropLog {
                    label: "encoder",
                    events: Arc::clone(&events),
                })
            },
        )
        .unwrap();
        assert_eq!(
            &*events.lock().unwrap(),
            &["drop low".to_owned(), "create encoder".to_owned()]
        );
        assert_eq!(slot.active_model(), Some(AiModel::SamEncoder));
    }

    #[test]
    fn warm_feature_keeps_its_model_and_cooling_it_unloads() {
        let mut slot = RuntimeSlot::default();
        slot.ensure_model(
            AiModel::SamDecoder,
            ModelRetention::WhileWarm,
            0,
            true,
            || Ok::<_, ()>(()),
        )
        .unwrap();
        slot.reconcile(warm(AiFeature::Object), 0, true);
        assert_eq!(slot.active_model(), Some(AiModel::SamDecoder));
        slot.reconcile(warm(AiFeature::SceneDepth), 0, true);
        assert_eq!(slot.active_model(), None);
    }

    #[test]
    fn scene_depth_stays_warm_for_effects_without_a_mask_context() {
        let mut slot = RuntimeSlot::default();
        slot.ensure_model(AiModel::Depth, ModelRetention::WhileWarm, 0, true, || {
            Ok::<_, ()>(())
        })
        .unwrap();
        slot.reconcile(warm(AiFeature::SceneDepth), 0, true);
        assert_eq!(slot.active_model(), Some(AiModel::Depth));
    }

    #[test]
    fn one_shot_model_unloads_after_inference() {
        let mut slot = RuntimeSlot::default();
        slot.ensure_model(
            AiModel::RawNindBayer,
            ModelRetention::OneShot,
            0,
            true,
            || Ok::<_, ()>(()),
        )
        .unwrap();
        slot.reconcile(AiFeatureSet::from_iter(AiFeature::ALL), 0, true);
        assert_eq!(slot.active_model(), None);
    }

    #[test]
    fn unload_request_does_not_block() {
        let runtime = Arc::new(Mutex::new(RuntimeSlot::<()>::default()));
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let worker_runtime = Arc::clone(&runtime);
        let worker_entered = Arc::clone(&entered);
        let worker_release = Arc::clone(&release);
        let worker = thread::spawn(move || {
            let _guard = worker_runtime.lock().unwrap();
            worker_entered.wait();
            worker_release.wait();
        });
        entered.wait();
        let start = Instant::now();
        assert!(!try_reconcile(&runtime, AiFeatureSet::EMPTY, 0, true));
        assert!(start.elapsed() < Duration::from_millis(100));
        release.wait();
        worker.join().unwrap();
    }

    #[test]
    fn pending_unload_is_applied_after_inference_releases_session() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let runtime = Arc::new(Mutex::new(RuntimeSlot::<DropLog>::default()));
        let warm_set = Arc::new(AtomicU8::new(warm(AiFeature::Remove).0));
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));

        let worker_runtime = Arc::clone(&runtime);
        let worker_warm_set = Arc::clone(&warm_set);
        let worker_events = Arc::clone(&events);
        let worker_entered = Arc::clone(&entered);
        let worker_release = Arc::clone(&release);
        let worker = thread::spawn(move || {
            let mut slot = worker_runtime.lock().unwrap();
            slot.ensure_model(AiModel::BigLama, ModelRetention::WhileWarm, 0, true, || {
                Ok::<_, ()>(DropLog {
                    label: "big-lama",
                    events: worker_events,
                })
            })
            .unwrap();
            worker_entered.wait();
            worker_release.wait();
            slot.reconcile(
                AiFeatureSet(worker_warm_set.load(Ordering::Acquire)),
                0,
                true,
            );
        });

        entered.wait();
        warm_set.store(AiFeatureSet::EMPTY.0, Ordering::Release);
        assert!(!try_reconcile(&runtime, AiFeatureSet::EMPTY, 0, true));
        release.wait();
        worker.join().unwrap();
        assert_eq!(runtime.lock().unwrap().active_model(), None);
        assert_eq!(&*events.lock().unwrap(), &["drop big-lama".to_owned()]);
    }

    #[test]
    fn provider_policy_change_invalidates_current_session() {
        let mut slot = RuntimeSlot::default();
        slot.ensure_model(
            AiModel::SamDecoder,
            ModelRetention::WhileWarm,
            4,
            true,
            || Ok::<_, ()>(()),
        )
        .unwrap();
        slot.reconcile(warm(AiFeature::Object), 5, false);
        assert_eq!(slot.active_model(), None);
    }
}
