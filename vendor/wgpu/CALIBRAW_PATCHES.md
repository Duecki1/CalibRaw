# CalibRaw diagnostic patch

Source: the unmodified crates.io `wgpu` 29.0.4 package, with its upstream
MIT/Apache-2.0 licenses retained. The local change is confined to
`src/backend/wgpu_core.rs`:

- Log a failed `Queue::write_buffer_with` operation's phase (destination
  validation versus staging allocation/map), requested bytes, and original
  wgpu-core error before passing that error to the existing handler.

This does not change allocation, validation, error scopes, return values or
recovery behavior. It identifies whether egui's staging-buffer panic is a
consequence of an already lost device or follows a fresh upload failure.
Keep the version pinned and review/remove this patch when updating wgpu.
