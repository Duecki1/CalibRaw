//! Platform and provider bugs that sessions work around, as opposed to what
//! a device can do (`config::AiCapabilities`). Each entry names the failure
//! it was added for and when to check whether it is still needed.

/// Nodes a model's WebGPU sessions leave to the CPU, for
/// `SessionOptions::webgpu_cpu_nodes`.
///
/// ONNX Runtime's WebGPU provider runs a Split node as one shader with a
/// storage buffer per input and output, and fails the run once a node needs
/// more than the adapter's `maxStorageBuffersPerShaderStage`: "Too many
/// storage buffers in shader. Current: 17, Max is 16". Concat with more
/// inputs than that runs fine.
///
/// Observed with plugin 0.4.0 on ONNX Runtime 1.30.0, NVIDIA Vulkan. Recheck
/// when the plugin changes; adapters with a lower limit fail on narrower
/// nodes and fall back to the CPU for the model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum WebGpuCpuNodes {
    #[default]
    None,
    /// BiRefNet's decoder splits feature maps 32 ways in `/decoder/Split` to
    /// `/decoder/Split_32` and 16 ways in `/decoder/Split_33` to `_49`, in
    /// every published export and precision.
    BiRefNetDecoderSplits,
}

impl WebGpuCpuNodes {
    /// The provider's `forceCpuNodeNames` value: node names, one per line.
    pub(crate) fn node_names(self) -> Option<String> {
        match self {
            Self::None => None,
            Self::BiRefNetDecoderSplits => Some(
                std::iter::once("/decoder/Split".to_owned())
                    .chain((1..50).map(|index| format!("/decoder/Split_{index}")))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        }
    }
}

/// SAM 2.1's Hiera encoder on Windows CPU builds of ONNX Runtime: after graph
/// fusions some builds return non-finite image features. CPU sessions of the
/// encoder therefore run without graph optimizations or the arena allocator,
/// and isolated non-finite values left over are replaced with zero instead of
/// failing the job (see `sam_encoder_tolerates_non_finite`). Thread count
/// does not affect it.
///
/// Observed with the Windows DirectML build of 1.24.4. P5 moved Windows to
/// Microsoft's 1.30.0 CPU build, where it has not been checked; recheck there.
pub(crate) const fn sam_encoder_cpu_needs_unfused_graph() -> bool {
    cfg!(target_os = "windows")
}

/// Whether the SAM encoder repairs a few non-finite CPU features instead of
/// failing; the same Windows CPU bug as `sam_encoder_cpu_needs_unfused_graph`.
/// Accelerated providers never get this tolerance.
pub(crate) const fn sam_encoder_tolerates_non_finite() -> bool {
    cfg!(target_os = "windows")
}

/// Whether SAM sessions may stay loaded between object-mask jobs. The Linux
/// AppImage unloads them (introduced in `837ab43d`, "Try fix pixelated"); the
/// reason is unknown. Recheck with the
/// WebGPU runtime, which the AppImage downloads like other Linux builds.
pub(crate) fn sam_sessions_may_stay_warm() -> bool {
    !(cfg!(target_os = "linux") && running_from_appimage())
}

fn running_from_appimage() -> bool {
    std::env::var_os("APPIMAGE").is_some() || std::env::var_os("APPDIR").is_some()
}
