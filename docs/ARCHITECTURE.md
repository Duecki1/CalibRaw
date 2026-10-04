# Architecture

This document records what each part of CalibRaw owns, which dependencies are
allowed, how the allowed graph is enforced, and the threading contracts that
background work follows. Update it in the change that alters a boundary.

## Crates and ownership

| Crate | Owns | May depend on | Must not depend on |
|---|---|---|---|
| `calibraw-core` | Image and edit models, colour mathematics, geometry, parameter validation, RAW/rendered loading, sidecars, presets, thumbnail cache | std, decoding and serde crates | any other CalibRaw crate, egui/eframe |
| `calibraw-gpu` | GPU resource planning, WGSL composition, processing passes, readback, tiled export and file encoding | `calibraw-core`, wgpu, naga_oil | UI crates, egui/eframe |
| `calibraw-ai` | Model artifacts and installation, ONNX Runtime sessions, provider fallback, model-specific pre/postprocessing | `calibraw-core`, `calibraw-gpu` (denoise/remove render paths) | UI crates, egui/eframe |
| `calibraw-ffi` | C ABI and the Android JNI bridge: storage, pickers, export publishing, notifications, replay encoder | `calibraw-core` (sidecar types), jni, android-activity | processing crates, UI crates, egui/eframe |
| `calibraw-ui` (lib `calibraw`) | Application coordination (`app/`), views (`ui/`), presentation-independent services (`services/`), preview presentation | all crates above, eframe/egui, `moduwu-design` | — |
| `calibraw-cli` | Headless develop export and white-balance diagnostics | `calibraw-core`, `calibraw-gpu` | UI crates, egui/eframe |
| `xtask` | Repository tooling: architecture checks, line counts, baselines, Android build helpers, icons | std and tooling crates | product crates |
| `moduwu-design` (separate repository) | Reusable egui tokens, themes, controls, frames and responsive helpers | egui | any CalibRaw crate |

Application services start as modules (`calibraw-ui/src/services/`) and become a
crate only when a second real caller or dependency ownership justifies it.

### Enforced boundaries

`cargo xtask arch-check` runs in CI. Each rule inspects the package's resolved
normal and build dependency graph for every target platform
(`cargo tree -e normal,build --target all`) and fails if `cargo tree` itself
fails, so a lockfile or network error is never read as "dependency absent".

| Package | Forbidden in its graph | Since |
|---|---|---|
| `calibraw-core` | `calibraw-gpu`, `calibraw-ai`, `calibraw-ui`, `calibraw-ffi`, egui family | baseline |
| `calibraw-gpu` | `calibraw-ai`, `calibraw-ui`, `calibraw-ffi`, egui family | rework step 4 |
| `calibraw-ai` | `calibraw-ui`, `calibraw-ffi`, egui family | rework step 4 |
| `calibraw-ffi` | `calibraw-gpu`, `calibraw-ai`, `calibraw-ui`, egui family | rework step 4 |
| `calibraw-cli` | `calibraw-ui`, `calibraw-ffi`, egui family | rework step 4 |
| `moduwu-design` | any `calibraw*` package | baseline |

Source rule: `crates/calibraw-ui/src/services/` must not use `egui`, `eframe`
or `egui_wgpu` in code (comments and strings are ignored), since a module
cannot be held to a narrower dependency set than its crate by Cargo.

"egui family" is egui, egui-wgpu, eframe and egui-winit. Add a rule in the
change that establishes a boundary, not in advance.

### Decoupling decisions

- GPU processing is headless. `RawGpuPipeline::output_view` exposes the display
  output; `calibraw-ui` registers it with egui (`app::preview_texture`).
- The Android bridge wakes the UI through a plain callback (`attach_ui`), not
  an `egui::Context`.
- Export destinations are explicit: `ExportTarget::File` is written beside the
  path and renamed into place; `ExportTarget::Descriptor` (Android MediaStore)
  is written in place with intermediates in a caller-chosen staging directory.
  The exporter no longer asks the platform bridge which kind a path is.

## Inside `calibraw-ui`

- `app/` — `CalibRawApp` and its state structs, command handling (`AppAction`),
  document lifecycle, worker startup and polling, persistence scheduling.
- `ui/` — views. They read state, keep view-local drafts, selection, focus and
  drag state, and return actions; application handlers perform mutation,
  persistence and background work.
- `services/` — presentation-independent workflows (no egui/eframe), currently
  edit replay. Progress is reported through callbacks and failures through
  typed errors; the app turns them into repaints and displayed state.

## State ownership

