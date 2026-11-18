# CalibRaw diagnostic patch

Source: the unmodified crates.io `wgpu-hal` 29.0.4 package, with its upstream
MIT/Apache-2.0 licenses retained. Local Vulkan changes include:

- Before converting a Vulkan failure into `DeviceError::Unexpected`,
  `OutOfMemory` or `Lost`, log the original Vulkan result and a Rust backtrace
  identifying the backend operation. Unexpected results otherwise lose their
  native error code before the application receives its device-lost callback.

- Native swapchain semaphores retain their device and are destroyed when their
  last owner drops. The swapchain releases its references after its idle wait.
  An acquired frame can outlive that wait during device-loss cleanup.

Backtraces are
captured only on these error paths. CalibRaw's bounded persistent logger keeps
these records outside logcat. Keep the version pinned and review/remove this
patch when updating wgpu.
