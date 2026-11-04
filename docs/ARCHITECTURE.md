# Architecture

This document records what each part of CalibRaw owns, which dependencies are
allowed, how the allowed graph is enforced, and the threading contracts that
background work follows. Update it in the change that alters a boundary.

## Crates and ownership

| Crate | Owns | May depend on | Must not depend on |
|---|---|---|---|
| `calibraw-core` | Image and edit models, colour mathematics, geometry, parameter validation, RAW/rendered loading, sidecars, presets, the LUT library and `.cube` parsing (`luts`), thumbnail cache | std, decoding and serde crates | any other CalibRaw crate, egui/eframe |
| `calibraw-gpu` | Headless devices (`device`), adapter memory queries (`memory/`), GPU resource planning, WGSL composition, processing passes, readback, tiled export and file encoding | `calibraw-core`, wgpu, naga_oil; in `memory/`, the platform APIs under wgpu (`windows` DXGI, `ash`, `objc2-metal`) | UI crates, egui/eframe |
| `calibraw-ai` | AI configuration and device capabilities (`config`), ONNX Runtime loading, its WebGPU plugin and the automatic packages (`runtime/`), where sessions run (GPU if they fit in free memory, else CPU), CPU memory admission, CPU fallback, GPU failure handling and platform and provider quirks (`placement/`), the model table, downloads and installation (`registry/`), the session slot (`slot`), the job worker (`jobs`), model-specific pre/postprocessing (`models/`) | `calibraw-core`, `calibraw-gpu` (denoise/remove render paths) | UI crates, egui/eframe |
| `calibraw-ffi` | C ABI and the Android JNI bridge: storage, pickers, export publishing, notifications, replay encoder | `calibraw-core` (sidecar types), jni, android-activity | processing crates, UI crates, egui/eframe |
| `calibraw-ui` (lib `calibraw`) | Application coordination (`app/`), views (`ui/`), presentation-independent services (`services/`), preview presentation, platform capabilities and OS services (`platform/`) | all crates above, eframe/egui, `moduwu-design`; in `platform/`, the OS crates (`calibraw-ffi`, `rfd`, `trash`, `zbus`, `windows-sys`, `objc2` AppKit bindings, Discord IPC, loggers) | — |
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

Source rules (in code; comments and strings are ignored):

- `crates/calibraw-ui/src/services/` must not use `egui`, `eframe` or
  `egui_wgpu`, since a module cannot be held to a narrower dependency set than
  its crate by Cargo.
- `calibraw-ai`'s `models/`, `registry/`, `jobs.rs` and `slot.rs` must not use
  `target_os`: they read `config::AiCapabilities` (what the device can do) and
  `placement::quirks` (platform bugs) instead.
- `crates/calibraw-ui/src/` outside `platform/` must not use `target_os`,
  `calibraw_ffi`, `rfd`, `zbus`, `windows_sys`, `objc2*`, `env_logger` or
  `android_logger`: UI code reads `PlatformCaps` and calls `Platform`.
  `cargo xtask cfg-ratchet` does not count `platform/` either.

"egui family" is egui, egui-wgpu, eframe and egui-winit. Add a rule in the
change that establishes a boundary, not in advance.

### Decoupling decisions

- GPU processing is headless. `RawGpuPipeline::output_view` exposes the display
  output; `calibraw-ui` registers it with egui (`app::preview_texture`).
- The Android bridge wakes the UI through a plain callback (`attach_ui`), not
  an `egui::Context`.
- Export destinations are explicit: `ExportTarget::File` is written beside the
  path and renamed into place; `ExportTarget::NewFile` (desktop batch and
  carousel exports) is moved into place without replacing anything, taking the
  next free `{stem}-N` name; `ExportTarget::Descriptor` (Android MediaStore)
  is written in place with intermediates in a caller-chosen staging directory.
  The exporter no longer asks the platform bridge which kind a path is.
