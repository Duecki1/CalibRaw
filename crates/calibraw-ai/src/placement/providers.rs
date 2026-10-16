//! The execution providers a session tries, in order, before the CPU.

use super::quirks;
use super::session::{CpuFallbackProfile, SessionOptions};
use crate::config::{MemoryClass, CAPABILITIES};
use ort::ep::{ExecutionProvider, ExecutionProviderDispatch};

pub(super) struct ProviderCandidate {
    pub(super) name: &'static str,
    pub(super) provider: ExecutionProviderDispatch,
    pub(super) force_sequential: bool,
    /// GPU memory a new session may still use, where the provider's adapter
    /// can be queried.
    pub(super) available_memory: Option<u64>,
}

/// Adds `provider` if the loaded ONNX Runtime was built with it, returning the
/// new candidate. A provider the runtime lacks is expected (CPU-only packages)
/// and is not a degradation.
fn push_if_runtime_available<'a, E>(
    providers: &'a mut Vec<ProviderCandidate>,
    name: &'static str,
    provider: E,
    force_sequential: bool,
) -> Option<&'a mut ProviderCandidate>
where
    E: ExecutionProvider + Into<ExecutionProviderDispatch>,
{
    match provider.is_available() {
        Ok(true) => {
            providers.push(ProviderCandidate {
                name,
                provider: provider.into(),
                force_sequential,
                available_memory: None,
            });
            providers.last_mut()
        }
        Ok(false) => {
            log::info!("AI execution provider {name} is not built into the selected ONNX Runtime");
            None
        }
        Err(error) => {
            log::warn!(
                "AI execution provider {name} skipped: could not query availability: {error}"
            );
            None
        }
    }
}

pub(super) fn preferred_execution_providers() -> Vec<ProviderCandidate> {
    let mut providers = Vec::new();

    #[cfg(target_os = "windows")]
    {
        let adapter = super::directml::editor_adapter();
        let directml = match &adapter {
            Some(adapter) => ort::ep::DirectML::default().with_device_id(adapter.index),
            None => ort::ep::DirectML::default(),
        };
        if let Some(candidate) =
            push_if_runtime_available(&mut providers, "DirectML", directml, true)
        {
            candidate.available_memory = adapter.map(|adapter| adapter.available_bytes);
        }
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

pub(super) fn cpu_provider(options: &SessionOptions) -> ExecutionProviderDispatch {
    let disable_arena = CAPABILITIES.memory == MemoryClass::Constrained
        || options.cpu_fallback_profile == CpuFallbackProfile::LowMemory
        || (options.cpu_fallback_profile == CpuFallbackProfile::SamEncoder
            && quirks::sam_encoder_cpu_needs_unfused_graph());
    ort::ep::CPU::default()
        .with_arena_allocator(!disable_arena)
        .build()
}

/// Why a provider cannot hold a session needing `required` bytes when
/// `available` are free, or `None` if it fits or either is unknown.
pub(super) fn gpu_memory_shortfall(
    required: Option<u64>,
    available: Option<u64>,
) -> Option<String> {
    let (required, available) = (required?, available?);
    (available < required).then(|| {
        format!(
            "{} MiB of GPU memory free, {} MiB needed",
            available / (1 << 20),
            required / (1 << 20)
        )
    })
}

#[cfg(test)]
mod tests {
    use super::gpu_memory_shortfall;

    #[test]
    fn gpu_providers_are_skipped_only_when_memory_is_known_to_be_short() {
        const MIB: u64 = 1 << 20;
        assert!(gpu_memory_shortfall(Some(1800 * MIB), Some(1200 * MIB)).is_some());
        assert_eq!(
            gpu_memory_shortfall(Some(1800 * MIB), Some(1800 * MIB)),
            None
        );
        assert_eq!(gpu_memory_shortfall(None, Some(0)), None);
        assert_eq!(gpu_memory_shortfall(Some(1800 * MIB), None), None);
    }
}
