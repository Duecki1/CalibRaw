//! The execution provider each model's latest session uses, for Settings and
//! diagnostics.

use std::{
    collections::BTreeMap,
    sync::{Mutex, OnceLock},
};

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

pub(super) fn publish_status(model_name: &str, active_provider: &str, degraded: bool) {
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

/// Forgets every status, for a change that makes them stale.
#[cfg(not(target_os = "android"))]
pub(super) fn clear() {
    if let Ok(mut statuses) = provider_statuses().lock() {
        statuses.clear();
    }
}

pub fn active_execution_providers() -> Vec<ExecutionProviderStatus> {
    provider_statuses()
        .lock()
        .map(|statuses| statuses.values().cloned().collect())
        .unwrap_or_default()
}