- A carousel export (`ExportFiles::Carousel`) renders a window of the output
  raster once, with the photo's geometry unchanged, and cuts its encoded rows
  into one file per slide (`SlideWriter`). Sharpening runs on the whole window,
  so slides continue each other pixel for pixel; a watermark is placed on every
  slide, each as its own frame. Its switch and placement belong to the open
  photo and are not saved in the sidecar.
- A batch export may apply a preset and a LUT to every photo
  (`ExportOverride`). They change only the edit snapshot each item is rendered
  from; sidecars are never written.
- A camera profile changes only how the samples' colour is read
  (`ProfileColor`), never the samples. A decode keeps what it read the colour
  from (`ProfileColorSource`), so another profile is applied to the decode in
  memory (`LoadedRaw::with_camera_profile_selection`), sharing its
  `SensorSamples`, exactly as decoding with that profile would apply it. One
  `DecodedRawCache` entry therefore serves every camera-profile request; only
  a decode with the embedded matrix alone, which did not read the RAW's
  calibration signature, cannot serve requests that match DCPs.

## Inside `calibraw-ui`

- `app/` — `CalibRawApp` and its state structs, command handling (`AppAction`),
  document lifecycle, worker startup and polling, persistence scheduling.
- `ui/` — views. They read state, keep view-local drafts, selection, focus and
  drag state, and return actions; application handlers perform mutation,
  persistence and background work.
- `services/` — presentation-independent workflows (no egui/eframe), currently
  edit replay. Progress is reported through callbacks and failures through
  typed errors; the app turns them into repaints and displayed state.