| Kind | Owner | Examples |
|---|---|---|
| Document state | `DevelopState`, `MaskState::stack`, `InpaintState::edits`, `PersistenceState::history` | exposure, geometry, masks, remove/retouch edits, undo history |
| Interaction state | `DevelopUiState`, `MaskState` drag fields, view-local `egui` memory | crop/mask drags, pickers, open sections |
| Active work | `ForegroundOperation`, `ExportTask`, `PreviewState` receivers, `AiState::update`, `InpaintState::receiver` | AI masks, export, preview rebuilds |
| Caches | `DevelopState::raw_cache`, `PreviewState::program_template`, `MaskState` caches, library thumbnail caches, `calibraw-ai` model runtime | decoded RAWs, compiled GPU programs, AI inference results |

`PersistenceState::sidecar_generation` identifies the open document: it
increases whenever a different document is installed, and every document-bound
job records the value it started with.

## Threads and concurrency contracts

### Thread affinity

| Resource | Allowed threads |
|---|---|
| `CalibRawApp` state, egui UI state | the eframe UI thread only |
| `egui::Context` | any thread, for `request_repaint` only |
| `wgpu::Device` / `wgpu::Queue` | any thread; cloned into workers |
| egui-wgpu renderer (texture registration and release) | UI thread, through `frame.wgpu_render_state()`. `PreviewPipeline` owns a registration; dropping it queues the texture in `TextureRetirement`, which the UI thread releases at the start of the next frame, after the current frame can no longer paint it |
| JNI environment | the calling thread; `with_activity` attaches it for the call's duration. Java→Rust callbacks run on Java threads and only enqueue results and request a repaint |
| ONNX Runtime sessions | one process-wide slot in `calibraw-ai::model_runtime`, locked by the running job |

### Background jobs

| Job | Queue | Cancellation | Document change | Failure / disconnect |
|---|---|---|---|---|
| Document load | single receiver; a new load replaces it | the replaced receiver is dropped and its result discarded | the load installs the new document and bumps `sidecar_generation` | notice or unsupported-file dialog; a disconnected worker is reported |
| Preview rebuild / detail | single receiver each; latest request wins | replaced receiver discards the stale result; `PreviewState::revision` rejects outdated detail renders | receivers cleared with the preview | disconnect clears the pending state |
| Foreground AI / lens correction | one `ForegroundOperation` slot; a second request is refused | shared `AtomicBool`; the worker stops at its next safe point | results are applied only if `document_id` equals the current `sidecar_generation` | error dialog or notice; slot cleared |
| Remove / retouch | one receiver in `InpaintState` | `AtomicBool` | `reset_for_document` sets the flag and drops the receiver, so late results are discarded | notice; the pending stroke is kept only when the user must re-consent to a download |
| Export / batch export / replay | one `ExportTask`; batch items run sequentially | `AtomicBool`; partial files are removed and never reported as success | export works on an immutable snapshot of the edit | notice; temporary output removed |
| Sidecar save | `VecDeque` of requests, one write in flight | none; writes are short | requests carry `generation` and `revision`; stale completions are ignored | failure keeps a recovery request and shows a dialog |
| Library thumbnails | shared work queue for a worker pool | `AtomicU64` generation; workers exit when it changes | generation bump on folder change | each item's result, success or error, is sent with its generation; older generations are ignored |

All channels are unbounded `std::sync::mpsc` channels drained each frame by
`drain_worker_events`, which stops at a terminal event and reports a
disconnected worker so the app never waits on a dead job.

### Shutdown

`eframe::App::on_exit` empties the warm AI feature set (models unload once
their job releases them), stops Discord presence, clears the Android task
notification, persists performance settings and flushes the pending sidecar
synchronously. Worker threads are detached; GPU and model resources are
released when their owners drop.

## GPU resources

`RawGpuPipeline` owns all textures and buffers of one processing graph and a
`GpuBudgetReservation` (RAII) against the process-wide GPU working-set limit.
Presenting its output is a UI concern: `PreviewPipeline` pairs a pipeline with
its egui registration, so a displayed pipeline cannot outlive or leak its
texture. On Android a replaced detail preview frees its texture immediately
(`free_now`) to return memory before the replacement is allocated.
Compiled programs are shared through `RawGpuProgramTemplate` and the persistent
pipeline cache. Rust/WGSL layout contracts are tested in
`calibraw-gpu/src/pipeline/gpu/layout_contract_tests.rs` against the modules the
production `ShaderManager` composes.

## Platform boundaries

- Android: `calibraw-ffi` exposes content-URI storage, document pickers,
  MediaStore publishing, background-task notifications and the MediaCodec replay
  encoder. Java classes live in `android/app/src/main/java/de/duecki/calibraw`.
- Desktop: native file dialogs (`rfd`), trash, Discord presence and the `ffmpeg`
  replay writer are confined to `cfg(not(target_os = "android"))` modules in
  `calibraw-ui`.
