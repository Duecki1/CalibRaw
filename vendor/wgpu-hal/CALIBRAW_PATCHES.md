# CalibRaw diagnostic patch

Source: the unmodified crates.io `wgpu-hal` 29.0.4 package, with its upstream
MIT/Apache-2.0 licenses retained. The local change is confined to Vulkan error
conversion in `src/vulkan/mod.rs`:

- Before converting a Vulkan failure into `DeviceError::Unexpected`,
  `OutOfMemory` or `Lost`, log the original Vulkan result and a Rust backtrace
  identifying the backend operation. Unexpected results otherwise lose their
  native error code before the application receives its device-lost callback.

No Vulkan calls, flags, return values or panic features change. Backtraces are
captured only on these error paths. CalibRaw's bounded persistent logger keeps
these records outside logcat. Keep the version pinned and review/remove this
patch when updating wgpu.