- `platform/` — the only module that names an OS (see
  [Platform boundaries](#platform-boundaries)).

## State ownership

| Kind | Owner | Examples |
|---|---|---|
| Document state | `DevelopState`, `MaskState::stack`, `InpaintState::edits`, `PersistenceState::history` | exposure, geometry, masks, remove/retouch edits, undo history |
| Interaction state | `DevelopUiState`, `MaskState` drag fields, view-local `egui` memory | crop/mask drags, pickers, open sections |
| Active work | `ForegroundOperation`, `ExportTask`, `PreviewState` receivers (including `full_resolution`), `AiState::update`, `InpaintState::receiver` | AI masks, export, preview rebuilds, the idle full-resolution render |
| Caches | `DevelopState::decoded_raws` (shared with workers), `PreviewState::program_template`, `MaskState` caches, `AiState::people` (kept beside the RAW as `.calibraw-people`), library thumbnail caches, `calibraw-ai` model runtime, `calibraw-gpu` shared relighting surface | decoded RAWs, compiled GPU programs, AI inference results, the people found in the photo and their body parts, the surface derived from the current scene depth while a pipeline shows it |

`PersistenceState::document_generation` identifies the open document: it
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
| ONNX Runtime sessions | one process-wide slot in `calibraw-ai::slot`, locked by the running job; every AI job runs on its own `jobs::spawn_job` thread |

### Background jobs

| Job | Queue | Cancellation | Document change | Failure / disconnect |
|---|---|---|---|---|
| Document load | single receiver; a new load replaces it | the replaced receiver is dropped and its result discarded | the load installs the new document and bumps `document_generation` | notice or unsupported-file dialog; a disconnected worker is reported |
| Neighbour prefetch (desktop) | one thread after each interactive load; prepares the library neighbours into `DecodedRawCache` | `AtomicBool` checked between steps; a new load or a profile-settings change cancels it | results only fill caches; an open of a photo being prefetched waits for that decode | ignored; the open reports its own failure |
| Preview rebuild / detail | single receiver each; latest request wins | replaced receiver discards the stale result; `PreviewState::revision` rejects outdated detail renders. The zoomed crop's worker (`DetailRebuild`) also holds an `AtomicBool`, set when the view moves (`note_preview_motion`): it checks it every few rows it resamples (`build_region_proxy_until`) and ends with `Cancelled`; the next crop starts once that is reported and the view has settled. A finished crop is not installed while a drag is followed by the reduced preview | receivers cleared with the preview (`drop_detail_rebuild` also sets the flag) | disconnect clears the pending state |
| Reduced preview warm-up | one receiver in `InteractiveState`, started when the editor is quiet and the GPU is slow | none; short | the result is used only while its source is still the fitted proxy or the zoomed crop's raw (`Arc::ptr_eq`); the graph is dropped when that changes (the fitted proxy, or the zoomed crop's raw), when an export starts, or when the GPU is no longer slow | a dead worker is an error dialog; a graph the budget cannot hold is skipped with a diagnostics line; neither is retried for that proxy |
| Full-resolution preview (desktop) | one `RunningRender` in `PreviewState::full_resolution`, started after the editor has been idle; no second one starts until its result arrives | `AtomicBool`, set by `FullResolutionState::observe` when the fitted preview shows another `FittedOutput` (texture and `output_revision`), and at once by an edit, a moved view or a pressed pointer. The worker renders one stage at a time and waits for the GPU between them (`wait_for_gpu`), so a cancel is noticed before the next stage | the result is installed only when `FittedOutput` and `document_generation` still match | a photo whose graph does not fit the adapter's free memory or texture limit is skipped with a diagnostics line; a failed render is an error dialog; neither is retried for the document |
| Foreground AI / lens correction / auto straighten and perspective | one `ForegroundOperation` slot; a second request is refused | shared `AtomicBool`; the worker stops at its next safe point | results are applied only if `document_id` equals the current `document_generation`; an automatic geometry estimate also needs the flips and skews it was made for (and, for straightening, the perspective correction) | error dialog or notice; slot cleared |
| People mask from the analysis | `AiState::people_composes`, one per people component; a newer change of the component replaces its entry. Runs no model, so it never takes the foreground slot | none; the replaced receiver is dropped and its result discarded | `forget_people` and history restores clear the entries; a result is applied only while its component still has the person and parts it had when it started | error dialog |
| Remove / retouch | one receiver in `InpaintState` | `AtomicBool` | `reset_for_document` sets the flag and drops the receiver, so late results are discarded | notice; the pending stroke is kept only when the user must re-consent to a download |
| Export / batch export / replay | one `ExportTask`; batch items run sequentially | `AtomicBool`; partial files are removed and never reported as success | export works on an immutable snapshot of the edit | notice; temporary output removed |
| Export watermark image | one receiver in `WatermarkState`: the saved image read at start-up, or a newly chosen one decoded and kept as `watermark.png` beside the settings | none; the card cannot choose another image while a load runs, and a picker result that arrives meanwhile replaces it | none; the watermark is an app setting, not part of a document, and each export takes the image it holds when it starts | error dialog; a saved image that cannot be read turns the watermark off |
| Sidecar save | `VecDeque` of requests, one write in flight | none; writes are short | requests carry `generation` and `revision`; stale completions are ignored | failure keeps a recovery request and shows a dialog |
| Heap purge (Android, Linux glibc) | one `calibraw-heap-purge` thread in `calibraw_core::system_memory`; requests made while a purge is queued or running are served by one more | none; a purge takes up to a few hundred milliseconds | none; it only returns pages the allocator already freed | a thread that cannot start leaves memory with the allocator; an Android without `M_PURGE` changes nothing |
| Library ratings and flags | one `calibraw-review-writer` thread per library, started by the first change; saves run in the order they were made | none; a later change to a photo queues behind the earlier one, and the grid shows a saved review only once its photo has no save waiting | the open photo changes through its document: on Android its review is saved with the edits, on desktop with its editing time | each save reports the review its sidecar then holds; failures show the error dialog; library photo actions, opening a photo and exit wait up to two seconds for queued saves, as a media-library save rewrites the whole sidecar |
| Library thumbnails | shared work queue for a worker pool | `AtomicU64` generation; workers exit when it changes | generation bump on folder change | each item's result, success or error, is sent with its generation; older generations are ignored |

Job channels are unbounded `std::sync::mpsc` channels: each job sends a
bounded number of progress events and one result, drained each frame by
`drain_worker_events`, which stops at a terminal event and reports a
disconnected worker so the app never waits on a dead job. The library
thumbnail pool is the exception: its request and result channels are bounded
(`MAX_PENDING_THUMBNAILS`, `MAX_PENDING_THUMBNAIL_RESULTS`), so a large folder
applies back-pressure to the scanner and workers instead of growing memory.
GPU readbacks wait on one-slot channels.

### Synchronization

| Lock | Owner | Holders | Purpose |
|---|---|---|---|
| decode gate (`RwLock<()>`) | `LibraryState`, shared by `decode_gate()` | write: document load and batch-export decodes, and the UI thread while clearing the thumbnail cache; read: library thumbnail, reference-preview and neighbour-prefetch decodes | an interactive decode runs alone; background decodes share the gate and yield; the cache is never cleared under a reader |
| `REFERENCE_PREVIEW_SERIAL` | `ui::develop` | the reference-preview worker | one reference decode at a time |
| `DEVELOPED_THUMBNAIL_GPU` | `ui::library::thumbnails::developed` | thumbnail workers | one headless device for developed thumbnails, used by one render at a time |
| thumbnail work queue and request receiver | `ThumbnailWorkerContext` | thumbnail workers | short critical sections; never held while decoding |
| rendered-thumbnail permits (counting semaphore) | `calibraw_core::thumbnail_cache` | thumbnail workers rendering a full RAW (no embedded preview, or edited) | at most four such renders, fewer with fewer workers, bounding indexing memory |
| model runtime slot | `calibraw_ai::slot` | the running AI job | one ONNX session at a time; evicted models unload once released |
| `RUNTIME_INIT_LOCK`, runtime probe cache, runtime install lock | `calibraw_ai::runtime` | AI jobs, the Settings runtime picker | serialize ONNX Runtime initialization, probes and automatic runtime installs |
| provider statuses | `calibraw_ai::placement` | AI jobs, Settings | the provider each model's latest session uses |
| out-of-memory records | `calibraw_ai::placement::out_of_memory` | AI jobs, also under the model runtime slot; Settings when GPU AI is switched | which models ran out of GPU memory and when they may retry; a leaf lock, never held while a session is created or run |
| AI settings (`RwLock`) | `calibraw_ai::config` | the UI thread writes at startup and on Settings changes; AI jobs read | runtime source and model root; never held across a load or download |
| `SHARED_SURFACE` | `calibraw_gpu` scene-depth upload | any thread uploading scene depth: preview pipelines on the UI thread, export workers | the relighting surface is derived once per depth result and shared by every pipeline; held while deriving, so a concurrent upload of the same result waits instead of repeating it |
| effect inputs (`EffectInputs`) | each `RawGpuPipeline` | threads encoding that pipeline | allocates scene depth, the scene shadow map, the image-light map, haze map, scene map, contour grid or scene emission textures the first time an effect reads them, and the LUT texture when a LUT is uploaded, and rebinds the passes that read them; encoding holds it only to clone a bind group, never while waiting on the GPU |
| LUT cache | `calibraw_core::luts::LutStore`, shared by its clones | the UI thread, document loads, batch export and thumbnail workers | the recently parsed LUTs; a leaf lock, never held while a file is read or parsed |
| `SIDECAR_SAVE_LOCK` | `calibraw_core::sidecar::files` | sidecar writers | one sidecar write at a time |
| decoded-RAW cache state and condvar | `DecodedRawCache` | document loads and the prefetch worker | short critical sections; decodes run unlocked, and a second request for a decoding entry waits on the condvar |
| per-entry lens-correction slot | `DecodedRaw` | document loads and the prefetch worker | held while correcting, so one correction per selection runs at a time |
| per-entry recoloured-copy slot | `DecodedRaw` | document loads and the prefetch worker | held while another camera profile is applied to the decode (reading its DCP); never held with the lens-correction slot |
| serialized-read gate (HDD mode) | `calibraw_core::serialized_reads` | thumbnail, display-metadata and RAW loads, only while HDD mode is on | one photo file read at a time; decoding stays outside; a leaf lock, re-entered inline on the holding thread |
| `TextureRetirement` | `PreviewState` | any thread dropping a `PreviewPipeline`; drained by the UI thread | short pushes and one drain per frame |
| FFI result queues, `REPAINT_NOTIFIER` | `calibraw_ffi::android` | Java callback threads push; the UI thread pops | short critical sections; no JNI call while held |

Apart from the serialized-read gate, only these nestings exist, each in a fixed
order: the reference-preview serial lock is taken before the decode gate; a
thumbnail worker takes `DEVELOPED_THUMBNAIL_GPU` while holding the decode gate
for reading; and a decode that owns a decoded-RAW cache entry takes the decode
gate without holding the cache lock, so waiting for an entry never holds the
gate. No code takes the decode gate while holding any other lock in this
table. The serialized-read gate may be taken while holding any of them, but none
of them is taken while holding it. Poisoned locks are recovered
(`PoisonError::into_inner`) where the protected data stays valid, and reported
as errors where it may not.

### Shutdown

`eframe::App::on_exit` empties the warm AI feature set (models unload once
their job releases them), stops Discord presence, clears the Android task
notification, persists performance settings and flushes the pending sidecar
synchronously. Worker threads are detached; GPU and model resources are
released when their owners drop.

## GPU resources

`RawGpuPipeline` owns all textures and buffers of one processing graph and a
`GpuBudgetReservation` (RAII) against the process-wide GPU working-set limit.
The LUT texture is sized by its LUT, so it is not part of that plan: it takes
a reservation of its own when a LUT is first uploaded, and pipelines without a
LUT reserve nothing for it.
A graph is built for one `PixelLayout` of its source. A mosaic is
reconstructed and demosaiced from the CFA textures. A preview proxy that
reduces a mosaic is binned to co-sited RGB instead (`BinnedSensor`, built by
`build_region_proxy`), so detail near the reduced resolution cannot demosaic
into false colour: its graph keeps the pixels in the RGB source texture and
replaces highlight reconstruction and demosaicing with
`binned_highlight_reconstruct` and the raster finish program. A raster takes
the raster finish alone. Graphs reused for new pixels (`upload_raw_tile`)
take only sources of their own layout.
The graphs that show a photo with camera profiles to choose from (fitted,
navigation and zoomed crop) size their profile buffer for
`PREVIEWED_PROFILE_BYTES`, so a hovered profile renders on them
(`GpuParams::new_with_profile_color`) without a rebuild; the preview budget
already counts that room.
Presenting its output is a UI concern: `PreviewPipeline` pairs a pipeline with
its egui registration, so a displayed pipeline cannot outlive or leak its
texture. On Android a replaced detail preview frees its texture immediately
(`free_now`) to return memory before the replacement is allocated.
The editor's previews (fitted, zoomed crop, navigation proxy) render in
batches: each frame submits every pending stage of each preview, at most one
batch is in flight (`PreviewDispatchGate`, opened by `on_submitted_work_done`),
and edits made meanwhile only update the target parameters, so the next batch
renders the newest ones. While a control is dragged zoomed in and only the
Output stage changes, the fitted preview and the navigation proxy wait until
the pointer lifts (`defer_background_preview_processing`). Where the dispatch
gate's timings show the GPU takes more than about 45 ms for a batch of the
preview that is shown (a device with little memory is assumed slow until it
has timings) and the setting `interactive_preview_fallback` is on (on by
default where memory is constrained, off on desktop), the drag is followed by a second, smaller graph at half the edge
instead (`interactive_preview.rs`): the whole photo at half the fitted edge,
or, zoomed in, the sharp crop's part of the photo at half the crop's edge
(`interactive_crop.rs`), drawn with the crop's coordinates. The sharp preview
keeps merging the stage it owes and renders it when the drag ends or pauses;
the reduced picture stays until that is done. The sharp preview does not start
at the release or pause itself: the reduced preview keeps the edits for another
`CATCH_UP_DELAY`, because the GPU cannot take back a batch it was given and a
control grabbed again within that finds the GPU free. While a zoomed drag is followed,
the navigation proxy still renders Raw and Tone changes, because the reduced
crop takes its tone statistics from it. There is one reduced graph at a time,
built from the sharp preview's source (the fitted proxy, or the crop's raw);
it counts against the same GPU budget and is dropped when that source changes,
when an export starts, or when the GPU is no longer slow.
The fitted preview graph is built with mask layers for its masks plus spare layers in steps of four (`fitted_pipeline_options`, `preview_mask_layer_capacity`), not every layer the editor allows; one with fewer layers than masks is rebuilt (`fitted_mask_capacity_stale`) and keeps showing its last frame until then. On Android the fitted and zoomed graphs are sized against shares of the RAM-scaled working-set limit that remain after an export tile and the navigation graph (`MobilePreviewRole`, `bounded_mobile_preview_edge`); a Preview graph accounts only for its RGBA8 readback, since RGBA32F readbacks need `ProcessingQuality::High`. Where the platform allows (`PlatformCaps::full_resolution_preview`), an idle editor renders the whole photo from the native RAW on a worker thread with its own graph and shows the scaled result (`app/full_resolution.rs`) in place of the fitted texture while the fitted output is unchanged.
Each mask effect is described once, in the effect registry of `calibraw-core`
(`pipeline/masks/effects/`): its shader ID, the lanes its settings pack, the passes it
runs in, how far it reads and which scene inputs it needs. Validation, export-tile
halos and scene depth generation read it there; `calibraw-gpu` writes the IDs, lanes
and the creative pass's per-slot dispatch into the creative module (`effect_wgsl.rs`)
and decides from it which passes and maps to prepare. Fog, Smoke, Grain and Vignette
read the photo and the scene through `mask_effects/effect_api.wgsl` and `medium.wgsl`;
Relight, Rain and Wet Surfaces shade the scene's surfaces, use its camera or find its
ground, and read the scene files below directly.

