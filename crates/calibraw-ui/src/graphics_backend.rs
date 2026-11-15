//! The graphics backend the editor window and its photo processing run on:
//! the persisted choice, and the backends this machine offers, found when the
//! window's adapter is chosen at startup (`native_options`).
//!
//! The editor's wgpu device is created once, before the app exists, so a new
//! choice takes effect at the next start.

use eframe::wgpu;
use serde::{Deserialize, Deserializer, Serialize};
use std::sync::Mutex;

/// The backend the user asked for. Stored in performance settings by its
/// serialized name, which must stay stable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum GraphicsBackend {
    /// The adapter `editor_adapter_index` prefers among every processing backend.
    #[default]
    Automatic,
    Vulkan,
    Metal,
    Dx12,
}

impl GraphicsBackend {
    /// The wgpu backend, or `None` for Automatic.
    pub(crate) fn backend(self) -> Option<wgpu::Backend> {
        match self {
            Self::Automatic => None,
            Self::Vulkan => Some(wgpu::Backend::Vulkan),
            Self::Metal => Some(wgpu::Backend::Metal),
            Self::Dx12 => Some(wgpu::Backend::Dx12),
        }
    }

    /// The choice for `backend`, or `None` for a backend that cannot be chosen.
    /// GL is never offered: CalibRaw does not process photos on it
    /// (`calibraw_gpu::processing_backends`).
    fn from_backend(backend: wgpu::Backend) -> Option<Self> {
        match backend {
            wgpu::Backend::Vulkan => Some(Self::Vulkan),
            wgpu::Backend::Metal => Some(Self::Metal),
            wgpu::Backend::Dx12 => Some(Self::Dx12),
            _ => None,
        }
    }

    /// The API's name, which is not translated; Automatic is labelled by the
    /// caller.
    pub(crate) fn api_name(self) -> Option<&'static str> {
        match self {
            Self::Automatic => None,
            Self::Vulkan => Some("Vulkan"),
            Self::Metal => Some("Metal"),
            Self::Dx12 => Some("Direct3D 12"),
        }
    }
}

/// Reads a stored choice, taking a name this version does not know (written by
/// a newer one) as Automatic rather than failing the whole settings file.
pub(crate) fn deserialize_lenient<'de, D>(deserializer: D) -> Result<GraphicsBackend, D::Error>
where
    D: Deserializer<'de>,
{
    let name = String::deserialize(deserializer)?;
    Ok(GraphicsBackend::deserialize(
        serde::de::value::StrDeserializer::<serde::de::value::Error>::new(&name),
    )
    .unwrap_or_default())
}

/// What choosing the window's adapter found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BackendSupport {
    /// Backends with an adapter that can draw to the window and process
    /// photos, in enumeration order.
    pub(crate) available: Vec<GraphicsBackend>,
    /// The backend Automatic picks.
    pub(crate) automatic: Option<GraphicsBackend>,
    /// The backend the editor runs on in this process.
    pub(crate) active: Option<GraphicsBackend>,
}

impl BackendSupport {
    /// The backend `choice` would run on at the next start, as long as the
    /// machine's adapters stay the same.
    pub(crate) fn resolve(&self, choice: GraphicsBackend) -> Option<GraphicsBackend> {
        if self.available.contains(&choice) {
            Some(choice)
        } else {
            self.automatic
        }
    }
}

/// Set by the adapter selector each time the window's adapter is chosen.
static SUPPORT: Mutex<Option<BackendSupport>> = Mutex::new(None);

/// Records what choosing the window's adapter found: the `(backend, device
/// type)` of each adapter that can draw to it, those `processing` accepts,
/// and the indices of the adapters Automatic and the editor picked.
pub(crate) fn record_support(
    adapters: &[(wgpu::Backend, wgpu::DeviceType)],
    processing: wgpu::Backends,
    automatic: Option<usize>,
    active: Option<usize>,
) {
    let choice_at = |index: Option<usize>| {
        index
            .and_then(|index| adapters.get(index))
            .and_then(|(backend, _)| GraphicsBackend::from_backend(*backend))
    };
    let mut available = Vec::new();
    for (backend, _) in adapters {
        if !processing.contains(wgpu::Backends::from(*backend)) {
            continue;
        }
        if let Some(choice) = GraphicsBackend::from_backend(*backend) {
            if !available.contains(&choice) {
                available.push(choice);
            }
        }
    }
    let support = BackendSupport {
        available,
        automatic: choice_at(automatic),
        active: choice_at(active),
    };
    // A poisoned lock still holds a whole `Option`; the new value replaces it.
    *SUPPORT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(support);
}

/// What the last adapter choice found; `None` before the window has an adapter.
pub(crate) fn support() -> Option<BackendSupport> {
    SUPPORT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}
