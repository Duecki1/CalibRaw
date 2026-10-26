//! Where each model's latest session runs, for Settings and diagnostics.

use super::place::{CpuReason, Placement};
use std::{
    collections::BTreeMap,
    sync::{Mutex, OnceLock},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiSessionStatus {
    pub model_name: String,
    pub placement: Placement,
    /// The adapter a GPU session runs on, where it is the editor's.
    pub adapter: Option<String>,
}

impl AiSessionStatus {
    /// `GPU · <adapter> (<provider>)`, `GPU · <provider>` when the adapter is
    /// not known, or `CPU · <reason>`.
    pub fn label(&self) -> String {
        match (self.placement, &self.adapter) {
            (Placement::Gpu { provider }, Some(adapter)) => {
                format!("GPU · {adapter} ({})", provider.name())
            }
            (Placement::Gpu { provider }, None) => format!("GPU · {}", provider.name()),
            (Placement::Cpu { reason }, _) => format!("CPU · {reason}"),
        }
    }

    /// Whether a GPU provider failed for this model, other than by running
    /// out of memory.
    pub fn gpu_failed(&self) -> bool {
        self.placement
            == Placement::Cpu {
                reason: CpuReason::GpuFailed,
            }
    }
}

fn statuses() -> &'static Mutex<BTreeMap<&'static str, AiSessionStatus>> {
    static STATUSES: OnceLock<Mutex<BTreeMap<&'static str, AiSessionStatus>>> = OnceLock::new();
    STATUSES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(super) fn publish_status(
    model_name: &'static str,
    placement: Placement,
    adapter: Option<String>,
) {
    if let Ok(mut statuses) = statuses().lock() {
        statuses.insert(
            model_name,
            AiSessionStatus {
                model_name: model_name.to_owned(),
                placement,
                adapter,
            },
        );
    }
}

/// Forgets every status, for a change that makes them stale.
#[cfg(not(target_os = "android"))]
pub(super) fn clear() {
    if let Ok(mut statuses) = statuses().lock() {
        statuses.clear();
    }
}

/// The latest session of each model that had one, by model name.
pub fn ai_session_statuses() -> Vec<AiSessionStatus> {
    statuses()
        .lock()
        .map(|statuses| statuses.values().cloned().collect())
        .unwrap_or_default()
}