Every mask effect and the scene it reads compile into one module
(`creative_effects.wgsl` with `mask_effects/`), and every effect pass binds the same
scene, so an effect may read any of it whichever pass it runs in:

- `scene_depth.wgsl` is the one depth (`scene_depth_at`). Fog, Smoke and Wet Surfaces
  read it directly; Relight asks it which part of the surface derived from depth a
  pixel lies on (`scene_surface_taps_at`), since shading needs that surface's own
  smooth depth and gradient. It is relative distance, converted from the generated map
  (linear in inverse depth) on upload (`scene_depth.rs`); Depth Range masks select on
  the map itself.
- `scene_environment.wgsl` is the scene's light as effects receive it: its ambient
  level, the photo's coarse colour and the light sources in the photo.
- `scene_lights.wgsl` turns each of Relight's lights and Light Rays' sources into a
  light any effect receives, and `medium.wgsl` is what Fog, Smoke and Rain's mist
  share to scatter them.
- `scene_shadows.wgsl` says how much of each of those lights reaches a point past the
  surfaces depth describes: one full-image map, traced in the output stage only when
  depth or a light's placement changes (`GpuParams::scene_shadow_map_key`), holds the
  surface and slices of the air for up to four lights. Every receiver reads it
  (`scene_shadow_air_at`, `scene_shadow_surface_at`), so a light behind a subject
  lights nothing in front of it; Relight's own shading applies it with its Shadows
  strength.
