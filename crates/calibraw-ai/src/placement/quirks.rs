//! Platform bugs that sessions work around, as opposed to what a device can
//! do (`config::AiCapabilities`). Each entry names the failure it was added
//! for and when to check whether it is still needed.

/// SAM 2.1's Hiera encoder on Windows CPU builds of ONNX Runtime: after graph
/// fusions some builds return non-finite image features. CPU sessions of the
/// encoder therefore run without graph optimizations or the arena allocator,
/// and isolated non-finite values left over are replaced with zero instead of
/// failing the job (see `sam_encoder_tolerates_non_finite`). Thread count
/// does not affect it.
///
/// Observed with the pinned Windows runtime, 1.24.4. Recheck when the Windows
/// runtime changes (docs/GPU_REWORK.md P5).
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
/// reason is unknown (docs/GPU_REWORK.md open decision 4). Recheck when the
/// AppImage loads the P5 runtime.
pub(crate) fn sam_sessions_may_stay_warm() -> bool {
    !(cfg!(target_os = "linux") && running_from_appimage())
}

fn running_from_appimage() -> bool {
    std::env::var_os("APPIMAGE").is_some() || std::env::var_os("APPDIR").is_some()
}