- `scene_emission.wgsl` makes effects that emit over an area (a self-illuminating
  Glow, Neon's lines) lights too. Each emitter gives its radiance at a full-image
  point from full-image inputs only (its parameters, its mask in the full-image mask
  atlas, the contour grid of the tone stage), so every export tile and zoomed view
  builds the same coarse emitter grid at the start of its output stage. Two passes
  then spread that grid with inverse-square falloff over the scene's surfaces and
  along view rays through the media's depth cells, and receivers read only those
  maps, whatever the number of emitters. A new emitter is one case in
  `scene_emission_slot_at` and `scene_emission_present`, and one in
  `MaskEffect::reads_full_image_mask`.

All of it but the lights comes from the scene map (`scene_analysis.wgsl`), a
`FullImageMap` holding the photo's coarse colour, a guided filter's coefficients that
refine scene depth against the photo, how much sky lies around each cell and how
bright what stands before it is (so that light fades out toward a soft sky outline,
`scene_sky_at`), stored depth's range around each cell, the ground with its terrain
height, and how much the lens blurs each depth. It depends on
the scene, scene depth and the direction of up (`GpuParams::with_vignette_geometry`),
not on the rest of the edit, and is rebuilt when any of them changes.

Rain and Wet Surfaces do not run inside the creative pass: `apply_wet_surfaces`
runs before it and `apply_weather_air` after it, so their result does not depend on
the order of their cards and edits without them run the same passes as before. Each
pass reads the image from one work texture and writes the other, and
`encode_output_stage` tracks which one holds it.
Compiled programs are shared through `RawGpuProgramTemplate` and the persistent
pipeline cache. Rust/WGSL layout contracts are tested in
`calibraw-gpu/src/pipeline/gpu/layout_contract_tests.rs` against the modules the
production `ShaderManager` composes.

## Platform boundaries

- Android: `calibraw-ffi` exposes content-URI storage, document pickers,
  MediaStore publishing, background-task notifications and the MediaCodec replay
  encoder. Java classes live in `android/app/src/main/java/de/duecki/calibraw`.
- `calibraw-ui/src/platform/` decides platform differences once. Everything
  else compiles on every target and branches at run time:
  - `PlatformCaps` (`form_factor`, `storage`, `memory`,
    `tasks_need_foreground`, `configurable_ai_runtime`, `video_encoder`,
    `can_restart`) is what views and handlers read. Views take it, or
    `app.caps()`, and return actions; leaf widgets without the app use
    `PlatformCaps::CURRENT`. Data that already names its kind is matched
    instead: `LibraryLocator`, `SidecarTarget` and `ExportDestination`.
  - `Platform` (`Arc<dyn Platform>`, in `CalibRawApp::platform` and
    `LibraryState`) is the service behind actions: the Android bridge's
    pickers, media library, sidecars, thumbnails, publishing, clipboard,
    notifications, system bars and replay encoder, and the desktop's settings
    and cache folders and GPU-loss restart. Its default methods answer
    "unavailable" or do nothing, so a backend implements only what it has;
    `release_free_memory` defaults to the heap purge every backend shares,
    which tests replace to observe it.
    `platform/android.rs` wraps `calibraw-ffi`; `platform/desktop/` holds the
    window entry point, the Flatpak portal, Discord IPC, on macOS the menu
    bar (`macos_menu`, whose Settings… reaches the app through
    `take_settings_request`) and, on Windows, the NVIDIA driver profile that
    keeps G-SYNC off for the window.
  - Stateless desktop services are free functions: `FileDialog` (`rfd`),
    `trash`, `show_fatal_message`, camera-profile search folders.
  - `platform::fake::FakePlatform` (tests) takes any `PlatformCaps` and
    scripted media-library contents and records the services called.
- Library storage lives in `ui/library/folders/` (folders on disk) and
  `ui/library/media_library/` (the Android library); `ui/library/sources.rs`
  dispatches on `PlatformCaps::storage`.
- Edit replay picks FFmpeg or the platform encoder from
  `PlatformCaps::video_encoder` (`services/replay/`).
